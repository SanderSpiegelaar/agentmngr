use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::mpsc::Sender,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue};

use super::{Project, ProjectMode, effective_project, plan_skills_from};
use crate::{
    Error,
    content_hash::content_hash,
    engine::{self, Action, ApplyResult, Ownership, Plan, PlannedAction, TargetState},
    events::CoreEvent,
    library::{ItemSelection, Library, SkillSource},
    paths::Paths,
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass},
};

#[derive(Serialize, Deserialize)]
pub(super) struct Lock {
    pub(super) version: u32,
    pub(super) skills: BTreeMap<String, Box<RawValue>>,
    #[serde(flatten)]
    pub(super) other: BTreeMap<String, serde_json::Value>,
}

pub fn plan_project(
    paths: &Paths,
    store: &Store,
    project: &Project,
    library: &Library,
    registry: &Registry,
) -> Result<Plan, Error> {
    let selection = effective_project(store, library, project.id, registry)?;
    let mut names: BTreeSet<_> = selection.skills.iter().cloned().collect();
    names.extend(owned_skills(store, project)?);
    match project.mode {
        ProjectMode::Copy => plan_copy_project(paths, store, project, library, &selection, &names),
        ProjectMode::Link => super::plan_link_project(paths, store, project, &selection, &names),
    }
}

pub fn apply_project(
    paths: &Paths,
    store: &Store,
    project: &Project,
    library: &Library,
    registry: &Registry,
    actor: &str,
    events: Option<&Sender<CoreEvent>>,
) -> Result<ApplyResult, Error> {
    engine::apply(
        paths,
        store,
        &plan_project(paths, store, project, library, registry)?,
        actor,
        events,
    )
}

