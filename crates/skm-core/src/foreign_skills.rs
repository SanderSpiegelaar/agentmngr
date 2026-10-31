//! Read-only discovery and explicit adoption through the guarded engine journal.
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::OptionalExtension;

use crate::{
    Error,
    content_hash::content_hash,
    engine::{self, Action, ApplyResult, Observation, Ownership, Plan, PlannedAction, TargetState},
    global_skills::validate_name,
    library::{Library, SkillProvenance, SkillSource},
    paths::Paths,
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass},
};

mod locks;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignSkill {
    pub name: String,
    pub target: PathBuf,
    pub class: TargetClass,
    pub provenance: Option<SkillProvenance>,
}

/// Never creates target directories, scopes, ledger rows, or library metadata.
pub fn list_foreign(
    paths: &Paths,
    store: &Store,
    user_home: &Path,
    project: Option<&Path>,
) -> Result<Vec<ForeignSkill>, Error> {
    let root = fs::canonicalize(project.unwrap_or(user_home))?;
    let provenance = locks::read(&root, project.is_some())?;
    let entries = match fs::read_dir(root.join(".agents/skills")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::InvalidSkill("skill name is not UTF-8".into()))?;
        let current = engine::observe(paths, store, &[entry.path()])?.remove(0);
        if matches!(
            current.state.class,
            TargetClass::OurLink(_) | TargetClass::Absent
        ) || (project.is_some()
            && current
                .ledger
                .as_ref()
                .is_some_and(|owner| current.state.hash.as_ref() == Some(&owner.written_hash)))
        {
            continue;
        }
        result.push(ForeignSkill {
            provenance: provenance.get(&name).cloned(),
            name,
            target: entry.path(),
            class: current.state.class,
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

fn scope(store: &Store, project: Option<&Path>, create: bool) -> Result<i64, Error> {
    let path = project.map(|path| path.to_string_lossy().into_owned());
    let kind = if project.is_some() {
        "project"
    } else {
        "global"
    };
    let found = store.read(|connection| {
        Ok(connection
            .query_row(
                "SELECT id,mode FROM scopes WHERE kind=?1 AND path IS ?2 ORDER BY id LIMIT 1",
                (kind, &path),
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?)
    })?;
    if let Some((id, mode)) = found {
        if project.is_some() && mode != "copy" {
            return Err(Error::ConfigChanged(
                "project adoption requires copy mode".into(),
            ));
        }
        return Ok(id);
    }
    if !create {
        return Ok(0);
    }
    store.write(move |connection| {
        connection.execute(
            "INSERT INTO scopes(kind,path,name,mode,added_at) SELECT ?1,?2,?3,?4,unixepoch() WHERE NOT EXISTS (SELECT 1 FROM scopes WHERE kind=?1 AND path IS ?2)",
            (
                kind,
                &path,
                path.as_deref().unwrap_or("Global"),
                if kind == "project" { "copy" } else { "link" },
            ),
        )?;
        Ok(connection.query_row("SELECT id FROM scopes WHERE kind=?1 AND path IS ?2 ORDER BY id LIMIT 1", (kind, &path), |row| row.get(0))?)
    })
}

fn action(
    action: Action,
    before: Observation,
    owner: Option<Ownership>,
    requires: Vec<Observation>,
    adopt: bool,
) -> PlannedAction {
    let mut planned = PlannedAction {
        action,
        before,
        owner,
        requires,
        approve_modified: false,
        adoption: None,
    };
    if adopt {
        planned.authorize_adoption();
    }
    planned
}

fn after(planned: &PlannedAction, state: TargetState) -> Observation {
    let mut ledger = planned.owner.clone();
    if let Some(owner) = &mut ledger {
        owner.written_hash = state.hash.clone().unwrap();
    }
    Observation {
        target: planned.action.target().to_path_buf(),
        state,
        ledger,
        parents_safe: true,
    }
}

/// The preview and apply use identical effects. A scope id of 0 is preview-only.
pub fn plan_adopt(
    paths: &Paths,
    store: &Store,
    user_home: &Path,
    names: &[String],
    project: Option<&Path>,
) -> Result<Plan, Error> {
    let root = fs::canonicalize(project.unwrap_or(user_home))?;
    let project = project.map(|_| root.as_path());
    let scope_id = scope(store, project, false)?;
    let provenance = locks::read(&root, project.is_some())?;
    let registry = Registry::load()?;
    let known_agents: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    let manifest = paths.library.join("library.toml");
    let manifest_before = engine::observe(paths, store, std::slice::from_ref(&manifest))?.remove(0);
    if !matches!(
        manifest_before.state.class,
        TargetClass::Absent | TargetClass::RealFile
    ) || !manifest_before.parents_safe
    {
        return Err(Error::ForeignTarget(
            "library.toml is not a safe regular file".into(),
        ));
    }
    let existing = match fs::read_to_string(&manifest) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut library = if existing.is_empty() {
        Library::default()
    } else {
        Library::parse(&existing, &known_agents)?
    };
    if engine::observe(paths, store, std::slice::from_ref(&manifest))?.remove(0) != manifest_before
    {
        return Err(Error::ConfigChanged(
            "library.toml changed while planning adoption".into(),
        ));
    }
    let mut plan = Plan {
        scope_id,
        ..Plan::default()
    };
    let mut prerequisites = Vec::new();
    for name in names.iter().collect::<BTreeSet<_>>() {
        validate_name(name)?;
        let target = root.join(".agents/skills").join(name);
        let before = engine::observe(paths, store, std::slice::from_ref(&target))?.remove(0);
        if matches!(
            before.state.class,
            TargetClass::OurLink(_) | TargetClass::Absent
        ) || !before.parents_safe
        {
            plan.issues.push(engine::issue(
                target,
                "E_FOREIGN_TARGET",
                "target is not an adoptable foreign skill",
            ));
            continue;
        }
        if project.is_some() && before.state.class != TargetClass::RealDir {
            plan.issues.push(engine::issue(
                target,
                "E_INVALID_SKILL",
                "project copy adoption requires a real directory",
            ));
            continue;
        }
        let source = match fs::canonicalize(&target) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                plan.issues.push(engine::issue(
                    target,
                    "E_INVALID_SKILL",
                    "skill link destination is missing",
                ));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if !source.is_dir() {
            plan.issues.push(engine::issue(
                target,
                "E_INVALID_SKILL",
                "adoption requires a skill directory",
            ));
            continue;
        }
        let source_before = engine::observe(paths, store, std::slice::from_ref(&source))?.remove(0);
        let hash = source_before.state.hash.clone().unwrap();
        let computed_hash = content_hash(&source)?;
        let destination = paths.library.join("skills").join(name);
        let mut imported =
            engine::observe(paths, store, std::slice::from_ref(&destination))?.remove(0);
        if imported.state.class != TargetClass::Absent
            && (imported.state.class != TargetClass::RealDir
                || imported.state.hash.as_ref() != Some(&hash))
        {
            plan.issues.push(engine::issue(
                destination,
                "E_NAME_COLLISION",
                "library item has different content",
            ));
            continue;
        }
        if !imported.parents_safe || !source_before.parents_safe {
            plan.issues.push(engine::issue(
                target,
                "E_FOREIGN_TARGET",
                "adoption path has unsafe ancestors",
            ));
            continue;
        }
        let owner = |how: &str| {
            Some(Ownership {
                scope_id,
                agent_id: None,
                item: ItemRef {
                    kind: ItemKind::Skill,
                    name: name.clone(),
                },
                how: how.into(),
                written_hash: String::new(),
            })
        };
        if imported.state.class == TargetClass::Absent {
            let copy = action(
                Action::CopyTree {
                    target: destination.clone(),
                    from: source.clone(),
                    hash: hash.clone(),
                    computed_hash: Some(computed_hash.clone()),
                },
                imported,
                owner("copy"),
                vec![before.clone(), source_before.clone()],
                false,
            );
            imported = after(
                &copy,
                TargetState {
                    class: TargetClass::RealDir,
                    hash: Some(hash),
                },
            );
            plan.actions.push(copy);
        } else if content_hash(&destination)? != computed_hash {
            plan.issues.push(engine::issue(
                destination,
                "E_NAME_COLLISION",
                "library content hash differs",
            ));
            continue;
        }
        let effect = if project.is_some() {
            Action::Claim {
                target: target.clone(),
            }
        } else {
            Action::Link {
                target: target.clone(),
                to: destination.clone(),
            }
        };
        let adoption = action(
            effect,
            before.clone(),
            owner(if project.is_some() { "copy" } else { "link" }),
            vec![imported.clone(), source_before],
            true,
        );
        let state = if project.is_some() {
            before.state
        } else {
            TargetState {
                class: TargetClass::OurLink(ItemRef {
                    kind: ItemKind::Skill,
                    name: name.clone(),
                }),
                hash: Some(engine::link_hash(&destination)),
            }
        };
        prerequisites.push(after(&adoption, state));
        prerequisites.push(imported);
        plan.actions.push(adoption);
        library.skills.entry(name.clone()).or_insert_with(|| {
            let mut entry = provenance.get(name).cloned().unwrap_or(SkillProvenance {
                source: SkillSource::Local {
                    origin: Some(target),
                },
                rev: None,
                base_hash: None,
                installed_at: None,
            });
            entry.base_hash = Some(computed_hash);
            entry
        });
        if project.is_none() && !library.global.skills.contains(name) {
            library.global.skills.push(name.clone());
        }
    }
    if !prerequisites.is_empty() {
        let bytes = library.render(&existing, &known_agents)?.into_bytes();
        if bytes != existing.as_bytes() {
            let before = manifest_before;
            let adopt = before.state.class != TargetClass::Absent;
            let owner = Some(Ownership {
                scope_id,
                agent_id: None,
                item: ItemRef {
                    kind: ItemKind::Skill,
                    name: "library.toml".into(),
                },
                how: "file".into(),
                written_hash: String::new(),
            });
            plan.actions.push(action(
                Action::WriteFile {
                    target: manifest,
                    bytes,
                },
                before,
                owner,
                prerequisites,
                adopt,
            ));
        }
    }
    plan.summary = format!(
        "{} actions, {} issues",
        plan.actions.len(),
        plan.issues.len()
    );
    Ok(plan)
}

/// Copies, verifies, replaces/claims, and records metadata in one engine operation.
pub fn adopt(
    paths: &Paths,
    store: &Store,
    user_home: &Path,
    names: &[String],
    project: Option<&Path>,
) -> Result<ApplyResult, Error> {
    let project = project.map(fs::canonicalize).transpose()?;
    // Validate a preview before creating machine-local scope state.
    let preview = plan_adopt(paths, store, user_home, names, project.as_deref())?;
    let mut plan = preview;
    if plan.scope_id == 0 {
        plan.scope_id = scope(store, project.as_deref(), true)?;
        for action in &mut plan.actions {
            if let Some(owner) = &mut action.owner {
                owner.scope_id = plan.scope_id;
            }
            for required in &mut action.requires {
                if let Some(owner) = &mut required.ledger
                    && owner.scope_id == 0
                {
                    owner.scope_id = plan.scope_id;
                }
            }
        }
    }
    engine::apply(paths, store, &plan, "adopt", None)
}
