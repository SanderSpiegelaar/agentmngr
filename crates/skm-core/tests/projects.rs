use std::{collections::BTreeSet, fs, path::PathBuf};

use skm_core::{
    engine::{self, Action},
    library::{ItemSelection, Library},
    paths::Paths,
    projects::{self, DirectItem, ProjectMode},
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef},
};

fn fixture() -> (tempfile::TempDir, Paths, Store, Registry) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
    paths.ensure_layout().unwrap();
    let store = Store::open(&paths).unwrap();
    let registry = Registry::load().unwrap();
    store
        .write(|connection| {
            for id in ["claude-code", "codex", "cursor", "opencode"] {
                connection.execute(
                    "INSERT INTO agents(id,detected,enabled) VALUES (?1,1,1)",
                    [id],
                )?;
            }
            Ok(())
        })
        .unwrap();
    (temp, paths, store, registry)
}

fn item(kind: ItemKind, name: &str) -> ItemRef {
    ItemRef {
        kind,
        name: name.into(),
    }
}

fn agents(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn project_directory(temp: &tempfile::TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    fs::create_dir(&path).unwrap();
    path.canonicalize().unwrap()
}

fn skill(paths: &Paths, name: &str) {
    let path = paths.library.join("skills").join(name);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("SKILL.md"), name).unwrap();
}

#[test]
fn registration_and_selection_persist_without_a_repository_footprint() {
    let (temp, paths, store, registry) = fixture();
    let repo = project_directory(&temp, "repo");
    fs::write(repo.join("tracked"), "untouched").unwrap();
    let before = engine::snapshot(&repo, &paths).unwrap();
    let project = projects::register(&store, &repo).unwrap();
    assert_eq!(project.mode, ProjectMode::Copy);
    assert!(project.auto_sync);
    assert_eq!(
        projects::register(&store, &repo.join(".")).unwrap(),
        project
    );
    let mut library = Library::default();
    library.create_set("rust", None).unwrap();
    projects::use_set(&store, &library, project.id, "rust").unwrap();
    projects::set_direct_item(
        &store,
        &registry,
        project.id,
        &item(ItemKind::Skill, "review"),
        None,
    )
    .unwrap();
    projects::set_direct_item(
        &store,
        &registry,
        project.id,
        &item(ItemKind::Mcp, "docs"),
        Some(&agents(&["codex"])),
    )
    .unwrap();
    projects::set_mode(&store, project.id, ProjectMode::Link).unwrap();
    projects::set_auto_sync(&store, project.id, false).unwrap();
    assert_eq!(engine::snapshot(&repo, &paths).unwrap(), before);
    drop(store);
    let store = Store::open(&paths).unwrap();
    let restored = projects::list(&store).unwrap().remove(0);
    assert_eq!(restored.id, project.id);
    assert_eq!(restored.mode, ProjectMode::Link);
    assert!(!restored.auto_sync);
    assert_eq!(projects::used_sets(&store, project.id).unwrap(), ["rust"]);
    let direct = projects::direct_items(&store, project.id).unwrap();
    assert_eq!(
        direct,
        vec![
            DirectItem {
                item: item(ItemKind::Mcp, "docs"),
                agents: Some(agents(&["codex"]))
            },
            DirectItem {
                item: item(ItemKind::Skill, "review"),
                agents: None
            },
        ]
    );
    projects::remove_direct_item(&store, project.id, &item(ItemKind::Mcp, "docs")).unwrap();
    projects::unuse_set(&store, project.id, "rust").unwrap();
    assert!(projects::used_sets(&store, project.id).unwrap().is_empty());
    assert!(
        projects::direct_items(&store, project.id)
            .unwrap()
            .iter()
            .all(|entry| entry.item.kind != ItemKind::Mcp)
    );
    assert_eq!(engine::snapshot(&repo, &paths).unwrap(), before);
}

