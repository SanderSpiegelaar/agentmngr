use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{OptionalExtension, params};

use super::{
    Action, ApplyResult, Observation, Ownership, Plan, PlannedAction, filesystem, issue, writable,
};
use crate::{
    Error,
    events::CoreEvent,
    paths::Paths,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass, classify_link},
};

fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Skill => "skill",
        ItemKind::Mcp => "mcp",
        ItemKind::Hook => "hook",
        ItemKind::Subagent => "subagent",
        ItemKind::Plugin => "plugin",
    }
}

fn read_owner(store: &Store, target: &Path) -> Result<Option<Ownership>, Error> {
    store.read(|connection| {
        let row = connection.query_row(
            "SELECT scope_id, agent_id, item_kind, item_name, how, written_hash FROM ledger WHERE target=?1 AND entry_key=''",
            [target.to_string_lossy().as_ref()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?)),
        ).optional()?;
        row.map(|(scope_id, agent_id, kind, name, how, written_hash)| {
            let kind = match kind.as_str() {
                "skill" => ItemKind::Skill,
                "mcp" => ItemKind::Mcp,
                "hook" => ItemKind::Hook,
                "subagent" => ItemKind::Subagent,
                "plugin" => ItemKind::Plugin,
                _ => return Err(Error::Database("unknown ledger item kind".into())),
            };
            Ok(Ownership { scope_id, agent_id, item: ItemRef { kind, name }, how, written_hash })
        }).transpose()
    })
}

/// Filesystem and ledger observations are separate from the pure planner.
pub fn observe(
    paths: &Paths,
    store: &Store,
    targets: &[PathBuf],
) -> Result<Vec<Observation>, Error> {
    targets
        .iter()
        .map(|target| {
            let parents_safe = filesystem::parents_safe(target)?;
            Ok(Observation {
                target: target.clone(),
                state: filesystem::snapshot(target, paths)?,
                ledger: read_owner(store, target)?,
                parents_safe,
            })
        })
        .collect()
}

fn json(value: &impl serde::Serialize) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|error| Error::ConfigParse(error.to_string()))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn persist(
    store: &Store,
    op_id: i64,
    target: PathBuf,
    owner: Option<Ownership>,
    inverse: &[PlannedAction],
) -> Result<(), Error> {
    let inverse_json = json(&inverse)?;
    store.write(move |connection| {
        let transaction = connection.transaction()?;
        if let Some(owner) = owner {
            transaction.execute(
                "INSERT INTO ledger(scope_id,agent_id,item_kind,item_name,target,how,written_hash,written_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(target,entry_key) DO UPDATE SET scope_id=excluded.scope_id,agent_id=excluded.agent_id,item_kind=excluded.item_kind,item_name=excluded.item_name,how=excluded.how,written_hash=excluded.written_hash,written_at=excluded.written_at",
                params![owner.scope_id,owner.agent_id,kind_name(owner.item.kind),owner.item.name,target.to_string_lossy(),owner.how,owner.written_hash,now()],
            )?;
        } else {
            transaction.execute("DELETE FROM ledger WHERE target=?1 AND entry_key=''", [target.to_string_lossy().as_ref()])?;
        }
        transaction.execute("UPDATE ops SET inverse_json=?1 WHERE id=?2", params![inverse_json,op_id])?;
        transaction.commit()?;
        Ok(())
    })
}

fn validate(paths: &Paths, planned: &PlannedAction, scope_id: i64) -> Result<(), Error> {
    let target = planned.action.target();
    if target != planned.before.target
        || !writable(&planned.before, scope_id, planned.approve_modified)
    {
        return Err(Error::ForeignTarget("target is not writable".into()));
    }
    if let Some(owner) = &planned.owner {
        if owner.scope_id != scope_id
            || !matches!(
                owner.how.as_str(),
                "link" | "copy" | "file" | "lock" | "exclude"
            )
        {
            return Err(Error::ConfigChanged("invalid action ownership".into()));
        }
    } else if !matches!(planned.action, Action::Trash { .. }) {
        // Restore may put back a previously unledgered OurLink.
        if !matches!(planned.action, Action::Restore { .. }) {
            return Err(Error::ConfigChanged(
                "created targets need ownership".into(),
            ));
        }
    }
    if let Action::Link { to, .. } = &planned.action {
        let class = classify_link(target, to, paths)?;
        if !matches!(class, TargetClass::OurLink(item) if planned.owner.as_ref().is_some_and(|owner| owner.item == item))
        {
            return Err(Error::ForeignTarget(
                "link destination is not the managed item".into(),
            ));
        }
    }
    if let Action::Restore { from, .. } = &planned.action
        // A backup can itself be a symlink; contain its entry, not its destination.
        && (!fs::canonicalize(from.parent().ok_or_else(|| Error::ForeignTarget("restore source has no parent".into()))?)?
            .join(from.file_name().ok_or_else(|| Error::ForeignTarget("restore source has no name".into()))?)
            .starts_with(fs::canonicalize(&paths.trash)?)
            || !filesystem::parents_safe(from)?)
    {
        return Err(Error::ForeignTarget(
            "restore source is outside trash".into(),
        ));
    }
    if matches!(planned.action, Action::Trash { .. }) && planned.owner.is_some() {
        return Err(Error::ConfigChanged(
            "removed target cannot retain ownership".into(),
        ));
    }
    Ok(())
}

