//! Global desired skills and their universal links. Agent-specific paths are never used.
use std::path::Path;

use rusqlite::OptionalExtension;

use crate::{
    Error,
    engine::{self, ApplyResult, DesiredContent, DesiredTarget, Plan, PlanIssue},
    library::Library,
    paths::Paths,
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass},
};

/// Preview the same reconciliation as `set_global_skill`, without saving or applying.
/// A first-run preview uses scope id 0 until a real apply creates the global scope.
pub fn plan_global_skill(
    paths: &Paths,
    store: &Store,
    user_home: &Path,
    name: &str,
    on: bool,
) -> Result<Plan, Error> {
    let scope_id = store.read(|connection| {
        Ok(connection
            .query_row(
                "SELECT id FROM scopes WHERE kind='global' ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    })?;
    Ok(prepare(paths, store, user_home, name, on, scope_id)?.1)
}

/// Save desired state through the library module, then apply through the engine.
/// Refusals retain desired state and are returned as issues, never silent success.
pub fn set_global_skill(
    paths: &Paths,
    store: &Store,
    user_home: &Path,
    name: &str,
    on: bool,
) -> Result<ApplyResult, Error> {
    let scope_id = store.write(|connection| {
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO scopes(kind,name,mode,added_at) SELECT 'global','Global','link',unixepoch() WHERE NOT EXISTS (SELECT 1 FROM scopes WHERE kind='global')",
            [],
        )?;
        let id = transaction.query_row("SELECT id FROM scopes WHERE kind='global' ORDER BY id LIMIT 1", [], |row| row.get(0))?;
        transaction.commit()?;
        Ok(id)
    })?;
    let (library, plan) = prepare(paths, store, user_home, name, on, scope_id)?;
    let registry = Registry::load()?;
    let known_agents: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    library.save(&paths.library.join("library.toml"), &known_agents)?;
    engine::apply(paths, store, &plan, "global-skills", None)
}

fn prepare(
    paths: &Paths,
    store: &Store,
    user_home: &Path,
    name: &str,
    on: bool,
    scope_id: i64,
) -> Result<(Library, Plan), Error> {
    validate_name(name)?;
    let registry = Registry::load()?;
    let known_agents: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    let mut library = match Library::load(&paths.library.join("library.toml"), &known_agents) {
        Ok(library) => library,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Library::default(),
        Err(error) => return Err(error),
    };
    library.global.skills.retain(|skill| skill != name);
    if on {
        library.global.skills.push(name.to_owned());
    }
    let directory = Paths::global_skills(user_home)?;
    let source = paths.library.join("skills").join(name);
    if on && !source.is_dir() {
        return Err(Error::InvalidSkill(format!(
            "skill {name:?} is not in the library"
        )));
    }
    let mut desired = vec![DesiredTarget {
        target: directory.join(name),
        item: ItemRef {
            kind: ItemKind::Skill,
            name: name.into(),
        },
        agent_id: None,
        content: if on {
            DesiredContent::Link { to: source }
        } else {
            DesiredContent::Absent
        },
        approve_modified: false,
    }];
    let targets: Vec<_> = desired.iter().map(|target| target.target.clone()).collect();
    let observed = engine::observe(paths, store, &targets)?;
    // Even ledger-owned real copies must not be removed by a global link toggle.
    let mut issues = Vec::new();
    desired.retain(|desired| {
        let current = observed
            .iter()
            .find(|current| current.target == desired.target)
            .unwrap();
        if matches!(
            current.state.class,
            TargetClass::Absent | TargetClass::OurLink(_)
        ) {
            true
        } else {
            issues.push(PlanIssue {
                target: desired.target.clone(),
                code: "E_FOREIGN_TARGET".into(),
                message: "global skills toggles only change managed links".into(),
            });
            false
        }
    });
    let mut plan = engine::plan(scope_id, &desired, &observed);
    plan.issues.extend(issues);
    plan.summary = format!(
        "{} actions, {} issues",
        plan.actions.len(),
        plan.issues.len()
    );
    Ok((library, plan))
}

pub fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(Error::InvalidSkill(
            "skill name must be one directory name".into(),
        ));
    }
    Ok(())
}
