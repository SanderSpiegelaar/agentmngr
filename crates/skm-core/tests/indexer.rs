use std::{
    fs,
    time::{Duration, Instant},
};

use skm_core::{Error, indexer, paths::Paths, store::Store};

fn setup() -> (tempfile::TempDir, Paths, Store) {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::resolve(Some(root.path()), None, None).unwrap();
    let store = Store::open(&paths).unwrap();
    (root, paths, store)
}

fn skill(paths: &Paths, name: &str, description: &str) {
    let directory = paths.library.join("skills").join(name);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nBody\n"),
    )
    .unwrap();
}

#[test]
fn startup_load_reads_only_sqlite() -> Result<(), Error> {
    let (_root, paths, store) = setup();
    skill(&paths, "test", "test skill");
    indexer::full_reindex(&store, &paths)?;
    let expected = indexer::load(&store)?;
    fs::rename(&paths.library, paths.home.join("unavailable-library"))?;
    assert_eq!(indexer::load(&store)?, expected);
    assert!(!paths.library.exists());
    println!(
        "startup_load_reads_only_sqlite: 1 cached row loaded with library directory renamed away"
    );
    Ok(())
}

#[test]
fn all_kinds_and_full_reindex_are_identical() -> Result<(), Error> {
    let (_root, paths, store) = setup();
    skill(&paths, "test", "test skill");
    for (relative, text) in [
        (
            "mcp/server.json",
            r#"{"command":"server","description":"MCP"}"#,
        ),
        ("hooks/fmt.json", r#"{"description":"Hook"}"#),
        (
            "subagents/reviewer.md",
            "---\nname: reviewer\ndescription: Review\n---\nBody",
        ),
        ("plugins/acme/plugin.json", r#"{"description":"Plugin"}"#),
        ("plugins/.claude-plugin/marketplace.json", "{}"),
        ("mcp/ignored.txt", "ignored"),
    ] {
        let file = paths.library.join(relative);
        fs::create_dir_all(file.parent().unwrap())?;
        fs::write(file, text)?;
    }
    fs::write(
        paths.library.join("library.toml"),
        "schema = 1\n[skills.test]\nsource = { kind = 'local' }\n",
    )?;
    assert_eq!(indexer::full_reindex(&store, &paths)?.inserted, 5);
    store.write(|conn| {
        conn.execute(
            "UPDATE items SET content_hash='lazy-existing' WHERE kind='skill'",
            [],
        )?;
        Ok(())
    })?;
    let expected = indexer::load(&store)?;
    assert!(expected.iter().all(|row| row.problems_json == "[]"));
    assert!(
        expected
            .iter()
            .find(|row| row.kind == "skill")
            .unwrap()
            .source_json
            .is_some()
    );
    assert_eq!(indexer::full_reindex(&store, &paths)?.unchanged, 5);
    assert_eq!(indexer::load(&store)?, expected);
    println!(
        "full_reindex: all 5 kinds identical across every column, including IDs, indexed_at, stamps and lazy hash"
    );
    Ok(())
}

#[test]
fn catch_up_changes_inserts_deletes_and_targets_ids() -> Result<(), Error> {
    let (_root, paths, store) = setup();
    skill(&paths, "one", "before");
    skill(&paths, "two", "before");
    indexer::full_reindex(&store, &paths)?;
    let before = indexer::load(&store)?;
    let one = before.iter().find(|row| row.name == "one").unwrap();
    // Editing an existing SKILL.md need not change its parent directory's stamp.
    skill(&paths, "one", "after with a different size");
    skill(&paths, "two", "not yet indexed");
    skill(&paths, "three", "new item");
    let counts = indexer::catch_up(&store, &paths, Some(&[one.id]))?;
    assert_eq!(counts.updated, 1);
    assert_eq!(counts.total, 2);
    let rows = indexer::load(&store)?;
    let changed = rows.iter().find(|row| row.name == "one").unwrap();
    assert_eq!(changed.id, one.id);
    assert_eq!(changed.stamp, one.stamp);
    assert_ne!(changed.file_stamp, one.file_stamp);
    assert_eq!(changed.description, "after with a different size");
    assert_eq!(
        rows.iter()
            .find(|row| row.name == "two")
            .unwrap()
            .description,
        "before"
    );
    fs::remove_dir_all(paths.library.join("skills/one"))?;
    let counts = indexer::catch_up(&store, &paths, None)?;
    assert_eq!(
        (
            counts.inserted,
            counts.updated,
            counts.removed,
            counts.total
        ),
        (1, 1, 1, 2)
    );
    assert_eq!(indexer::catch_up(&store, &paths, None)?.unchanged, 2);
    Ok(())
}

#[test]
fn malformed_and_missing_metadata_are_indexed_as_problems() -> Result<(), Error> {
    let (_root, paths, store) = setup();
    fs::create_dir_all(paths.library.join("skills/missing"))?;
    fs::create_dir_all(paths.library.join("mcp"))?;
    fs::write(paths.library.join("mcp/broken.json"), "{")?;
    indexer::full_reindex(&store, &paths)?;
    let rows = indexer::load(&store)?;
    assert!(
        rows.iter()
            .any(|row| row.problems_json.contains("Missing SKILL.md"))
    );
    assert!(
        rows.iter()
            .any(|row| row.problems_json.contains("Invalid JSON"))
    );
    skill(&paths, "missing", "now present");
    assert_eq!(indexer::catch_up(&store, &paths, None)?.updated, 1);
    fs::remove_file(paths.library.join("skills/missing/SKILL.md"))?;
    assert_eq!(indexer::catch_up(&store, &paths, None)?.updated, 1);
    Ok(())
}

#[test]
fn failed_reindex_does_not_commit_partial_changes() -> Result<(), Error> {
    let (_root, paths, store) = setup();
    skill(&paths, "one", "before");
    indexer::full_reindex(&store, &paths)?;
    let expected = indexer::load(&store)?;
    skill(&paths, "one", "after");
    fs::create_dir_all(paths.library.join("skills/z-failure/SKILL.md"))?;
    assert!(indexer::full_reindex(&store, &paths).is_err());
    assert_eq!(indexer::load(&store)?, expected);
    Ok(())
}

#[test]
fn unchanged_stamps_do_not_parse_item_files() -> Result<(), Error> {
    let (_root, paths, store) = setup();
    skill(&paths, "test", "before");
    indexer::full_reindex(&store, &paths)?;
    let expected = indexer::load(&store)?;
    let file_path = paths.library.join("skills/test/SKILL.md");
    let file = fs::OpenOptions::new().write(true).open(&file_path)?;
    let metadata = file.metadata()?;
    fs::write(&file_path, vec![0xff; metadata.len() as usize])?;
    file.set_modified(metadata.modified()?)?;
    assert_eq!(indexer::catch_up(&store, &paths, None)?.unchanged, 1);
    assert_eq!(indexer::load(&store)?, expected);
    Ok(())
}

#[cfg(unix)]
#[test]
fn large_files_and_external_symlinks_produce_warnings() -> Result<(), Error> {
    let (_root, paths, store) = setup();
    skill(&paths, "test", "before");
    let directory = paths.library.join("skills/test");
    fs::File::create(directory.join("large"))?.set_len(5 * 1024 * 1024 + 1)?;
    std::os::unix::fs::symlink(&paths.home, directory.join("outside"))?;
    indexer::full_reindex(&store, &paths)?;
    let row = indexer::load(&store)?.remove(0);
    assert!(row.problems_json.contains("File exceeds 5 MB"));
    assert!(row.problems_json.contains("Symlink points outside skill"));
    Ok(())
}

#[test]
#[ignore = "release-mode performance acceptance; run with --release -- --ignored --nocapture"]
fn catch_up_1000_unchanged_under_150_ms() -> Result<(), Error> {
    if cfg!(debug_assertions) {
        panic!("run this acceptance test with --release");
    }
    let (_root, paths, store) = setup();
    for index in 0..1000 {
        skill(&paths, &format!("skill-{index}"), "Performance fixture");
    }
    indexer::full_reindex(&store, &paths)?;
    let start = Instant::now();
    let counts = indexer::catch_up(&store, &paths, None)?;
    let elapsed = start.elapsed();
    println!(
        "catch_up 1000 unchanged items: {:.3} ms (budget 150 ms)",
        elapsed.as_secs_f64() * 1000.0
    );
    assert_eq!(counts.unchanged, 1000);
    assert_eq!(counts.inserted + counts.updated + counts.removed, 0);
    assert!(elapsed <= Duration::from_millis(150), "{elapsed:?}");
    Ok(())
}