/// Applies independent actions under the inter-process lock. Refusals are returned
/// as issues, failures separately; only completed effects enter the undo journal.
pub fn apply(
    paths: &Paths,
    store: &Store,
    plan: &Plan,
    actor: &str,
    events: Option<&Sender<CoreEvent>>,
) -> Result<ApplyResult, Error> {
    apply_checked(paths, store, plan, actor, events, false)
}

pub(super) fn apply_checked(
    paths: &Paths,
    store: &Store,
    plan: &Plan,
    actor: &str,
    events: Option<&Sender<CoreEvent>>,
    undo: bool,
) -> Result<ApplyResult, Error> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.lock_file)?;
    let mut lock = fd_lock::RwLock::new(file);
    let guard = lock.try_write().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Error::Locked("another apply holds the lock".into())
        } else {
            error.into()
        }
    })?;
    if undo {
        let mut checked = std::collections::BTreeSet::new();
        for planned in &plan.actions {
            let target = planned.action.target();
            // Repeated targets are checked against their final state, not an
            // intermediate state which an earlier inverse will restore.
            if checked.insert(target) {
                let current = observe(paths, store, &[target.to_path_buf()])?.remove(0);
                if current != planned.before {
                    return Err(Error::ConfigChanged(format!(
                        "undo target or ownership changed: {}",
                        target.display()
                    )));
                }
            }
            validate(paths, planned, plan.scope_id)?;
            if let Action::Restore { from, hash, .. } = &planned.action
                && filesystem::snapshot(from, paths)?.hash.as_ref() != Some(hash)
            {
                return Err(Error::ConfigChanged(format!(
                    "undo backup changed: {}",
                    from.display()
                )));
            }
        }
    }
    store.read(|connection| {
        connection.query_row("SELECT id FROM scopes WHERE id=?1", [plan.scope_id], |_| {
            Ok(())
        })?;
        Ok(())
    })?;
    let plan_json = json(plan)?;
    let actor = actor.to_owned();
    let summary = plan.summary.clone();
    let (op_id, timestamp) = store.write(move |connection| {
        connection.execute("INSERT INTO ops(at,actor,summary,plan_json,inverse_json,result) VALUES(?1,?2,?3,?4,'[]','applying')", params![now(),actor,summary,plan_json])?;
        let timestamp: String = connection.query_row("SELECT strftime('%Y%m%d-%H%M%S','now')", [], |row| row.get(0))?;
        Ok((connection.last_insert_rowid(), timestamp))
    })?;
    let trash = paths.trash.join(format!("{timestamp}-{op_id}"));
    fs::create_dir(&trash)?;
    let mut result = ApplyResult {
        op_id,
        done: Vec::new(),
        issues: plan.issues.clone(),
        failed: Vec::new(),
        inverse: Vec::new(),
    };
    for (index, planned) in plan.actions.iter().enumerate() {
        let target = planned.action.target().to_path_buf();
        let attempt = (|| {
            let current = observe(paths, store, std::slice::from_ref(&target))?.remove(0);
            if current != planned.before {
                return Err(Error::ConfigChanged(
                    "target or ownership changed since planning".into(),
                ));
            }
            validate(paths, planned, plan.scope_id)?;
            let backup = trash.join(index.to_string());
            let existed = current.state.class != TargetClass::Absent;
            filesystem::execute(&planned.action, &backup, existed, paths, &current.state)?;
            let state = filesystem::snapshot(&target, paths)?;
            let mut owner = planned.owner.clone();
            if let Some(owner) = &mut owner
                && !matches!(planned.action, Action::Restore { .. })
            {
                owner.written_hash = state
                    .hash
                    .clone()
                    .ok_or_else(|| Error::ConfigChanged("created target vanished".into()))?;
            }
            let inverse = if existed {
                Action::Restore {
                    target: target.clone(),
                    from: backup,
                    hash: current.state.hash.clone().unwrap(),
                }
            } else {
                Action::Trash {
                    target: target.clone(),
                }
            };
            result.inverse.insert(
                0,
                PlannedAction {
                    action: inverse,
                    before: Observation {
                        target: target.clone(),
                        state,
                        ledger: owner.clone(),
                        parents_safe: true,
                    },
                    owner: current.ledger,
                    approve_modified: false,
                },
            );
            persist(store, op_id, target.clone(), owner, &result.inverse)?;
            Ok(())
        })();
        match attempt {
            Ok(()) => result.done.push(planned.action.clone()),
            Err(error) => {
                let problem = issue(target, error.code(), error.to_string());
                if matches!(
                    error,
                    Error::ForeignTarget(_) | Error::ConfigChanged(_) | Error::ProjectEdited(_)
                ) {
                    result.issues.push(problem);
                } else {
                    result.failed.push(problem);
                }
            }
        }
    }
    let status = if result.failed.is_empty() && result.issues.is_empty() {
        "ok"
    } else {
        "issues"
    };
    store.write(move |connection| {
        connection.execute(
            "UPDATE ops SET result=?1 WHERE id=?2",
            params![status, op_id],
        )?;
        Ok(())
    })?;
    let changed = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.home.join(".changed"))?;
    changed.set_modified(SystemTime::now())?;
    changed.sync_all()?;
    drop(guard);
    if let Some(events) = events {
        let ids = result
            .inverse
            .iter()
            .filter_map(|inverse| inverse.before.ledger.as_ref().or(inverse.owner.as_ref()))
            .map(|owner| owner.item.name.clone())
            .collect();
        let _ = events.send(CoreEvent::ItemsChanged { ids });
        let _ = events.send(CoreEvent::ApplyDone);
    }
    Ok(result)
}
