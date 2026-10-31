use std::{collections::BTreeMap, fs, path::Path};

use super::{Project, ProjectMode, register_scope};
use crate::{
    Error,
    engine::{self, Action, Ownership, Plan, PlannedAction},
    foreign_skills::locks,
    library::Library,
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSkillState {
    Attached,
    Edited,
    Foreign,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReattachResult {
    pub skills: BTreeMap<String, ProjectSkillState>,
    pub suggested_sets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub project: Project,
    pub reattachment: ReattachResult,
}

pub fn register_with_report(store: &Store, path: &Path) -> Result<Registration, Error> {
    let (mut registration, mut plan) = plan_registration(store, path)?;
    registration.project = register_scope(store, path)?;
    plan.scope_id = registration.project.id;
    for action in &mut plan.actions {
        if let Some(owner) = &mut action.owner {
            owner.scope_id = plan.scope_id;
        }
    }
    if !plan.actions.is_empty() {
        let applied = engine::apply(&store.paths, store, &plan, "reattach", None)?;
        if !applied.issues.is_empty() || !applied.failed.is_empty() {
            return Err(Error::ConfigChanged(
                "project content changed while reattaching".into(),
            ));
        }
    }
    Ok(registration)
}

pub fn plan_registration(store: &Store, path: &Path) -> Result<(Registration, Plan), Error> {
    let root = path.canonicalize()?;
    let locked = locks::read(&root, true)?;
    let registry = Registry::load()?;
    let known: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    let library = match fs::read_to_string(store.paths.library.join("library.toml")) {
        Ok(text) => Library::parse(&text, &known)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Library::default(),
        Err(error) => return Err(error.into()),
    };
    let project = super::list(store)?
        .into_iter()
        .find(|p| p.path == root)
        .unwrap_or(Project {
            id: 0,
            path: root.clone(),
            name: root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            mode: ProjectMode::Copy,
            auto_sync: true,
        });
    let mut plan = Plan {
        scope_id: project.id,
        summary: "Reattach project".into(),
        ..Plan::default()
    };
    let mut report = ReattachResult {
        suggested_sets: library
            .sets
            .iter()
            .filter(|(_, set)| {
                !set.items.skills.is_empty()
                    && set
                        .items
                        .skills
                        .iter()
                        .all(|name| locked.contains_key(name))
            })
            .map(|(name, _)| name.clone())
            .collect(),
        ..ReattachResult::default()
    };
    if project.mode != ProjectMode::Copy {
        return Ok((
            Registration {
                project,
                reattachment: report,
            },
            plan,
        ));
    }
    let entries = match fs::read_dir(root.join(".agents/skills")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((
                Registration {
                    project,
                    reattachment: report,
                },
                plan,
            ));
        }
        Err(error) => return Err(error.into()),
    };
    let paths = &store.paths;
    for entry in entries {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::InvalidSkill("skill name is not UTF-8".into()))?;
        let before = engine::observe(paths, store, &[entry.path()])?.remove(0);
        let source = paths.library.join("skills").join(&name);
        let library_item = engine::observe(paths, store, &[source])?.remove(0);
        let state = if let Some(owner) = &before.ledger {
            if before.state.hash.as_ref() == Some(&owner.written_hash) {
                ProjectSkillState::Attached
            } else {
                ProjectSkillState::Edited
            }
        } else if before.parents_safe
            && library_item.parents_safe
            && before.state.class == TargetClass::RealDir
            && library_item.state.class == TargetClass::RealDir
            && before.state.hash == library_item.state.hash
        {
            let mut claim = PlannedAction {
                action: Action::Claim {
                    target: entry.path(),
                },
                owner: Some(Ownership {
                    scope_id: project.id,
                    agent_id: None,
                    item: ItemRef {
                        kind: ItemKind::Skill,
                        name: name.clone(),
                    },
                    how: "copy".into(),
                    written_hash: String::new(),
                }),
                before,
                requires: vec![library_item],
                approve_modified: false,
                adoption: None,
            };
            claim.authorize_adoption();
            plan.actions.push(claim);
            ProjectSkillState::Attached
        } else if library_item.state.class == TargetClass::RealDir {
            ProjectSkillState::Edited
        } else {
            ProjectSkillState::Foreign
        };
        report.skills.insert(name, state);
    }
    Ok((
        Registration {
            project,
            reattachment: report,
        },
        plan,
    ))
}
