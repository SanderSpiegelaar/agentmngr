#![cfg(unix)]

use std::{fs, os::unix::fs::symlink, path::PathBuf, time::Instant};

use skm_core::{
    engine::{self, Action},
    global_skills::{plan_global_skill, set_global_skill},
    library::Library,
    paths::Paths,
    store::Store,
};

fn fixture() -> (tempfile::TempDir, Paths, Store, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
    let store = Store::open(&paths).unwrap();
    let home = root.join("user");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(paths.library.join("skills/review")).unwrap();
    fs::write(
        paths.library.join("skills/review/SKILL.md"),
        "review bytes\n",
    )
    .unwrap();
    (temp, paths, store, home)
}

#[test]
fn toggle_preserves_content_and_claude_tree_and_records_engine_operations() {
    let (_temp, paths, store, home) = fixture();
    let claude = home.join(".claude/skills");
    fs::create_dir_all(claude.join("review/scripts/empty")).unwrap();
    fs::write(claude.join("review/SKILL.md"), "Claude's own skill\0\n").unwrap();
    symlink("missing", claude.join("broken")).unwrap();
    let claude_before = engine::snapshot(&claude, &paths).unwrap();
    let source = paths.library.join("skills/review");
    let source_before = engine::snapshot(&source, &paths).unwrap();
    let target = home.join(".agents/skills/review");
    let manifest = paths.library.join("library.toml");
    fs::write(
        &manifest,
        "# preserved\nschema = 1\n[global]\n# selection\nskills = []\ncustom = 'keep'\n",
    )
    .unwrap();
    let on = set_global_skill(&paths, &store, &home, "review", true).unwrap();
    assert!(on.issues.is_empty() && on.failed.is_empty());
    assert!(matches!(on.done.as_slice(), [Action::Link { .. }]));
    assert_eq!(fs::read_link(&target).unwrap(), source);
    assert_eq!(
        Library::load(&manifest, &[]).unwrap().global.skills,
        ["review"]
    );
    assert!(
        fs::read_to_string(&manifest)
            .unwrap()
            .contains("custom = 'keep'")
    );
    assert_eq!(engine::snapshot(&claude, &paths).unwrap(), claude_before);
    assert!(
        set_global_skill(&paths, &store, &home, "review", true)
            .unwrap()
            .done
            .is_empty()
    );
    let off = set_global_skill(&paths, &store, &home, "review", false).unwrap();
    assert!(off.issues.is_empty() && off.failed.is_empty());
    assert!(matches!(off.done.as_slice(), [Action::Trash { .. }]));
    assert!(fs::symlink_metadata(&target).is_err());
    assert!(
        Library::load(&manifest, &[])
            .unwrap()
            .global
            .skills
            .is_empty()
    );
    assert_eq!(engine::snapshot(&source, &paths).unwrap(), source_before);
    assert_eq!(engine::snapshot(&claude, &paths).unwrap(), claude_before);
    store.read(|connection| {
        let (ops, ledger): (i64, i64) = connection.query_row(
            "SELECT (SELECT count(*) FROM ops WHERE inverse_json != '[]'), (SELECT count(*) FROM ledger)",
            [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        assert_eq!((ops, ledger), (2, 0));
        Ok(())
    }).unwrap();
    assert!(paths.home.join(".changed").is_file());
    println!(
        "on/off: managed link created/trashed; library content and fake ~/.claude/skills snapshot unchanged; inverse ops and ledger verified"
    );
}

#[test]
fn foreign_entries_have_issues_and_remain_byte_identical_on_and_off() {
    for kind in ["file", "directory", "link"] {
        let (_temp, paths, store, home) = fixture();
        let target = home.join(".agents/skills/review");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        match kind {
            "file" => fs::write(&target, "foreign\0bytes\n").unwrap(),
            "directory" => {
                fs::create_dir_all(target.join("empty")).unwrap();
                fs::write(target.join("SKILL.md"), "foreign directory bytes\n").unwrap();
            }
            _ => symlink("foreign-missing", &target).unwrap(),
        }
        let before = engine::snapshot(&target, &paths).unwrap();
        for on in [true, false] {
            let plan = plan_global_skill(&paths, &store, &home, "review", on).unwrap();
            assert!(plan.actions.is_empty());
            assert_eq!(plan.issues.len(), 1);
            assert_eq!(plan.issues[0].code, "E_FOREIGN_TARGET");
            let result = set_global_skill(&paths, &store, &home, "review", on).unwrap();
            assert!(result.done.is_empty() && result.failed.is_empty());
            assert_eq!(result.issues.len(), 1);
            assert_eq!(engine::snapshot(&target, &paths).unwrap(), before);
        }
        println!(
            "foreign {kind}: on/off each produce one PlanIssue, zero actions; exact byte/link/tree fingerprint unchanged"
        );
    }
}

#[test]
fn off_refuses_even_a_ledger_owned_real_file() {
    let (_temp, paths, store, home) = fixture();
    set_global_skill(&paths, &store, &home, "review", false).unwrap();
    let target = home.join(".agents/skills/review");
    let scope_id = store
        .read(|connection| {
            Ok(
                connection.query_row("SELECT id FROM scopes WHERE kind='global'", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();
    let desired = engine::DesiredTarget {
        target: target.clone(),
        item: skm_core::target::ItemRef {
            kind: skm_core::target::ItemKind::Skill,
            name: "review".into(),
        },
        agent_id: None,
        content: engine::DesiredContent::File {
            bytes: b"owned real file".to_vec(),
        },
        approve_modified: false,
    };
    let observed = engine::observe(&paths, &store, std::slice::from_ref(&target)).unwrap();
    let plan = engine::plan(scope_id, &[desired], &observed);
    let result = engine::apply(&paths, &store, &plan, "test", None).unwrap();
    assert_eq!(result.done.len(), 1);
    let result = set_global_skill(&paths, &store, &home, "review", false).unwrap();
    assert!(result.done.is_empty());
    assert_eq!(result.issues.len(), 1);
    assert_eq!(fs::read(&target).unwrap(), b"owned real file");
    println!("off: ledger-owned real file refused, only OurLink is removable");
}

#[test]
fn preview_is_read_only_and_scoped_to_the_toggled_skill() {
    let (_temp, paths, store, home) = fixture();
    fs::create_dir_all(paths.library.join("skills/other")).unwrap();
    let manifest = paths.library.join("library.toml");
    let text = "schema = 1\n[global]\nskills = ['other']\n";
    fs::write(&manifest, text).unwrap();
    let plan = plan_global_skill(&paths, &store, &home, "review", true).unwrap();
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(fs::read_to_string(&manifest).unwrap(), text);
    assert!(!home.join(".agents").exists());
    assert!(!paths.home.join(".changed").exists());
    store
        .read(|connection| {
            let count: i64 =
                connection.query_row("SELECT count(*) FROM scopes", [], |row| row.get(0))?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    let result = set_global_skill(&paths, &store, &home, "review", true).unwrap();
    assert_eq!(result.done.len(), 1);
    assert!(!home.join(".agents/skills/other").exists());
    assert!(result.failed.is_empty() && result.issues.is_empty());
    println!(
        "preview: only toggled link planned; manifest, targets, scope table and activity untouched"
    );
}

#[test]
fn refused_on_does_not_leak_into_unrelated_toggles() {
    let (_temp, paths, store, home) = fixture();
    fs::create_dir_all(paths.library.join("skills/taken")).unwrap();
    let taken = home.join(".agents/skills/taken");
    fs::create_dir_all(&taken).unwrap();
    fs::write(taken.join("SKILL.md"), "foreign bytes").unwrap();
    let before = engine::snapshot(&taken, &paths).unwrap();
    let refused = set_global_skill(&paths, &store, &home, "taken", true).unwrap();
    assert_eq!(refused.issues[0].code, "E_FOREIGN_TARGET");
    for on in [true, false] {
        let plan = plan_global_skill(&paths, &store, &home, "review", on).unwrap();
        assert!(plan.issues.is_empty(), "{:?}", plan.issues);
        let result = set_global_skill(&paths, &store, &home, "review", on).unwrap();
        assert!(result.issues.is_empty() && result.failed.is_empty());
    }
    assert_eq!(engine::snapshot(&taken, &paths).unwrap(), before);
    assert_eq!(
        Library::load(&paths.library.join("library.toml"), &[])
            .unwrap()
            .global
            .skills,
        ["taken"]
    );
}

#[test]
fn symlinked_manager_and_user_home_support_toggle_and_undo() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let alias = root.join("alias");
    symlink(&root, &alias).unwrap();
    let mut paths = Paths::resolve(Some(&alias.join("manager")), None, None).unwrap();
    paths.ensure_layout().unwrap();
    assert_eq!(paths.home, root.join("manager"));
    let store = Store::open(&paths).unwrap();
    fs::create_dir(root.join("user")).unwrap();
    let home = alias.join("user");
    let source = paths.library.join("skills/review");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("SKILL.md"), "original bytes").unwrap();
    for on in [true, false] {
        let result = set_global_skill(&paths, &store, &home, "review", on).unwrap();
        assert_eq!(result.done.len(), 1, "{:?}", result.issues);
        assert!(result.failed.is_empty() && result.issues.is_empty());
    }
    let result = engine::undo(&paths, &store, None, "test", None).unwrap();
    assert_eq!(result.done.len(), 1);
    assert!(result.failed.is_empty() && result.issues.is_empty());
    let target = home.join(".agents/skills/review");
    assert!(matches!(
        skm_core::target::classify(&target, &paths).unwrap(),
        skm_core::target::TargetClass::OurLink(_)
    ));
    assert_eq!(
        fs::read(target.join("SKILL.md")).unwrap(),
        b"original bytes"
    );
}

#[test]
fn unsafe_names_and_missing_skills_are_rejected_and_symlinked_parents_refuse() {
    let (_temp, paths, store, home) = fixture();
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "/absolute",
        "a/b",
        "a\\b",
        "missing",
    ] {
        assert_eq!(
            set_global_skill(&paths, &store, &home, name, true)
                .unwrap_err()
                .code(),
            "E_INVALID_SKILL"
        );
    }
    let claude = home.join(".claude/skills");
    fs::create_dir_all(&claude).unwrap();
    fs::create_dir(home.join(".agents")).unwrap();
    symlink(&claude, home.join(".agents/skills")).unwrap();
    let before = engine::snapshot(&claude, &paths).unwrap();
    let result = set_global_skill(&paths, &store, &home, "review", true).unwrap();
    assert!(result.done.is_empty());
    assert_eq!(result.issues.len(), 1);
    assert_eq!(engine::snapshot(&claude, &paths).unwrap(), before);
}

#[test]
#[ignore = "release end-to-end global skill toggle timing"]
fn release_toggle_latency() {
    if cfg!(debug_assertions) {
        panic!("run this test with --release");
    }
    let (_temp, paths, store, home) = fixture();
    for on in [true, false] {
        let start = Instant::now();
        let result = set_global_skill(&paths, &store, &home, "review", on).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(result.done.len(), 1);
        assert!(result.issues.is_empty() && result.failed.is_empty());
        let target = home.join(".agents/skills/review");
        assert_eq!(fs::symlink_metadata(&target).is_ok(), on);
        println!(
            "set_global_skill(on={on}) end-to-end: {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(elapsed.as_millis() < 50, "toggle took {elapsed:?}");
    }
}