#[test]
fn effective_unions_kinds_subtracts_global_per_agent_and_expands_supported_enabled_agents() {
    let registry = Registry::load().unwrap();
    let mut library = Library::default();
    library.create_set("a", None).unwrap();
    library.create_set("b", None).unwrap();
    let a = &mut library.sets.get_mut("a").unwrap().items;
    a.skills = agents(&["shared", "only-a"]);
    a.mcp.insert("docs".into(), agents(&["codex"]));
    a.hooks.insert("fmt".into(), agents(&["*"]));
    a.subagents.insert("reviewer".into(), agents(&["*"]));
    let b = &mut library.sets.get_mut("b").unwrap().items;
    b.skills = agents(&["shared", "only-b"]);
    b.mcp.insert("docs".into(), agents(&["cursor"]));
    library.global.skills = agents(&["shared"]);
    library.global.mcp.insert("docs".into(), agents(&["codex"]));
    library
        .global
        .hooks
        .insert("fmt".into(), agents(&["claude-code"]));
    let mut direct = ItemSelection {
        skills: agents(&["direct", "shared"]),
        ..ItemSelection::default()
    };
    direct.mcp.insert("private".into(), agents(&["*"]));
    let enabled = agents(&["claude-code", "codex", "cursor", "opencode"]);
    let effective =
        projects::effective(&library, &agents(&["a", "b"]), &direct, &registry, &enabled).unwrap();
    assert_eq!(effective.skills, ["direct", "only-a", "only-b"]);
    assert_eq!(effective.mcp["docs"], ["cursor"]);
    assert_eq!(
        effective.mcp["private"],
        ["claude-code", "codex", "cursor", "opencode"]
    );
    assert_eq!(effective.hooks["fmt"], ["codex", "cursor"]);
    assert_eq!(
        effective.subagents["reviewer"],
        ["claude-code", "codex", "cursor", "opencode"]
    );
    direct.mcp.insert("docs".into(), agents(&["claude-code"]));
    direct.hooks.insert("fmt".into(), vec![]);
    let effective =
        projects::effective(&library, &agents(&["a", "b"]), &direct, &registry, &enabled).unwrap();
    assert_eq!(effective.mcp["docs"], ["claude-code"]);
    assert!(!effective.hooks.contains_key("fmt"));
    assert!(!effective.mcp["private"].contains(&"gemini".into()));
    library
        .global
        .subagents
        .insert("reviewer".into(), agents(&["*"]));
    assert!(
        projects::effective(&library, &agents(&["a", "b"]), &direct, &registry, &enabled)
            .unwrap()
            .subagents
            .is_empty()
    );
}

#[test]
fn direct_null_inherits_sets_but_explicit_wildcard_overrides_them() {
    let (temp, _paths, store, registry) = fixture();
    let project = projects::register(&store, &project_directory(&temp, "repo")).unwrap();
    let mut library = Library::default();
    library.create_set("rust", None).unwrap();
    library
        .sets
        .get_mut("rust")
        .unwrap()
        .items
        .mcp
        .insert("docs".into(), agents(&["codex"]));
    projects::use_set(&store, &library, project.id, "rust").unwrap();
    let docs = item(ItemKind::Mcp, "docs");
    projects::set_direct_item(&store, &registry, project.id, &docs, None).unwrap();
    assert_eq!(
        projects::effective_project(&store, &library, project.id, &registry)
            .unwrap()
            .mcp["docs"],
        ["codex"]
    );
    projects::set_direct_item(&store, &registry, project.id, &docs, Some(&agents(&["*"]))).unwrap();
    assert_eq!(
        projects::effective_project(&store, &library, project.id, &registry)
            .unwrap()
            .mcp["docs"],
        ["claude-code", "codex", "cursor", "opencode"]
    );
    projects::unuse_set(&store, project.id, "rust").unwrap();
    projects::set_direct_item(&store, &registry, project.id, &docs, None).unwrap();
    assert_eq!(
        projects::effective_project(&store, &library, project.id, &registry)
            .unwrap()
            .mcp["docs"]
            .len(),
        4
    );
}