pub(super) fn owned_skills(store: &Store, project: &Project) -> Result<BTreeSet<String>, Error> {
    store.read(|connection| {
        let mut statement = connection.prepare("SELECT item_name,target FROM ledger WHERE scope_id=?1 AND item_kind='skill' AND how IN ('copy','link') AND entry_key=''")?;
        let rows = statement.query_map([project.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut names = BTreeSet::new();
        for row in rows {
            let (name, target) = row?;
            if Path::new(&target) == project.path.join(".agents/skills").join(&name) {
                names.insert(name);
            }
        }
        Ok(names)
    })
}

pub fn plan_copy_project(
    paths: &Paths,
    store: &Store,
    project: &Project,
    library: &Library,
    selection: &ItemSelection,
    names: &BTreeSet<String>,
) -> Result<Plan, Error> {
    plan_copy_from(
        paths,
        store,
        project,
        library,
        selection,
        names,
        &BTreeMap::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_copy_from(
    paths: &Paths,
    store: &Store,
    project: &Project,
    library: &Library,
    selection: &ItemSelection,
    names: &BTreeSet<String>,
    sources: &BTreeMap<String, std::path::PathBuf>,
) -> Result<Plan, Error> {
    if project.mode != ProjectMode::Copy {
        return Err(Error::ConfigChanged(
            "copy planning requires copy mode".into(),
        ));
    }
    let mut plan = plan_skills_from(paths, store, project, selection, names, sources)?;
    if !plan.issues.is_empty() {
        plan.actions.clear();
        plan.summary = format!("0 actions, {} issues", plan.issues.len());
        return Ok(plan);
    }
    let target = project.path.join("skills-lock.json");
    let before = engine::observe(paths, store, std::slice::from_ref(&target))?.remove(0);
    if !before.parents_safe
        || !matches!(
            before.state.class,
            TargetClass::Absent | TargetClass::RealFile
        )
    {
        return Err(Error::ForeignTarget(
            "skills-lock.json is not a safe regular file".into(),
        ));
    }
    let existing = match fs::read_to_string(&target) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut lock = if before.state.class == TargetClass::Absent {
        Lock {
            version: 1,
            skills: BTreeMap::new(),
            other: BTreeMap::new(),
        }
    } else {
        serde_json::from_str::<Lock>(&existing)
            .map_err(|error| Error::ConfigParse(error.to_string()))?
    };
    if lock.version != 1 {
        return Err(Error::ConfigParse(
            "expected skills-lock.json version 1".into(),
        ));
    }
    if engine::observe(paths, store, std::slice::from_ref(&target))?.remove(0) != before {
        return Err(Error::ConfigChanged(
            "skills-lock.json changed while planning".into(),
        ));
    }
    let owned = owned_skills(store, project)?;
    let mut requires = Vec::new();
    for name in names {
        if lock.skills.contains_key(name) && !owned.contains(name) {
            if selection.skills.contains(name) {
                plan.issues.push(engine::issue(
                    target.clone(),
                    "E_FOREIGN_TARGET",
                    format!("lock entry {name:?} is foreign"),
                ));
            }
            continue;
        }
        let skill_target = project.path.join(".agents/skills").join(name);
        let mut expected =
            engine::observe(paths, store, std::slice::from_ref(&skill_target))?.remove(0);
        if selection.skills.contains(name) {
            let library_source = paths.library.join("skills").join(name);
            let source = sources.get(name).unwrap_or(&library_source);
            let source_state = engine::snapshot(source, paths)?;
            let hash = content_hash(source)?;
            if engine::snapshot(source, paths)? != source_state {
                return Err(Error::ConfigChanged("skill changed while hashing".into()));
            }
            lock.skills.insert(
                name.clone(),
                to_raw(&lock_entry(
                    library,
                    name,
                    &library_source,
                    &project.path,
                    &hash,
                )?)?,
            );
            if let Some(action) = plan
                .actions
                .iter_mut()
                .find(|action| action.action.target() == skill_target)
            {
                if let Action::CopyTree { computed_hash, .. } = &mut action.action {
                    *computed_hash = Some(hash);
                }
                if let Action::CopyTree { hash, .. } = &action.action
                    && source_state.hash.as_ref() != Some(hash)
                {
                    return Err(Error::ConfigChanged("skill changed while planning".into()));
                }
                expected.state = source_state;
                expected.ledger = action.owner.clone();
                if let Some(owner) = &mut expected.ledger {
                    owner.written_hash = expected.state.hash.clone().unwrap();
                }
            } else if expected.state != source_state {
                return Err(Error::ConfigChanged("skill changed while planning".into()));
            }
        } else if owned.contains(name) {
            lock.skills.remove(name);
            expected.state = TargetState {
                class: TargetClass::Absent,
                mode: None,
                hash: None,
            };
            if plan
                .actions
                .iter()
                .any(|action| action.action.target() == skill_target)
            {
                expected.ledger = None;
            }
        }
        requires.push(expected);
    }
    if !plan.issues.is_empty() {
        plan.actions.clear();
        plan.summary = format!("0 actions, {} issues", plan.issues.len());
        return Ok(plan);
    }
    let bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(&lock)
            .map_err(|error| Error::ConfigParse(error.to_string()))?
    )
    .into_bytes();
    if bytes != existing.as_bytes()
        && (!names.is_empty() || before.state.class != TargetClass::Absent)
    {
        let mut action = PlannedAction {
            action: Action::WriteFile { target, bytes },
            before,
            owner: Some(Ownership {
                scope_id: project.id,
                agent_id: None,
                item: ItemRef {
                    kind: ItemKind::Skill,
                    name: "skills-lock.json".into(),
                },
                how: "lock".into(),
                written_hash: String::new(),
            }),
            approve_modified: false,
            adoption: None,
            requires,
        };
        // The merge preserves foreign entries, while authorization binds it to the exact input file.
        if action.before.state.class == TargetClass::RealFile {
            action.authorize_adoption();
        }
        plan.actions.push(action);
    }
    super::link::clear_exclude(paths, store, project, &mut plan)?;
    plan.summary = format!(
        "{} actions, {} issues",
        plan.actions.len(),
        plan.issues.len()
    );
    Ok(plan)
}

fn to_raw(value: &serde_json::Value) -> Result<Box<RawValue>, Error> {
    RawValue::from_string(
        serde_json::to_string_pretty(value)
            .map_err(|error| Error::ConfigParse(error.to_string()))?,
    )
    .map_err(|error| Error::ConfigParse(error.to_string()))
}

fn lock_entry(
    library: &Library,
    name: &str,
    source: &Path,
    root: &Path,
    hash: &str,
) -> Result<serde_json::Value, Error> {
    let mut entry = match library.skills.get(name).map(|skill| &skill.source) {
        Some(SkillSource::Git {
            url,
            reference,
            path,
        }) => {
            let mut entry = json!({"source": url, "sourceType": "git"});
            if let Some(reference) = reference {
                entry["ref"] = json!(reference);
            }
            if let Some(path) = path {
                entry["skillPath"] = json!(format!("{}/SKILL.md", path.trim_end_matches('/')));
            }
            entry
        }
        Some(SkillSource::Npx {
            source,
            source_type,
            url,
            reference,
            path,
        }) => {
            let mut entry = json!({"source": source, "sourceType": source_type});
            if let Some(url) = url {
                entry["sourceUrl"] = json!(url);
            }
            if let Some(reference) = reference {
                entry["ref"] = json!(reference);
            }
            if let Some(path) = path {
                entry["skillPath"] = json!(path);
            }
            entry
        }
        provenance => {
            let origin = match provenance {
                Some(SkillSource::Local {
                    origin: Some(origin),
                }) => origin.as_path(),
                _ => source,
            };
            json!({"source": relative_source(root, origin)?, "sourceType": "local"})
        }
    };
    entry["computedHash"] = json!(hash);
    Ok(entry)
}

fn relative_source(root: &Path, source: &Path) -> Result<String, Error> {
    let source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        root.join(source)
    };
    let root: Vec<_> = root.components().collect();
    let source: Vec<_> = source.components().collect();
    let common = root.iter().zip(&source).take_while(|(a, b)| a == b).count();
    let mut parts = vec!["..".to_owned(); root.len() - common];
    for component in &source[common..] {
        parts.push(
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| Error::ConfigParse("local source must be UTF-8".into()))?
                .to_owned(),
        );
    }
    let relative = parts.join("/");
    Ok(if relative.is_empty() {
        ".".into()
    } else if relative.starts_with("../") || relative == ".." {
        relative
    } else {
        format!("./{relative}")
    })
}
