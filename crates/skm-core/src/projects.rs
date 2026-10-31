//! Machine-local projects and effective selections. Planning never writes to a project.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{
    Error,
    engine::{self, DesiredContent, DesiredTarget, Plan},
    library::{ItemSelection, Library},
    paths::Paths,
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef},
};

mod copy;
mod link;
pub mod sync;
pub use copy::{apply_project, plan_copy_project, plan_project};
pub use link::plan_link_project;
mod reattach;
pub use reattach::{ProjectSkillState, ReattachResult, Registration, register_with_report};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectMode {
    #[default]
    Copy,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub path: PathBuf,
    pub name: String,
    pub mode: ProjectMode,
    pub auto_sync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPlan {
    pub project: Project,
    pub plan: Plan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectItem {
    pub item: ItemRef,
    pub agents: Option<Vec<String>>,
}

pub fn register(store: &Store, path: &Path) -> Result<Project, Error> {
    Ok(register_with_report(store, path)?.project)
}

fn register_scope(store: &Store, path: &Path) -> Result<Project, Error> {
    let path = path.canonicalize()?;
    if !path.is_dir() {
        return Err(Error::ConfigParse(
            "project path must be a directory".into(),
        ));
    }
    let path = path
        .to_str()
        .ok_or_else(|| Error::ConfigParse("project path must be UTF-8".into()))?
        .to_owned();
    let name = Path::new(&path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let stored_path = path.clone();
    store.write(move |connection| {
        connection.execute("INSERT INTO scopes(kind,path,name,added_at) VALUES ('project',?1,?2,unixepoch()) ON CONFLICT(path) DO NOTHING", params![stored_path,name])?;
        Ok(())
    })?;
    list(store)?
        .into_iter()
        .find(|project| project.path == path)
        .ok_or_else(|| Error::Database("registered project disappeared".into()))
}

/// Forget registration and ownership, leaving every repository file untouched.
pub fn unregister(store: &Store, project_id: i64) -> Result<(), Error> {
    require_project(store, project_id)?;
    store.write(move |connection| {
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM ledger WHERE scope_id=?1", [project_id])?;
        transaction.execute(
            "DELETE FROM foreign_entries WHERE scope_id=?1",
            [project_id],
        )?;
        transaction.execute(
            "DELETE FROM scopes WHERE id=?1 AND kind='project'",
            [project_id],
        )?;
        transaction.commit()?;
        Ok(())
    })
}

pub fn list(store: &Store) -> Result<Vec<Project>, Error> {
    store.read(|connection| {
        let mut statement = connection.prepare(
            "SELECT id,path,name,mode,auto_sync FROM scopes WHERE kind='project' ORDER BY path",
        )?;
        let rows = statement.query_map([], |row| {
            let mode: String = row.get(3)?;
            Ok(Project {
                id: row.get(0)?,
                path: row.get::<_, String>(1)?.into(),
                name: row.get(2)?,
                mode: if mode == "link" {
                    ProjectMode::Link
                } else {
                    ProjectMode::Copy
                },
                auto_sync: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    })
}

pub fn set_mode(store: &Store, project_id: i64, mode: ProjectMode) -> Result<(), Error> {
    require_project(store, project_id)?;
    store.write(move |connection| {
        connection.execute(
            "UPDATE scopes SET mode=?2 WHERE id=?1 AND kind='project'",
            params![
                project_id,
                if mode == ProjectMode::Copy {
                    "copy"
                } else {
                    "link"
                }
            ],
        )?;
        Ok(())
    })
}

pub fn set_auto_sync(store: &Store, project_id: i64, on: bool) -> Result<(), Error> {
    require_project(store, project_id)?;
    store.write(move |connection| {
        connection.execute(
            "UPDATE scopes SET auto_sync=?2 WHERE id=?1 AND kind='project'",
            params![project_id, on],
        )?;
        Ok(())
    })
}

pub fn use_set(store: &Store, library: &Library, project_id: i64, name: &str) -> Result<(), Error> {
    require_project(store, project_id)?;
    if !library.sets.contains_key(name) {
        return Err(Error::ConfigParse(format!("unknown set {name:?}")));
    }
    let name = name.to_owned();
    store.write(move |connection| {
        connection.execute(
            "INSERT OR IGNORE INTO project_sets(scope_id,set_name) VALUES (?1,?2)",
            params![project_id, name],
        )?;
        Ok(())
    })
}

pub fn unuse_set(store: &Store, project_id: i64, name: &str) -> Result<(), Error> {
    require_project(store, project_id)?;
    let name = name.to_owned();
    store.write(move |connection| {
        connection.execute(
            "DELETE FROM project_sets WHERE scope_id=?1 AND set_name=?2",
            params![project_id, name],
        )?;
        Ok(())
    })
}

pub fn used_sets(store: &Store, project_id: i64) -> Result<Vec<String>, Error> {
    require_project(store, project_id)?;
    store.read(|connection| {
        let mut statement = connection
            .prepare("SELECT set_name FROM project_sets WHERE scope_id=?1 ORDER BY set_name")?;
        Ok(statement
            .query_map([project_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?)
    })
}

pub fn set_direct_item(
    store: &Store,
    registry: &Registry,
    project_id: i64,
    item: &ItemRef,
    agents: Option<&[String]>,
) -> Result<(), Error> {
    require_project(store, project_id)?;
    let mut selection = ItemSelection::default();
    selection.set_item(item, agents)?;
    let known: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    Library {
        global: selection,
        ..Library::default()
    }
    .validate(&known)?;
    let item = item.clone();
    let agents = agents
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| Error::ConfigParse(error.to_string()))?;
    store.write(move |connection| {
        connection.execute("INSERT INTO project_items(scope_id,item_kind,item_name,agents_json) VALUES (?1,?2,?3,?4) ON CONFLICT(scope_id,item_kind,item_name) DO UPDATE SET agents_json=excluded.agents_json", params![project_id,kind_name(item.kind)?,item.name,agents])?;
        Ok(())
    })
}

pub fn remove_direct_item(store: &Store, project_id: i64, item: &ItemRef) -> Result<(), Error> {
    require_project(store, project_id)?;
    let item = item.clone();
    store.write(move |connection| {
        connection.execute(
            "DELETE FROM project_items WHERE scope_id=?1 AND item_kind=?2 AND item_name=?3",
            params![project_id, kind_name(item.kind)?, item.name],
        )?;
        Ok(())
    })
}

pub fn direct_items(store: &Store, project_id: i64) -> Result<Vec<DirectItem>, Error> {
    require_project(store, project_id)?;
    store.read(|connection| {
        let mut statement = connection.prepare("SELECT item_kind,item_name,agents_json FROM project_items WHERE scope_id=?1 ORDER BY item_kind,item_name")?;
        let rows = statement.query_map([project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (kind, name, agents) = row?;
            let kind = match kind.as_str() {
                "skill" => ItemKind::Skill,
                "mcp" => ItemKind::Mcp,
                "hook" => ItemKind::Hook,
                "subagent" => ItemKind::Subagent,
                _ => return Err(Error::Database(format!("invalid project kind {kind}"))),
            };
            let agents = agents
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .map_err(|error| Error::ConfigParse(error.to_string()))?;
            items.push(DirectItem { item: ItemRef { kind, name }, agents });
        }
        Ok(items)
    })
}

/// Explicit direct agent lists override set membership, then Global agents are subtracted.
pub fn effective(
    library: &Library,
    sets: &[String],
    direct: &ItemSelection,
    registry: &Registry,
    enabled: &[String],
) -> Result<ItemSelection, Error> {
    let mut result = ItemSelection::default();
    for name in sets {
        // A synced set deletion may leave local links until the user removes them.
        if let Some(set) = library.sets.get(name) {
            result.skills.extend(set.items.skills.iter().cloned());
            for kind in [ItemKind::Mcp, ItemKind::Hook, ItemKind::Subagent] {
                for (name, agents) in set.items.entries(kind).unwrap() {
                    result
                        .entries_mut(kind)
                        .unwrap()
                        .entry(name.clone())
                        .or_default()
                        .extend(agents.iter().cloned());
                }
            }
        }
    }
    result.skills.extend(direct.skills.iter().cloned());
    result
        .skills
        .retain(|name| !library.global.skills.contains(name));
    result.skills.sort();
    result.skills.dedup();
    for kind in [ItemKind::Mcp, ItemKind::Hook, ItemKind::Subagent] {
        let entries = result.entries_mut(kind).unwrap();
        for (name, agents) in direct.entries(kind).unwrap() {
            entries.insert(name.clone(), agents.clone());
        }
        for (name, agents) in entries.iter_mut() {
            let mut expanded = expand_agents(agents, kind, registry, enabled)?;
            if let Some(global) = library.global.entries(kind).unwrap().get(name) {
                let global = expand_agents(global, kind, registry, enabled)?;
                expanded.retain(|agent| !global.contains(agent));
            }
            *agents = expanded;
        }
        entries.retain(|_, agents| !agents.is_empty());
    }
    Ok(result)
}

pub fn effective_project(
    store: &Store,
    library: &Library,
    project_id: i64,
    registry: &Registry,
) -> Result<ItemSelection, Error> {
    let sets = used_sets(store, project_id)?;
    let mut direct = ItemSelection::default();
    for entry in direct_items(store, project_id)? {
        // NULL inherits set membership; an explicit wildcard overrides it.
        let inherits = entry.agents.is_none()
            && entry.item.kind != ItemKind::Skill
            && sets
                .iter()
                .filter_map(|name| library.sets.get(name))
                .any(|set| {
                    set.items
                        .entries(entry.item.kind)
                        .unwrap()
                        .contains_key(&entry.item.name)
                });
        if !inherits {
            direct.set_item(&entry.item, entry.agents.as_deref())?;
        }
    }
    effective(library, &sets, &direct, registry, &enabled_agents(store)?)
}

pub fn projects_using_set(store: &Store, name: &str) -> Result<Vec<Project>, Error> {
    let ids: BTreeSet<i64> = store.read(|connection| {
        let mut statement =
            connection.prepare("SELECT scope_id FROM project_sets WHERE set_name=?1")?;
        Ok(statement
            .query_map([name], |row| row.get(0))?
            .collect::<Result<_, _>>()?)
    })?;
    Ok(list(store)?
        .into_iter()
        .filter(|project| ids.contains(&project.id))
        .collect())
}

/// Includes paused projects for previews. Callers decide whether to auto-apply.
pub fn plan_set_change(
    paths: &Paths,
    store: &Store,
    before: &Library,
    after: &Library,
    name: &str,
    registry: &Registry,
) -> Result<Vec<ProjectPlan>, Error> {
    let names: BTreeSet<_> = [before, after]
        .into_iter()
        .filter_map(|library| library.sets.get(name))
        .flat_map(|set| set.items.skills.iter().cloned())
        .collect();
    projects_using_set(store, name)?
        .into_iter()
        .map(|project| {
            let selection = effective_project(store, after, project.id, registry)?;
            let plan = if project.mode == ProjectMode::Copy {
                plan_copy_project(paths, store, &project, after, &selection, &names)?
            } else {
                plan_link_project(paths, store, &project, &selection, &names)?
            };
            Ok(ProjectPlan { project, plan })
        })
        .collect()
}

/// Only supplied skill names are reconciled, including removals; no lock or exclude writes.
pub fn plan_skills(
    paths: &Paths,
    store: &Store,
    project: &Project,
    selection: &ItemSelection,
    names: &BTreeSet<String>,
) -> Result<Plan, Error> {
    require_project(store, project.id)?;
    let mut desired = Vec::new();
    for name in names {
        crate::global_skills::validate_name(name)?;
        let source = paths.library.join("skills").join(name);
        let content = if !selection.skills.contains(name) {
            DesiredContent::Absent
        } else {
            if !source.is_dir() {
                return Err(Error::InvalidSkill(format!(
                    "skill {name:?} is not in the library"
                )));
            }
            match project.mode {
                ProjectMode::Copy => DesiredContent::Tree {
                    hash: engine::snapshot(&source, paths)?.hash.ok_or_else(|| {
                        Error::InvalidSkill(format!("skill {name:?} disappeared while planning"))
                    })?,
                    from: source,
                },
                ProjectMode::Link => DesiredContent::Link { to: source },
            }
        };
        desired.push(DesiredTarget {
            target: project.path.join(".agents/skills").join(name),
            item: ItemRef {
                kind: ItemKind::Skill,
                name: name.clone(),
            },
            agent_id: None,
            content,
            approve_modified: false,
        });
    }
    let targets: Vec<_> = desired.iter().map(|target| target.target.clone()).collect();
    let observed = engine::observe(paths, store, &targets)?;
    Ok(engine::plan(project.id, &desired, &observed))
}

fn enabled_agents(store: &Store) -> Result<Vec<String>, Error> {
    store.read(|connection| {
        let mut statement =
            connection.prepare("SELECT id FROM agents WHERE enabled=1 ORDER BY id")?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?)
    })
}

fn require_project(store: &Store, id: i64) -> Result<(), Error> {
    store.read(|connection| {
        let exists = connection
            .query_row(
                "SELECT 1 FROM scopes WHERE id=?1 AND kind='project'",
                [id],
                |_| Ok(()),
            )
            .optional()?;
        exists.ok_or_else(|| Error::ConfigParse(format!("unknown project {id}")))
    })
}

fn kind_name(kind: ItemKind) -> Result<&'static str, Error> {
    match kind {
        ItemKind::Skill => Ok("skill"),
        ItemKind::Mcp => Ok("mcp"),
        ItemKind::Hook => Ok("hook"),
        ItemKind::Subagent => Ok("subagent"),
        ItemKind::Plugin => Err(Error::ConfigParse("plugins are global-only".into())),
    }
}

fn expand_agents(
    agents: &[String],
    kind: ItemKind,
    registry: &Registry,
    enabled: &[String],
) -> Result<Vec<String>, Error> {
    for id in agents {
        if id != "*" && !registry.agents.contains_key(id) {
            return Err(Error::ConfigParse(format!("unknown agent {id:?}")));
        }
    }
    Ok(registry
        .agents
        .iter()
        .filter(|(id, agent)| {
            let supports = match kind {
                ItemKind::Mcp => agent
                    .mcp
                    .as_ref()
                    .is_some_and(|targets| targets.project.is_some()),
                ItemKind::Hook => agent
                    .hooks
                    .as_ref()
                    .is_some_and(|targets| targets.project.is_some()),
                ItemKind::Subagent => agent
                    .subagents
                    .as_ref()
                    .is_some_and(|targets| targets.project.is_some()),
                _ => false,
            };
            supports
                && enabled.contains(id)
                && (agents.iter().any(|id| id == "*") || agents.contains(id))
        })
        .map(|(id, _)| id.clone())
        .collect())
}