#[test]
fn set_edits_plan_exactly_using_projects_without_touching_unrelated_targets() {
    let (temp, paths, store, registry) = fixture();
    let first = projects::register(&store, &project_directory(&temp, "first")).unwrap();
    let second = projects::register(&store, &project_directory(&temp, "second")).unwrap();
    let unrelated = projects::register(&store, &project_directory(&temp, "unrelated")).unwrap();
    let mut before = Library::default();
    before.create_set("rust", None).unwrap();
    before.sets.get_mut("rust").unwrap().items.skills = agents(&["old"]);
    before.create_set("other", None).unwrap();
    before.sets.get_mut("other").unwrap().items.skills = agents(&["foreign"]);
    for project in [&first, &second] {
        projects::use_set(&store, &before, project.id, "rust").unwrap();
    }
    projects::use_set(&store, &before, first.id, "other").unwrap();
    projects::use_set(&store, &before, unrelated.id, "other").unwrap();
    projects::set_auto_sync(&store, second.id, false).unwrap();
    for name in ["old", "new", "foreign"] {
        skill(&paths, name);
    }
    let empty = Library::default();
    let initial =
        projects::plan_set_change(&paths, &store, &empty, &before, "rust", &registry).unwrap();
    assert!(!first.path.join(".agents").exists());
    assert!(!second.path.join(".agents").exists());
    for result in initial {
        let applied = engine::apply(&paths, &store, &result.plan, "test", None).unwrap();
        assert!(applied.failed.is_empty() && applied.issues.is_empty());
    }
    fs::create_dir_all(first.path.join(".agents/skills/foreign")).unwrap();
    fs::write(
        first.path.join(".agents/skills/foreign/SKILL.md"),
        "not ours",
    )
    .unwrap();
    projects::set_direct_item(
        &store,
        &registry,
        first.id,
        &item(ItemKind::Skill, "old"),
        None,
    )
    .unwrap();
    let mut after = before.clone();
    after.sets.get_mut("rust").unwrap().items.skills = agents(&["new"]);
    let plans =
        projects::plan_set_change(&paths, &store, &before, &after, "rust", &registry).unwrap();
    assert_eq!(
        plans.iter().map(|p| p.project.id).collect::<BTreeSet<_>>(),
        BTreeSet::from([first.id, second.id])
    );
    assert!(plans.iter().all(|p| p.plan.issues.is_empty()));
    let first_plan = &plans
        .iter()
        .find(|p| p.project.id == first.id)
        .unwrap()
        .plan;
    assert_eq!(first_plan.actions.len(), 1);
    assert!(
        matches!(&first_plan.actions[0].action, Action::CopyTree { target, .. } if target.ends_with("skills/new"))
    );
    let second_plan = plans.iter().find(|p| p.project.id == second.id).unwrap();
    assert!(!second_plan.project.auto_sync);
    assert_eq!(second_plan.plan.actions.len(), 2);
    assert!(
        second_plan
            .plan
            .actions
            .iter()
            .any(|a| matches!(&a.action, Action::Trash {target} if target.ends_with("skills/old")))
    );
    assert!(!first.path.join(".agents/skills/new").exists());
    assert!(!unrelated.path.join(".agents").exists());
    after.delete_set("rust").unwrap();
    let deletion =
        projects::plan_set_change(&paths, &store, &before, &after, "rust", &registry).unwrap();
    assert_eq!(deletion.len(), 2);
    assert!(
        deletion
            .iter()
            .find(|p| p.project.id == first.id)
            .unwrap()
            .plan
            .actions
            .is_empty()
    );
    assert_eq!(
        deletion
            .iter()
            .find(|p| p.project.id == second.id)
            .unwrap()
            .plan
            .actions
            .len(),
        1
    );
}

