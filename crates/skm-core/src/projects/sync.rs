use std::{
    collections::BTreeSet,
    sync::mpsc::{Receiver, Sender},
};

use rusqlite::{OptionalExtension, params};

use super::{
    Project, ProjectMode, apply_project, effective_project, list, plan_project, set_auto_sync,
    used_sets,
};
use crate::{
    Error,
    engine::{self, Action, ApplyResult, Observation, Ownership, Plan, PlannedAction},
    events::CoreEvent,
    library::Library,
    paths::Paths,
    registry::Registry,
    store::Store,
    target::{ItemKind, TargetClass},
};

#[derive(Debug, Clone)]
pub enum Change {
    Items(Vec<String>),
    Set(String),
}

#[derive(Debug)]
pub enum SyncOutcome {
    Pending {
        project_id: i64,
        count: usize,
    },
    Applied {
        project_id: i64,
        result: ApplyResult,
    },
    Refused {
        project_id: i64,
        issues: Vec<engine::PlanIssue>,
    },
    Failed {
        project_id: i64,
        error: Error,
    },
}

/// Each affected project is independent; a refusal never prevents the others syncing.
pub fn sync_change(
    paths: &Paths,
    store: &Store,
    library: &Library,
    registry: &Registry,
    change: &Change,
) -> Result<Vec<SyncOutcome>, Error> {
    let mut outcomes = Vec::new();
    for project in list(store)? {
        let keys = affected_changes(store, library, registry, &project, change)?;
        if keys.is_empty() {
            continue;
        }
        let id = project.id;
        store.write(move |connection| {
            let transaction = connection.transaction()?;
            for key in keys {
                transaction.execute(
                    "INSERT INTO project_pending(scope_id,change_key) VALUES (?1,?2) ON CONFLICT(scope_id,change_key) DO UPDATE SET revision=revision+1",
                    params![id, key],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })?;
        if !project.auto_sync {
            outcomes.push(SyncOutcome::Pending {
                project_id: id,
                count: pending_count(store, id)?,
            });
        } else {
            outcomes.push(sync_project(paths, store, library, registry, &project));
        }
    }
    Ok(outcomes)
}

fn affected_changes(
    store: &Store,
    library: &Library,
    registry: &Registry,
    project: &Project,
    change: &Change,
) -> Result<Vec<String>, Error> {
    match change {
        Change::Set(name) => Ok(if used_sets(store, project.id)?.contains(name) {
            vec![format!("set:{name}")]
        } else {
            vec![]
        }),
        Change::Items(ids) => {
            let selection = effective_project(store, library, project.id, registry)?;
            let mut keys = BTreeSet::new();
            for id in ids {
                let item: Option<(String, String)> = store.read(|connection| {
                    Ok(connection
                        .query_row("SELECT kind,name FROM items WHERE id=?1", [id], |row| {
                            Ok((row.get(0)?, row.get(1)?))
                        })
                        .optional()?)
                })?;
                let Some((kind, name)) = item else {
                    // Deleted index IDs no longer carry a name. Reconcile conservatively.
                    keys.insert(format!("item:{id}"));
                    continue;
                };
                let selected = match kind.as_str() {
                    "skill" => selection.skills.contains(&name),
                    "mcp" => selection.mcp.contains_key(&name),
                    "hook" => selection.hooks.contains_key(&name),
                    "subagent" => selection.subagents.contains_key(&name),
                    _ => false,
                };
                let owned = store.read(|connection| {
                    Ok(connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM ledger WHERE scope_id=?1 AND item_kind=?2 AND item_name=?3)",
                        params![project.id, kind, name], |row| row.get::<_, bool>(0),
                    )?)
                })?;
                if selected || owned {
                    keys.insert(format!("item:{id}"));
                }
            }
            Ok(keys.into_iter().collect())
        }
    }
}

