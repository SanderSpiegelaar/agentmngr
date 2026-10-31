use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    time::Duration,
};

use super::{Project, ProjectMode, copy, plan_skills_from};
use crate::{
    Error,
    engine::{self, Action, Observation, Ownership, Plan, PlannedAction, TargetState},
    git::GitRunner,
    library::ItemSelection,
    paths::Paths,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass},
};

const BEGIN: &str = "# BEGIN skillsmgr managed paths\n";
const END: &str = "# END skillsmgr managed paths\n";

pub fn plan_link_project(
    paths: &Paths,
    store: &Store,
    project: &Project,
    selection: &ItemSelection,
    names: &BTreeSet<String>,
) -> Result<Plan, Error> {
    plan_link_from(paths, store, project, selection, names, &BTreeMap::new())
}

pub(super) fn plan_link_from(
    paths: &Paths,
    store: &Store,
    project: &Project,
    selection: &ItemSelection,
    names: &BTreeSet<String>,
    sources: &BTreeMap<String, PathBuf>,
) -> Result<Plan, Error> {
    if project.mode != ProjectMode::Link {
        return Err(Error::ConfigChanged(
            "link planning requires link mode".into(),
        ));
    }
    if names.iter().any(|name| name.contains(['\r', '\n'])) {
        return Err(Error::InvalidSkill(
            "link-mode skill names cannot contain line breaks".into(),
        ));
    }
    let mut plan = plan_skills_from(paths, store, project, selection, names, sources)?;
    if !plan.issues.is_empty() {
        plan.actions.clear();
        plan.summary = format!("0 actions, {} issues", plan.issues.len());
        return Ok(plan);
    }
    let owned = copy::owned_skills(store, project)?;
    let mut linked = owned.clone();
    for name in names {
        linked.remove(name);
        if selection.skills.contains(name)
            && (owned.contains(name)
                || plan.actions.iter().any(|action| {
                    matches!(&action.action, Action::Link { target, .. }
                        if target == &project.path.join(".agents/skills").join(name))
                }))
        {
            linked.insert(name.clone());
        }
    }
    let requires = skill_effects(paths, store, project, &plan)?;
    remove_lock_entries(paths, store, project, &owned, &requires, &mut plan)?;
    if !names.is_empty() || !owned.is_empty() {
        let repository = GitRunner::new(Duration::from_secs(10))?.project(&project.path)?;
        let target = repository.exclude_path()?;
        let entries = linked
            .iter()
            .map(|name| format!("/.agents/skills/{}", escape_pattern(name)))
            .collect();
        update_exclude(paths, store, project, target, &entries, requires, &mut plan)?;
    } else {
        clear_exclude(paths, store, project, &mut plan)?;
    }
    plan.summary = format!(
        "{} actions, {} issues",
        plan.actions.len(),
        plan.issues.len()
    );
    Ok(plan)
}