#[test]
fn edited_copies_are_refused_by_scoped_plans_and_unregister_keeps_files() {
    let (temp, paths, store, registry) = fixture();
    let project = projects::register(&store, &project_directory(&temp, "repo")).unwrap();
    skill(&paths, "review");
    let mut library = Library::default();
    library.create_set("rust", None).unwrap();
    library.sets.get_mut("rust").unwrap().items.skills = agents(&["review"]);
    projects::use_set(&store, &library, project.id, "rust").unwrap();
    let plan = projects::plan_set_change(
        &paths,
        &store,
        &Library::default(),
        &library,
        "rust",
        &registry,
    )
    .unwrap()
    .remove(0)
    .plan;
    assert!(
        engine::apply(&paths, &store, &plan, "test", None)
            .unwrap()
            .failed
            .is_empty()
    );
    let target = project.path.join(".agents/skills/review/SKILL.md");
    fs::write(&target, "teammate edits").unwrap();
    let plan = projects::plan_set_change(&paths, &store, &library, &library, "rust", &registry)
        .unwrap()
        .remove(0)
        .plan;
    assert!(plan.actions.is_empty());
    assert_eq!(plan.issues[0].code, "E_PROJECT_EDITED");
    projects::unregister(&store, project.id).unwrap();
    assert!(projects::list(&store).unwrap().is_empty());
    assert_eq!(fs::read_to_string(target).unwrap(), "teammate edits");
    store
        .read(|connection| {
            for table in ["project_sets", "project_items", "ledger"] {
                let count: i64 =
                    connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(count, 0);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn sets_crud_and_all_memberships_round_trip_in_library_only() {
    let (_temp, paths, store, registry) = fixture();
    let known: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    let mut library = Library::default();
    library
        .create_set("rust", Some("Rust projects".into()))
        .unwrap();
    assert!(library.create_set("rust", None).is_err());
    let mut set = library.sets["rust"].clone();
    for kind in [
        ItemKind::Skill,
        ItemKind::Mcp,
        ItemKind::Hook,
        ItemKind::Subagent,
    ] {
        set.items
            .set_item(&item(kind, "shared-name"), Some(&agents(&["codex"])))
            .unwrap();
    }
    library.update_set("rust", set, &known).unwrap();
    let manifest = paths.library.join("library.toml");
    fs::write(&manifest, "# retained\nschema = 1\n").unwrap();
    library.save(&manifest, &known).unwrap();
    assert_eq!(Library::load(&manifest, &known).unwrap(), library);
    assert!(
        fs::read_to_string(&manifest)
            .unwrap()
            .starts_with("# retained")
    );
    let mut set = library.sets["rust"].clone();
    set.items.remove_item(&item(ItemKind::Mcp, "shared-name"));
    library.update_set("rust", set, &known).unwrap();
    assert!(library.sets["rust"].items.mcp.is_empty());
    assert_eq!(library.sets["rust"].items.hooks.len(), 1);
    library.delete_set("rust").unwrap();
    library.save(&manifest, &known).unwrap();
    assert!(Library::load(&manifest, &known).unwrap().sets.is_empty());
    assert!(projects::list(&store).unwrap().is_empty());
}

#[test]
fn unsupported_memberships_and_invalid_references_refuse_without_writes() {
    let (temp, _paths, store, registry) = fixture();
    let project = projects::register(&store, &project_directory(&temp, "repo")).unwrap();
    let mut library = Library::default();
    library.create_set("rust", None).unwrap();
    assert!(projects::use_set(&store, &library, project.id, "missing").is_err());
    assert!(
        projects::set_direct_item(
            &store,
            &registry,
            project.id,
            &item(ItemKind::Plugin, "plugin"),
            None
        )
        .is_err()
    );
    assert!(
        projects::set_direct_item(
            &store,
            &registry,
            project.id,
            &item(ItemKind::Mcp, "docs"),
            Some(&agents(&["missing-agent"]))
        )
        .is_err()
    );
    assert!(
        projects::set_direct_item(
            &store,
            &registry,
            999,
            &item(ItemKind::Skill, "review"),
            None
        )
        .is_err()
    );
    let mut bad = library.sets["rust"].clone();
    bad.items
        .plugins
        .insert("plugin".into(), agents(&["codex"]));
    assert!(library.update_set("rust", bad, &["codex"]).is_err());
    assert!(library.sets["rust"].items.plugins.is_empty());
    assert!(
        projects::direct_items(&store, project.id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn direct_non_skill_kinds_round_trip_and_m1_plans_only_universal_skills() {
    let (temp, paths, store, registry) = fixture();
    let project = projects::register(&store, &project_directory(&temp, "repo")).unwrap();
    for kind in [ItemKind::Mcp, ItemKind::Hook, ItemKind::Subagent] {
        projects::set_direct_item(&store, &registry, project.id, &item(kind, "shared"), None)
            .unwrap();
    }
    let direct = projects::direct_items(&store, project.id).unwrap();
    assert_eq!(direct.len(), 3);
    assert!(direct.iter().all(|entry| entry.agents.is_none()));
    let mut before = Library::default();
    before.create_set("rust", None).unwrap();
    projects::use_set(&store, &before, project.id, "rust").unwrap();
    let mut after = before.clone();
    after
        .sets
        .get_mut("rust")
        .unwrap()
        .items
        .mcp
        .insert("docs".into(), agents(&["*"]));
    let plans =
        projects::plan_set_change(&paths, &store, &before, &after, "rust", &registry).unwrap();
    assert_eq!(plans.len(), 1);
    assert!(plans[0].plan.actions.is_empty() && plans[0].plan.issues.is_empty());
    skill(&paths, "review");
    projects::set_mode(&store, project.id, ProjectMode::Link).unwrap();
    after
        .sets
        .get_mut("rust")
        .unwrap()
        .items
        .skills
        .push("review".into());
    let plan = projects::plan_set_change(&paths, &store, &before, &after, "rust", &registry)
        .unwrap()
        .remove(0)
        .plan;
    assert_eq!(plan.actions.len(), 1);
    assert!(
        matches!(&plan.actions[0].action, Action::Link {target, to} if target == &project.path.join(".agents/skills/review") && to == &paths.library.join("skills/review"))
    );
    assert!(!project.path.join(".agents").exists());
    assert!(!project.path.join(".claude").exists());
}
