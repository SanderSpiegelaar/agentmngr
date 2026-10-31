use std::{fs, path::PathBuf, sync::Arc, time::Duration};

use notify::{Event, EventKind, event::Flag};
use skm_core::{
    events::{self, CoreEvent},
    indexer,
    paths::Paths,
    store::Store,
    watcher::{BatchIndexer, WatchRoot, WatchedTarget, Watcher},
};

fn fixture() -> (tempfile::TempDir, Paths, Arc<Store>) {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::resolve(Some(&root.path().join("manager")), None, None).unwrap();
    let store = Arc::new(Store::open(&paths).unwrap());
    (root, paths, store)
}

fn skill(paths: &Paths, name: &str, description: &str) -> PathBuf {
    let path = paths.library.join("skills").join(name).join("SKILL.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!("---\nname: {name}\ndescription: {description}\n---\n"),
    )
    .unwrap();
    path
}

fn event(path: PathBuf) -> Event {
    Event::new(EventKind::Any).add_path(path)
}

fn ids(receiver: &std::sync::mpsc::Receiver<CoreEvent>) -> Vec<String> {
    match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
        CoreEvent::ItemsChanged { ids } => ids,
        other => panic!("expected ItemsChanged, got {other:?}"),
    }
}

#[test]
fn edit_one_skill_reindexes_exactly_one_item() {
    let (_root, paths, store) = fixture();
    let one = skill(&paths, "one", "before");
    skill(&paths, "two", "before");
    indexer::full_reindex(&store, &paths).unwrap();
    let before = indexer::load(&store).unwrap();
    let (sender, receiver) = events::channel();
    let mut processor = BatchIndexer::new(paths.clone(), store.clone(), vec![], sender).unwrap();
    skill(&paths, "one", "after");
    // Parsing the unrelated item would fail with invalid UTF-8.
    fs::write(paths.library.join("skills/two/SKILL.md"), [0xff]).unwrap();
    let changed = processor.process(&[event(one)]).unwrap();
    assert_eq!(changed, [before[0].id.to_string()]);
    assert_eq!(ids(&receiver), changed);
    let after = indexer::load(&store).unwrap();
    assert_eq!(after[0].description, "after");
    assert_eq!(after[1], before[1]);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn project_config_reclassifies_only_that_projects_changed_entries() {
    let (root, paths, store) = fixture();
    let mut targets = Vec::new();
    for project in ["one", "two"] {
        let path = root.path().join(project).join(".mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"mcpServers":{"github":{"command":"before"},"kept":{"command":"same"}},"foreign":0}"#).unwrap();
        for entry in ["github", "kept"] {
            targets.push(WatchedTarget {
                path: path.clone(),
                item_id: format!("{project}-{entry}"),
                entry: Some(vec!["mcpServers".into(), entry.into()]),
            });
        }
    }
    let (sender, receiver) = events::channel();
    let mut processor = BatchIndexer::new(paths, store, targets, sender).unwrap();
    let path = root.path().join("one/.mcp.json");
    fs::write(
        &path,
        r#"{"mcpServers":{"github":{"command":"after"},"kept":{"command":"same"}},"foreign":1}"#,
    )
    .unwrap();
    fs::write(
        root.path().join("two/.mcp.json"),
        "invalid JSON must not be read",
    )
    .unwrap();
    assert_eq!(
        processor.process(&[event(path.clone())]).unwrap(),
        ["one-github"]
    );
    assert_eq!(ids(&receiver), ["one-github"]);
    fs::write(
        &path,
        r#"{"mcpServers":{"github":{"command":"after"},"kept":{"command":"same"}},"foreign":2}"#,
    )
    .unwrap();
    assert!(processor.process(&[event(path)]).unwrap().is_empty());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn rescan_is_directory_scoped_and_discovers_insertions_and_deletions() {
    let (_root, paths, store) = fixture();
    skill(&paths, "one", "before");
    skill(&paths, "two", "before");
    indexer::full_reindex(&store, &paths).unwrap();
    let (sender, _receiver) = events::channel();
    let mut processor = BatchIndexer::new(paths.clone(), store.clone(), vec![], sender).unwrap();
    fs::write(paths.library.join("skills/two/SKILL.md"), [0xff]).unwrap();
    skill(&paths, "one", "after");
    assert_eq!(
        processor
            .process(&[event(paths.library.join("skills/one")).set_flag(Flag::Rescan)])
            .unwrap()
            .len(),
        1
    );
    skill(&paths, "new", "new item");
    assert_eq!(
        processor
            .process(&[event(paths.library.join("skills/new"))])
            .unwrap()
            .len(),
        1
    );
    fs::remove_dir_all(paths.library.join("skills/new")).unwrap();
    assert_eq!(
        processor
            .process(&[event(paths.library.join("skills/new"))])
            .unwrap()
            .len(),
        1
    );
    assert_eq!(indexer::load(&store).unwrap().len(), 2);
}

#[test]
fn native_400_file_burst_emits_one_batch_and_idle_emits_nothing() {
    let (_root, paths, store) = fixture();
    let directory = paths.library.join("mcp");
    fs::create_dir_all(&directory).unwrap();
    for number in 0..400 {
        fs::write(
            directory.join(format!("file-{number}.json")),
            r#"{"description":"before"}"#,
        )
        .unwrap();
    }
    indexer::full_reindex(&store, &paths).unwrap();
    let expected: std::collections::BTreeSet<_> = indexer::load(&store)
        .unwrap()
        .into_iter()
        .map(|row| row.id.to_string())
        .collect();
    let (sender, receiver) = events::channel();
    let processor = BatchIndexer::new(paths.clone(), store.clone(), vec![], sender).unwrap();
    let _watcher = Watcher::start(vec![WatchRoot::Tree(paths.library.clone())], processor).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    for number in 0..400 {
        fs::write(
            directory.join(format!("file-{number}.json")),
            r#"{"description":"after"}"#,
        )
        .unwrap();
    }
    assert_eq!(
        ids(&receiver)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert!(
        indexer::load(&store)
            .unwrap()
            .iter()
            .all(|row| row.description == "after")
    );
    assert!(receiver.recv_timeout(Duration::from_millis(800)).is_err());
}

#[test]
fn file_parent_filter_uses_native_events() {
    let (root, paths, store) = fixture();
    let config = root.path().join("project/.mcp.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let target = WatchedTarget {
        path: config.clone(),
        item_id: "mcp".into(),
        entry: Some(vec!["server".into()]),
    };
    let (sender, receiver) = events::channel();
    let processor = BatchIndexer::new(paths, store, vec![target], sender).unwrap();
    let _watcher = Watcher::start(vec![WatchRoot::File(config.clone())], processor).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    fs::write(config.parent().unwrap().join("unrelated.json"), "{}").unwrap();
    assert!(receiver.recv_timeout(Duration::from_millis(500)).is_err());
    fs::write(config, r#"{"server":"new"}"#).unwrap();
    assert_eq!(ids(&receiver), ["mcp"]);
}

#[test]
fn ignored_directories_do_not_reindex_items() {
    let (_root, paths, store) = fixture();
    skill(&paths, "one", "before");
    indexer::full_reindex(&store, &paths).unwrap();
    let before = indexer::load(&store).unwrap();
    let (sender, receiver) = events::channel();
    let mut processor = BatchIndexer::new(paths.clone(), store.clone(), vec![], sender).unwrap();
    skill(&paths, "one", "not indexed");
    for ignored in [
        ".git",
        "node_modules",
        "target",
        ".venv",
        "__pycache__",
        ".DS_Store",
    ] {
        assert!(
            processor
                .process(&[event(
                    paths.library.join("skills/one").join(ignored).join("file")
                )])
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(indexer::load(&store).unwrap(), before);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn changed_marker_catches_up_library_and_reclassifies_targets() {
    let (root, paths, store) = fixture();
    skill(&paths, "one", "before");
    skill(&paths, "two", "unchanged");
    indexer::full_reindex(&store, &paths).unwrap();
    let before = indexer::load(&store).unwrap();
    let target_path = root.path().join("global/skills/one");
    fs::create_dir_all(target_path.parent().unwrap()).unwrap();
    let target = WatchedTarget {
        path: target_path.clone(),
        item_id: before[0].id.to_string(),
        entry: None,
    };
    let (sender, receiver) = events::channel();
    let mut processor =
        BatchIndexer::new(paths.clone(), store.clone(), vec![target], sender).unwrap();
    skill(&paths, "one", "after");
    fs::create_dir(&target_path).unwrap();
    fs::write(target_path.join("SKILL.md"), "external skill").unwrap();
    assert_eq!(
        processor
            .process(&[event(paths.home.join(".changed"))])
            .unwrap(),
        [before[0].id.to_string()]
    );
    assert_eq!(ids(&receiver), [before[0].id.to_string()]);
    assert_eq!(indexer::load(&store).unwrap()[1], before[1]);
    assert!(
        processor
            .process(&[event(paths.home.join(".changed"))])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn missing_project_skills_tree_is_observed_when_created() {
    let (root, paths, store) = fixture();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let skills = project.join(".agents/skills");
    let target_path = skills.join("one");
    let target = WatchedTarget {
        path: target_path.clone(),
        item_id: "one".into(),
        entry: None,
    };
    let (sender, receiver) = events::channel();
    let processor = BatchIndexer::new(paths, store, vec![target], sender).unwrap();
    let _watcher = Watcher::start(vec![WatchRoot::Tree(skills)], processor).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    fs::create_dir_all(&target_path).unwrap();
    fs::write(target_path.join("SKILL.md"), "skill").unwrap();
    assert_eq!(ids(&receiver), ["one"]);
}

#[test]
fn invalid_config_does_not_swallow_other_changes_in_the_batch() {
    let (root, paths, store) = fixture();
    let skill_path = skill(&paths, "one", "before");
    indexer::full_reindex(&store, &paths).unwrap();
    let id = indexer::load(&store).unwrap()[0].id.to_string();
    let config = root.path().join("config.json");
    fs::write(&config, r#"{"server":1}"#).unwrap();
    let target = WatchedTarget {
        path: config.clone(),
        item_id: "server".into(),
        entry: Some(vec!["server".into()]),
    };
    let (sender, receiver) = events::channel();
    let mut processor = BatchIndexer::new(paths.clone(), store, vec![target], sender).unwrap();
    skill(&paths, "one", "after");
    fs::write(&config, "{").unwrap();
    assert_eq!(
        processor
            .process(&[event(skill_path), event(config.clone())])
            .unwrap(),
        std::slice::from_ref(&id)
    );
    assert_eq!(ids(&receiver), [id]);
    assert!(
        matches!(receiver.recv().unwrap(), CoreEvent::Error(error) if error.code() == "E_CONFIG_PARSE")
    );
    fs::write(&config, r#"{"server":2}"#).unwrap();
    assert_eq!(processor.process(&[event(config)]).unwrap(), ["server"]);
    assert_eq!(ids(&receiver), ["server"]);
}