pub fn pending_count(store: &Store, project_id: i64) -> Result<usize, Error> {
    super::require_project(store, project_id)?;
    store.read(|connection| {
        Ok(connection.query_row(
            "SELECT count(*) FROM project_pending WHERE scope_id=?1",
            [project_id],
            |row| row.get::<_, i64>(0),
        )? as usize)
    })
}

pub fn pause(store: &Store, project_id: i64) -> Result<(), Error> {
    set_auto_sync(store, project_id, false)
}

pub fn resume(
    paths: &Paths,
    store: &Store,
    library: &Library,
    registry: &Registry,
    project_id: i64,
) -> Result<SyncOutcome, Error> {
    set_auto_sync(store, project_id, true)?;
    let project = list(store)?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| Error::ConfigChanged("project disappeared".into()))?;
    Ok(sync_project(paths, store, library, registry, &project))
}

fn sync_project(
    paths: &Paths,
    store: &Store,
    library: &Library,
    registry: &Registry,
    project: &Project,
) -> SyncOutcome {
    let result = (|| {
        let pending: Vec<(String, i64)> = store.read(|connection| {
            let mut statement = connection
                .prepare("SELECT change_key,revision FROM project_pending WHERE scope_id=?1")?;
            Ok(statement
                .query_map([project.id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?)
        })?;
        let plan = plan_project(paths, store, project, library, registry)?;
        if !plan.issues.is_empty() {
            return Ok(SyncOutcome::Refused {
                project_id: project.id,
                issues: plan.issues,
            });
        }
        let result = engine::apply(paths, store, &plan, "auto-sync", None)?;
        if result.issues.is_empty() && result.failed.is_empty() {
            let id = project.id;
            store.write(move |connection| {
                let transaction = connection.transaction()?;
                for (key, revision) in pending {
                    transaction.execute("DELETE FROM project_pending WHERE scope_id=?1 AND change_key=?2 AND revision=?3", params![id,key,revision])?;
                }
                transaction.commit()?;
                Ok(())
            })?;
        }
        Ok(SyncOutcome::Applied {
            project_id: project.id,
            result,
        })
    })();
    result.unwrap_or_else(|error| SyncOutcome::Failed {
        project_id: project.id,
        error,
    })
}

/// Blocks until the input channel closes. Run on a caller-owned worker thread.
/// Tee ItemsChanged events here; other core events belong to the application's bridge.
pub fn run_watcher_sync(
    paths: &Paths,
    store: &Store,
    registry: &Registry,
    input: Receiver<CoreEvent>,
    output: &Sender<Result<Vec<SyncOutcome>, Error>>,
) {
    let known: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    for event in input {
        if let CoreEvent::ItemsChanged { ids } = event {
            let result = (|| {
                let library = Library::load(&paths.library.join("library.toml"), &known)?;
                sync_change(paths, store, &library, registry, &Change::Items(ids))
            })();
            if output.send(result).is_err() {
                break;
            }
        }
    }
}

/// Capture this observation when presenting Edited actions; consent is snapshot-bound.
pub fn edited_copy(
    paths: &Paths,
    store: &Store,
    project_id: i64,
    name: &str,
) -> Result<Observation, Error> {
    crate::global_skills::validate_name(name)?;
    let project = list(store)?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| Error::ConfigParse("unknown project".into()))?;
    if project.mode != ProjectMode::Copy {
        return Err(Error::ConfigChanged(
            "Edited actions require copy mode".into(),
        ));
    }
    let target = project.path.join(".agents/skills").join(name);
    let observed = engine::observe(paths, store, &[target])?.remove(0);
    let owner = observed
        .ledger
        .as_ref()
        .ok_or_else(|| Error::ForeignTarget("copy is not owned".into()))?;
    if !observed.parents_safe
        || observed.state.class != TargetClass::RealDir
        || owner.scope_id != project_id
        || owner.item.kind != ItemKind::Skill
        || owner.item.name != name
        || owner.how != "copy"
        || observed.state.hash.as_ref() == Some(&owner.written_hash)
    {
        return Err(Error::ConfigChanged("copy is not Edited".into()));
    }
    Ok(observed)
}

fn validate_edit(paths: &Paths, store: &Store, edited: &Observation) -> Result<Ownership, Error> {
    let owner = edited
        .ledger
        .as_ref()
        .ok_or_else(|| Error::ForeignTarget("copy is not owned".into()))?;
    if edited_copy(paths, store, owner.scope_id, &owner.item.name)? != *edited {
        return Err(Error::ProjectEdited("copy changed since review".into()));
    }
    Ok(owner.clone())
}

/// Keep the teammate's version by pausing this project's automatic reconciliation.
pub fn keep(paths: &Paths, store: &Store, edited: &Observation) -> Result<(), Error> {
    pause(store, validate_edit(paths, store, edited)?.scope_id)
}

/// The baseline claim and subsequent copy/lock reconciliation are separate undoable ops.
pub fn overwrite(
    paths: &Paths,
    store: &Store,
    library: &Library,
    registry: &Registry,
    edited: &Observation,
) -> Result<Vec<ApplyResult>, Error> {
    let owner = validate_edit(paths, store, edited)?;
    let claim = PlannedAction {
        action: Action::Claim {
            target: edited.target.clone(),
        },
        before: edited.clone(),
        owner: Some(owner.clone()),
        approve_modified: true,
        adoption: None,
        requires: vec![],
    };
    let result = engine::apply(
        paths,
        store,
        &Plan {
            scope_id: owner.scope_id,
            actions: vec![claim],
            ..Plan::default()
        },
        "resolve-edited",
        None,
    )?;
    let mut results = vec![result];
    if results[0].failed.is_empty() && results[0].issues.is_empty() {
        let project = list(store)?
            .into_iter()
            .find(|project| project.id == owner.scope_id)
            .ok_or_else(|| Error::ConfigChanged("project disappeared".into()))?;
        results.push(apply_project(
            paths,
            store,
            &project,
            library,
            registry,
            "resolve-edited",
            None,
        )?);
    }
    Ok(results)
}

/// Import the reviewed copy into the library. Other projects can then sync normally.
pub fn pull_into_library(
    paths: &Paths,
    store: &Store,
    edited: &Observation,
) -> Result<ApplyResult, Error> {
    let owner = validate_edit(paths, store, edited)?;
    let target = paths.library.join("skills").join(&owner.item.name);
    let before = engine::observe(paths, store, std::slice::from_ref(&target))?.remove(0);
    if !before.parents_safe || before.state.class != TargetClass::RealDir {
        return Err(Error::ForeignTarget(
            "library skill is not a safe directory".into(),
        ));
    }
    let mut action = PlannedAction {
        action: Action::CopyTree {
            target,
            from: edited.target.clone(),
            hash: edited
                .state
                .hash
                .clone()
                .ok_or_else(|| Error::ProjectEdited("copy disappeared".into()))?,
            computed_hash: None,
        },
        before,
        owner: Some(Ownership {
            scope_id: owner.scope_id,
            agent_id: None,
            item: owner.item.clone(),
            how: "copy".into(),
            written_hash: String::new(),
        }),
        approve_modified: false,
        adoption: None,
        requires: vec![edited.clone()],
    };
    action.authorize_adoption();
    let mut imported = action.before.clone();
    imported.state = edited.state.clone();
    imported.ledger = action.owner.clone();
    imported.ledger.as_mut().unwrap().written_hash = edited.state.hash.clone().unwrap();
    let claim = PlannedAction {
        action: Action::Claim {
            target: edited.target.clone(),
        },
        before: edited.clone(),
        owner: Some(owner.clone()),
        approve_modified: true,
        adoption: None,
        requires: vec![imported],
    };
    engine::apply(
        paths,
        store,
        &Plan {
            scope_id: owner.scope_id,
            actions: vec![action, claim],
            ..Plan::default()
        },
        "pull-into-library",
        None,
    )
}
