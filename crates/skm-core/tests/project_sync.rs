use std::{
    fs,
    sync::mpsc,
    time::{Duration, Instant},
};

use skm_core::{
    engine,
    events::CoreEvent,
    library::{ItemSelection, ItemSet, Library},
    paths::Paths,
    projects::{
        self, Project,
        sync::{self, Change, SyncOutcome},
    },
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef},
};

struct Fixture {
    _temp: tempfile::TempDir,
    paths: Paths,
    store: Store,
    registry: Registry,
    library: Library,
    projects: Vec<Project>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut paths = Paths::resolve(Some(&temp.path().join("manager")), None, None).unwrap();
        paths.ensure_layout().unwrap();
        let store = Store::open(&paths).unwrap();
        let registry = Registry::load().unwrap();
        for name in ["alpha", "beta"] {
            fs::create_dir_all(paths.library.join("skills").join(name)).unwrap();
            fs::write(
                paths.library.join("skills").join(name).join("SKILL.md"),
                format!("---\nname: {name}\ndescription: fixture\n---\noriginal {name}\n"),
            )
            .unwrap();
        }
        store.write(|connection| {
            for (id, name) in [(1,"alpha"), (2,"beta")] {
                connection.execute("INSERT INTO items(id,kind,name,rel_path,stamp_mtime,stamp_size,indexed_at) VALUES (?1,'skill',?2,?3,0,0,0)", rusqlite::params![id,name,format!("skills/{name}")])?;
            }
            Ok(())
        }).unwrap();
        let mut library = Library::default();
        library.sets.insert(
            "team".into(),
            ItemSet {
                items: ItemSelection {
                    skills: vec!["alpha".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        library
            .save(&paths.library.join("library.toml"), &[])
            .unwrap();
        let mut projects = Vec::new();
        for i in 0..3 {
            let path = temp.path().join(format!("project-{i}"));
            fs::create_dir(&path).unwrap();
            let project = projects::register(&store, &path).unwrap();
            projects::use_set(&store, &library, project.id, "team").unwrap();
            let result = projects::apply_project(
                &paths, &store, &project, &library, &registry, "fixture", None,
            )
            .unwrap();
            assert!(
                result.failed.is_empty() && result.issues.is_empty(),
                "{result:?}"
            );
            projects.push(project);
        }
        Self {
            _temp: temp,
            paths,
            store,
            registry,
            library,
            projects,
        }
    }

    fn change(&self, change: Change) -> Vec<SyncOutcome> {
        sync::sync_change(
            &self.paths,
            &self.store,
            &self.library,
            &self.registry,
            &change,
        )
        .unwrap()
    }

    fn update(&self) {
        fs::write(
            self.paths.library.join("skills/alpha/SKILL.md"),
            "updated library alpha",
        )
        .unwrap();
    }

    fn copy(&self, index: usize) -> std::path::PathBuf {
        self.projects[index]
            .path
            .join(".agents/skills/alpha/SKILL.md")
    }

    fn lock(&self, index: usize) -> Vec<u8> {
        fs::read(self.projects[index].path.join("skills-lock.json")).unwrap()
    }
}

fn applied(outcomes: &[SyncOutcome]) {
    for outcome in outcomes {
        assert!(
            matches!(outcome, SyncOutcome::Applied { result, .. } if result.failed.is_empty() && result.issues.is_empty()),
            "{outcome:?}"
        );
    }
}

#[test]
fn library_update_rewrites_three_copies_and_locks_under_one_second_and_undo_restores() {
    let fixture = Fixture::new();
    let old_copies: Vec<_> = (0..3).map(|i| fs::read(fixture.copy(i)).unwrap()).collect();
    let old_locks: Vec<_> = (0..3).map(|i| fixture.lock(i)).collect();
    fixture.update();
    let start = Instant::now();
    let outcomes = fixture.change(Change::Items(vec!["1".into()]));
    let elapsed = start.elapsed();
    assert_eq!(outcomes.len(), 3);
    applied(&outcomes);
    assert!(
        elapsed < Duration::from_secs(1),
        "three-project sync {elapsed:?}, spec §14 budget is 1 s for 20 projects"
    );
    for (i, outcome) in outcomes.iter().enumerate() {
        assert_eq!(
            fs::read_to_string(fixture.copy(i)).unwrap(),
            "updated library alpha"
        );
        assert_ne!(fixture.lock(i), old_locks[i]);
        let lock: serde_json::Value = serde_json::from_slice(&fixture.lock(i)).unwrap();
        assert_eq!(
            lock["skills"]["alpha"]["computedHash"],
            skm_core::content_hash::content_hash(&fixture.paths.library.join("skills/alpha"))
                .unwrap()
        );
        let SyncOutcome::Applied { result, .. } = outcome else {
            unreachable!()
        };
        assert!(
            engine::list_ops(&fixture.store)
                .unwrap()
                .iter()
                .any(|op| op.id == result.op_id)
        );
        let undo = engine::undo(
            &fixture.paths,
            &fixture.store,
            Some(result.op_id),
            "test",
            None,
        )
        .unwrap();
        assert!(undo.failed.is_empty() && undo.issues.is_empty(), "{undo:?}");
        assert_eq!(fs::read(fixture.copy(i)).unwrap(), old_copies[i]);
        assert_eq!(fixture.lock(i), old_locks[i]);
    }
}

#[test]
fn edited_copy_is_untouched_reported_and_does_not_block_other_projects() {
    let fixture = Fixture::new();
    fs::write(fixture.copy(0), "teammate edit").unwrap();
    let lock = fixture.lock(0);
    fixture.update();
    let outcomes = fixture.change(Change::Items(vec!["1".into()]));
    assert!(
        matches!(&outcomes[0], SyncOutcome::Refused { issues, .. } if issues.iter().any(|issue| issue.code == "E_PROJECT_EDITED"))
    );
    applied(&outcomes[1..]);
    assert_eq!(
        fs::read_to_string(fixture.copy(0)).unwrap(),
        "teammate edit"
    );
    assert_eq!(fixture.lock(0), lock);
    assert_eq!(
        sync::pending_count(&fixture.store, fixture.projects[0].id).unwrap(),
        1
    );
}

#[test]
fn paused_changes_persist_deduplicate_and_resume_applies_latest_set_and_item_state() {
    let mut fixture = Fixture::new();
    let id = fixture.projects[0].id;
    let old_copy = fs::read(fixture.copy(0)).unwrap();
    sync::pause(&fixture.store, id).unwrap();
    fixture.update();
    fixture.change(Change::Items(vec!["1".into()]));
    fixture.change(Change::Items(vec!["1".into()]));
    fixture
        .library
        .sets
        .get_mut("team")
        .unwrap()
        .items
        .skills
        .push("beta".into());
    fixture.change(Change::Set("team".into()));
    assert_eq!(fs::read(fixture.copy(0)).unwrap(), old_copy);
    assert!(
        !fixture.projects[0]
            .path
            .join(".agents/skills/beta")
            .exists()
    );
    assert_eq!(sync::pending_count(&fixture.store, id).unwrap(), 2);
    drop(fixture.store);
    fixture.store = Store::open(&fixture.paths).unwrap();
    assert_eq!(sync::pending_count(&fixture.store, id).unwrap(), 2);
    assert!(!projects::list(&fixture.store).unwrap()[0].auto_sync);
    let result = sync::resume(
        &fixture.paths,
        &fixture.store,
        &fixture.library,
        &fixture.registry,
        id,
    )
    .unwrap();
    applied(&[result]);
    assert_eq!(sync::pending_count(&fixture.store, id).unwrap(), 0);
    assert_eq!(
        fs::read_to_string(fixture.copy(0)).unwrap(),
        "updated library alpha"
    );
    assert!(
        fixture.projects[0]
            .path
            .join(".agents/skills/beta/SKILL.md")
            .is_file()
    );
    let lock: serde_json::Value = serde_json::from_slice(&fixture.lock(0)).unwrap();
    assert!(lock["skills"]["beta"].is_object());
}

#[test]
fn set_removal_reconciles_only_subscribers_and_direct_items_survive() {
    let mut fixture = Fixture::new();
    projects::set_direct_item(
        &fixture.store,
        &fixture.registry,
        fixture.projects[1].id,
        &ItemRef {
            kind: ItemKind::Skill,
            name: "alpha".into(),
        },
        None,
    )
    .unwrap();
    projects::unuse_set(&fixture.store, fixture.projects[2].id, "team").unwrap();
    fixture.library.sets.remove("team");
    let outcomes = fixture.change(Change::Set("team".into()));
    assert_eq!(outcomes.len(), 2);
    applied(&outcomes);
    assert!(!fixture.copy(0).exists());
    assert!(fixture.copy(1).is_file());
    assert!(fixture.copy(2).is_file());
}

#[test]
fn unrelated_ids_do_not_queue_and_owned_targets_reconcile_after_global_subtraction() {
    let mut fixture = Fixture::new();
    sync::pause(&fixture.store, fixture.projects[0].id).unwrap();
    assert!(fixture.change(Change::Items(vec!["2".into()])).is_empty());
    assert_eq!(
        sync::pending_count(&fixture.store, fixture.projects[0].id).unwrap(),
        0
    );
    fixture.change(Change::Items(vec!["1".into(), "2".into()]));
    assert_eq!(
        sync::pending_count(&fixture.store, fixture.projects[0].id).unwrap(),
        1
    );
    fixture.library.global.skills.push("alpha".into());
    let outcomes = fixture.change(Change::Items(vec!["1".into()]));
    assert!(matches!(outcomes[0], SyncOutcome::Pending { .. }));
    applied(&outcomes[1..]);
    assert!(fixture.copy(0).exists());
    assert!(!fixture.copy(1).exists());
    assert!(!fixture.copy(2).exists());
}

#[test]
fn pulled_copy_can_sync_locks_and_other_projects_without_another_approval() {
    let fixture = Fixture::new();
    fs::write(fixture.copy(0), "teammate edit").unwrap();
    let edited = sync::edited_copy(
        &fixture.paths,
        &fixture.store,
        fixture.projects[0].id,
        "alpha",
    )
    .unwrap();
    let result = sync::pull_into_library(&fixture.paths, &fixture.store, &edited).unwrap();
    assert!(
        result.failed.is_empty() && result.issues.is_empty(),
        "{result:?}"
    );
    let outcomes = fixture.change(Change::Items(vec!["1".into()]));
    applied(&outcomes);
    for i in 0..3 {
        assert_eq!(
            fs::read_to_string(fixture.copy(i)).unwrap(),
            "teammate edit"
        );
        assert_eq!(fixture.lock(i), fixture.lock(0));
    }
}

#[test]
fn overwrite_is_review_bound_and_undo_restores_teammate_copy_and_lock() {
    let fixture = Fixture::new();
    fs::write(fixture.copy(0), "teammate edit").unwrap();
    let edited = sync::edited_copy(
        &fixture.paths,
        &fixture.store,
        fixture.projects[0].id,
        "alpha",
    )
    .unwrap();
    fs::write(fixture.copy(0), "later edit").unwrap();
    assert!(
        sync::overwrite(
            &fixture.paths,
            &fixture.store,
            &fixture.library,
            &fixture.registry,
            &edited
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(fixture.copy(0)).unwrap(), "later edit");
    let edited = sync::edited_copy(
        &fixture.paths,
        &fixture.store,
        fixture.projects[0].id,
        "alpha",
    )
    .unwrap();
    let old_lock = fixture.lock(0);
    fixture.update();
    let results = sync::overwrite(
        &fixture.paths,
        &fixture.store,
        &fixture.library,
        &fixture.registry,
        &edited,
    )
    .unwrap();
    assert_eq!(results.len(), 2);
    for result in &results {
        assert!(
            result.failed.is_empty() && result.issues.is_empty(),
            "{result:?}"
        );
    }
    assert_eq!(
        fs::read_to_string(fixture.copy(0)).unwrap(),
        "updated library alpha"
    );
    let result = engine::undo(
        &fixture.paths,
        &fixture.store,
        Some(results[1].op_id),
        "test",
        None,
    )
    .unwrap();
    assert!(
        result.failed.is_empty() && result.issues.is_empty(),
        "{result:?}"
    );
    assert_eq!(fs::read_to_string(fixture.copy(0)).unwrap(), "later edit");
    assert_eq!(fixture.lock(0), old_lock);
}

#[test]
fn pull_imports_reviewed_copy_and_can_be_undone() {
    let fixture = Fixture::new();
    fs::write(fixture.copy(0), "teammate edit").unwrap();
    let edited = sync::edited_copy(
        &fixture.paths,
        &fixture.store,
        fixture.projects[0].id,
        "alpha",
    )
    .unwrap();
    let original = fs::read(fixture.paths.library.join("skills/alpha/SKILL.md")).unwrap();
    let result = sync::pull_into_library(&fixture.paths, &fixture.store, &edited).unwrap();
    assert!(
        result.failed.is_empty() && result.issues.is_empty(),
        "{result:?}"
    );
    assert_eq!(
        fs::read_to_string(fixture.paths.library.join("skills/alpha/SKILL.md")).unwrap(),
        "teammate edit"
    );
    let undo = engine::undo(
        &fixture.paths,
        &fixture.store,
        Some(result.op_id),
        "test",
        None,
    )
    .unwrap();
    assert!(undo.failed.is_empty() && undo.issues.is_empty());
    assert_eq!(
        fs::read(fixture.paths.library.join("skills/alpha/SKILL.md")).unwrap(),
        original
    );
}

#[test]
fn keep_pauses_without_replacing_the_reviewed_copy() {
    let fixture = Fixture::new();
    fs::write(fixture.copy(0), "teammate edit").unwrap();
    let edited = sync::edited_copy(
        &fixture.paths,
        &fixture.store,
        fixture.projects[0].id,
        "alpha",
    )
    .unwrap();
    sync::keep(&fixture.paths, &fixture.store, &edited).unwrap();
    fixture.update();
    let outcomes = fixture.change(Change::Items(vec!["1".into()]));
    assert!(matches!(outcomes[0], SyncOutcome::Pending { .. }));
    assert_eq!(
        fs::read_to_string(fixture.copy(0)).unwrap(),
        "teammate edit"
    );
}

#[test]
fn watcher_items_changed_can_drive_background_sync_without_timers() {
    let fixture = Fixture::new();
    fixture.update();
    let (events, input) = mpsc::channel();
    let (output, results) = mpsc::channel();
    std::thread::scope(|scope| {
        let fixture = &fixture;
        let worker = scope.spawn(move || {
            sync::run_watcher_sync(
                &fixture.paths,
                &fixture.store,
                &fixture.registry,
                input,
                &output,
            )
        });
        events
            .send(CoreEvent::ItemsChanged {
                ids: vec!["1".into()],
            })
            .unwrap();
        let outcomes = results
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert_eq!(outcomes.len(), 3);
        applied(&outcomes);
        for i in 0..3 {
            assert_eq!(
                fs::read_to_string(fixture.copy(i)).unwrap(),
                "updated library alpha"
            );
        }
        drop(events);
        worker.join().unwrap();
    });
}