fn escape_pattern(name: &str) -> String {
    let mut escaped = String::new();
    for character in name.chars() {
        if matches!(character, '\\' | '*' | '?' | '[' | ']' | ' ' | '#' | '!') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn skill_effects(
    paths: &Paths,
    store: &Store,
    project: &Project,
    plan: &Plan,
) -> Result<Vec<Observation>, Error> {
    let mut targets: BTreeSet<_> = copy::owned_skills(store, project)?
        .into_iter()
        .map(|name| project.path.join(".agents/skills").join(name))
        .collect();
    targets.extend(
        plan.actions
            .iter()
            .filter(|action| {
                action.action.target().parent()
                    == Some(project.path.join(".agents/skills").as_path())
            })
            .map(|action| action.action.target().to_path_buf()),
    );
    let mut requires = engine::observe(paths, store, &targets.into_iter().collect::<Vec<_>>())?;
    for action in &plan.actions {
        if action.action.target().parent() != Some(project.path.join(".agents/skills").as_path()) {
            continue;
        }
        let state = match &action.action {
            Action::Link { to, .. } => TargetState {
                class: TargetClass::OurLink(action.owner.as_ref().unwrap().item.clone()),
                mode: None,
                hash: Some(engine::link_hash(to)),
            },
            Action::CopyTree { from, .. } => engine::snapshot(from, paths)?,
            Action::Trash { .. } => TargetState {
                class: TargetClass::Absent,
                mode: None,
                hash: None,
            },
            _ => continue,
        };
        let expected = requires
            .iter_mut()
            .find(|observed| observed.target == action.action.target())
            .unwrap();
        expected.state = state;
        expected.ledger = action.owner.clone();
        if let Some(owner) = &mut expected.ledger {
            owner.written_hash = expected.state.hash.clone().unwrap();
        }
    }
    Ok(requires)
}

fn remove_lock_entries(
    paths: &Paths,
    store: &Store,
    project: &Project,
    owned: &BTreeSet<String>,
    requires: &[Observation],
    plan: &mut Plan,
) -> Result<(), Error> {
    let target = project.path.join("skills-lock.json");
    let (before, existing) = read_regular_file(paths, store, &target)?;
    if before.state.class == TargetClass::Absent {
        return Ok(());
    }
    let ours = before
        .ledger
        .as_ref()
        .is_some_and(|owner| owner.scope_id == project.id && owner.how == "lock");
    if owned.is_empty() && !ours {
        return Ok(());
    }
    let mut lock: copy::Lock =
        serde_json::from_slice(&existing).map_err(|error| Error::ConfigParse(error.to_string()))?;
    if lock.version != 1 {
        return Err(Error::ConfigParse(
            "expected skills-lock.json version 1".into(),
        ));
    }
    let count = lock.skills.len();
    lock.skills.retain(|name, _| !owned.contains(name));
    let remove = lock.skills.is_empty() && lock.other.is_empty() && ours;
    if count == lock.skills.len() && !remove {
        return Ok(());
    }
    let action = if remove {
        Action::Trash { target }
    } else {
        Action::WriteFile {
            target,
            bytes: format!(
                "{}\n",
                serde_json::to_string_pretty(&lock)
                    .map_err(|error| Error::ConfigParse(error.to_string()))?
            )
            .into_bytes(),
        }
    };
    metadata_action(project, action, before, "lock", requires.to_vec(), plan);
    Ok(())
}

pub(super) fn clear_exclude(
    paths: &Paths,
    store: &Store,
    project: &Project,
    plan: &mut Plan,
) -> Result<(), Error> {
    let targets: Vec<PathBuf> = store.read(|connection| {
        let mut statement =
            connection.prepare("SELECT target FROM ledger WHERE scope_id=?1 AND how='exclude'")?;
        Ok(statement
            .query_map([project.id], |row| {
                row.get::<_, String>(0).map(PathBuf::from)
            })?
            .collect::<Result<_, _>>()?)
    })?;
    let requires = skill_effects(paths, store, project, plan)?;
    for target in targets {
        update_exclude(
            paths,
            store,
            project,
            target,
            &BTreeSet::new(),
            requires.clone(),
            plan,
        )?;
    }
    Ok(())
}

fn read_regular_file(
    paths: &Paths,
    store: &Store,
    target: &PathBuf,
) -> Result<(Observation, Vec<u8>), Error> {
    let before = engine::observe(paths, store, std::slice::from_ref(target))?.remove(0);
    if !before.parents_safe
        || !matches!(
            before.state.class,
            TargetClass::Absent | TargetClass::RealFile
        )
    {
        return Err(Error::ForeignTarget(format!(
            "{} is not a safe regular file",
            target.display()
        )));
    }
    let bytes = if before.state.class == TargetClass::Absent {
        Vec::new()
    } else {
        fs::read(target)?
    };
    if engine::observe(paths, store, std::slice::from_ref(target))?.remove(0) != before {
        return Err(Error::ConfigChanged(format!(
            "{} changed while planning",
            target.display()
        )));
    }
    Ok((before, bytes))
}

fn update_exclude(
    paths: &Paths,
    store: &Store,
    project: &Project,
    target: PathBuf,
    entries: &BTreeSet<String>,
    requires: Vec<Observation>,
    plan: &mut Plan,
) -> Result<(), Error> {
    let (before, existing) = read_regular_file(paths, store, &target)?;
    let mut remaining = Vec::new();
    let mut inside = false;
    let mut seen = false;
    for line in existing.split_inclusive(|byte| *byte == b'\n') {
        if line == BEGIN.as_bytes() {
            if inside || seen {
                return Err(Error::ConfigParse("duplicate managed exclude block".into()));
            }
            inside = true;
            seen = true;
        } else if line == END.as_bytes() {
            if !inside {
                return Err(Error::ConfigParse(
                    "unmatched managed exclude marker".into(),
                ));
            }
            inside = false;
        } else if !inside {
            remaining.extend_from_slice(line);
        }
    }
    if inside {
        return Err(Error::ConfigParse(
            "unterminated managed exclude block".into(),
        ));
    }
    let mut bytes = Vec::new();
    if !entries.is_empty() {
        bytes.extend_from_slice(BEGIN.as_bytes());
        for entry in entries {
            bytes.extend_from_slice(entry.as_bytes());
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(END.as_bytes());
    }
    bytes.extend_from_slice(&remaining);
    if bytes != existing {
        metadata_action(
            project,
            Action::WriteFile { target, bytes },
            before,
            "exclude",
            requires,
            plan,
        );
    }
    Ok(())
}

fn metadata_action(
    project: &Project,
    action: Action,
    before: Observation,
    how: &str,
    requires: Vec<Observation>,
    plan: &mut Plan,
) {
    let owner = (!matches!(action, Action::Trash { .. })).then(|| Ownership {
        scope_id: project.id,
        agent_id: None,
        item: ItemRef {
            kind: ItemKind::Skill,
            name: action
                .target()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        },
        how: how.into(),
        written_hash: String::new(),
    });
    let mut action = PlannedAction {
        action,
        before,
        owner,
        approve_modified: false,
        adoption: None,
        requires,
    };
    if action.before.state.class == TargetClass::RealFile {
        action.authorize_adoption();
    }
    plan.actions.push(action);
}
