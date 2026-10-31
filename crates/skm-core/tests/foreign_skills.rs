#![cfg(unix)]

use skm_core::{
    content_hash::content_hash,
    engine::{self, Action, ApplyResult},
    foreign_skills::{adopt, list_foreign, plan_adopt},
    library::{Library, SkillSource},
    paths::Paths,
    store::Store,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

struct Fixture {
    _temp: tempfile::TempDir,
    paths: Paths,
    store: Store,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
        paths.ensure_layout().unwrap();
        let store = Store::open(&paths).unwrap();
        store.write(|connection| {
            connection.execute("INSERT INTO scopes(kind,name,mode,added_at) VALUES('global','Global','link',0)", [])?;
            Ok(())
        }).unwrap();
        let home = root.join("user");
        let project = root.join("project");
        for root in [&home, &project] {
            fs::create_dir(root).unwrap();
        }
        Self {
            _temp: temp,
            paths,
            store,
            home,
            project,
        }
    }

    fn skill(&self, root: &Path, name: &str) -> PathBuf {
        let target = root.join(".agents/skills").join(name);
        fs::create_dir_all(target.join("empty")).unwrap();
        fs::create_dir_all(target.join(".git")).unwrap();
        fs::create_dir_all(target.join("node_modules")).unwrap();
        fs::write(target.join("SKILL.md"), b"skill bytes\0\n").unwrap();
        fs::write(target.join(".git/keep"), b"ignored by npx but preserved").unwrap();
        fs::write(target.join("node_modules/keep"), b"also preserved").unwrap();
        fs::write(target.join("run"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(target.join("run"), fs::Permissions::from_mode(0o751)).unwrap();
        symlink("SKILL.md", target.join("alias")).unwrap();
        target
    }

    fn names(&self) -> Vec<String> {
        vec!["demo".into()]
    }
    fn content_tree(&self) -> Vec<engine::TargetState> {
        [
            &self.home,
            &self.project,
            &self.paths.library,
            &self.paths.trash,
            &self.paths.cache,
        ]
        .into_iter()
        .map(|root| engine::snapshot(root, &self.paths).unwrap())
        .collect()
    }
    fn library(&self) -> Library {
        Library::load(&self.paths.library.join("library.toml"), &[]).unwrap()
    }
    fn assert_ok(&self, result: &ApplyResult, count: usize) {
        assert!(
            result.issues.is_empty() && result.failed.is_empty(),
            "{result:?}"
        );
        assert_eq!(result.done.len(), count, "{result:?}");
    }
}

#[test]
fn listing_and_planning_are_byte_identical_and_discover_all_foreign_classes() {
    let f = Fixture::new();
    f.skill(&f.home, "demo");
    let directory = f.home.join(".agents/skills");
    fs::write(directory.join("file"), "foreign file").unwrap();
    symlink("missing", directory.join("broken")).unwrap();
    symlink(f.paths.library.join("skills/ours"), directory.join("ours")).unwrap();
    let before = f.content_tree();
    let entries = list_foreign(&f.paths, &f.store, &f.home, None).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["broken", "demo", "file"]
    );
    let plan = plan_adopt(&f.paths, &f.store, &f.home, &f.names(), None).unwrap();
    assert_eq!(plan.actions.len(), 3);
    assert!(plan.issues.is_empty());
    assert_eq!(f.content_tree(), before);
    f.store.read(|connection| {
        let counts: (i64, i64, i64) = connection.query_row("SELECT (SELECT count(*) FROM scopes), (SELECT count(*) FROM ledger), (SELECT count(*) FROM ops)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        assert_eq!(counts, (1, 0, 0));
        Ok(())
    }).unwrap();
    println!("listing/planning: fake HOME, project, library, cache and trash byte-identical");
}

#[test]
fn global_adopt_verifies_bytes_permissions_provenance_and_undo_redo() {
    let f = Fixture::new();
    let target = f.skill(&f.home, "demo");
    // Exact v3 shape: vercel-labs/skills src/skill-lock.ts at
    // 92a9a34ac3bf88e695e468b90c7bf32f163d32c8. skillFolderHash is a tree SHA, not a commit or computedHash.
    let lock = r#"{"version":3,"skills":{"demo":{"source":"acme/skills","sourceType":"github","sourceUrl":"https://github.com/acme/skills.git","ref":"release","skillPath":"skills/demo/SKILL.md","skillFolderHash":"tree-sha-not-revision","installedAt":"2026-10-01T12:00:00Z","updatedAt":"2026-10-02T12:00:00Z","installMode":"copy"}},"dismissed":{},"lastSelectedAgents":["codex"]}"#;
    fs::write(f.home.join(".agents/.skill-lock.json"), lock).unwrap();
    let manifest = f.paths.library.join("library.toml");
    let original_manifest = "# keep me\nschema = 1\n[global]\nskills = []\nunknown = 'stay'\n";
    fs::write(&manifest, original_manifest).unwrap();
    let before = engine::snapshot(&target, &f.paths).unwrap();
    let hash = content_hash(&target).unwrap();
    let found = list_foreign(&f.paths, &f.store, &f.home, None).unwrap();
    assert!(
        matches!(&found[0].provenance.as_ref().unwrap().source, SkillSource::Git { url, reference: Some(reference), path: Some(path) } if url == "https://github.com/acme/skills.git" && reference == "release" && path == "skills/demo")
    );
    let result = adopt(&f.paths, &f.store, &f.home, &f.names(), None).unwrap();
    f.assert_ok(&result, 3);
    let imported = f.paths.library.join("skills/demo");
    assert!(target.is_symlink());
    assert_eq!(content_hash(&imported).unwrap(), hash);
    assert_eq!(content_hash(&target).unwrap(), hash);
    assert_eq!(engine::snapshot(&imported, &f.paths).unwrap(), before);
    assert_eq!(
        fs::metadata(imported.join("run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    let library = f.library();
    assert_eq!(library.global.skills, ["demo"]);
    assert_eq!(library.skills["demo"].base_hash.as_ref(), Some(&hash));
    assert!(library.skills["demo"].rev.is_none());
    assert!(library.skills["demo"].installed_at.is_some());
    assert!(
        fs::read_to_string(&manifest)
            .unwrap()
            .contains("unknown = 'stay'")
    );
    assert_eq!(
        fs::read_to_string(f.home.join(".agents/.skill-lock.json")).unwrap(),
        lock
    );
    assert!(
        list_foreign(&f.paths, &f.store, &f.home, None)
            .unwrap()
            .is_empty()
    );
    let undone = engine::undo(&f.paths, &f.store, None, "test", None).unwrap();
    f.assert_ok(&undone, 3);
    assert_eq!(engine::snapshot(&target, &f.paths).unwrap(), before);
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);
    assert!(!imported.exists());
    let redone = engine::undo(&f.paths, &f.store, None, "test", None).unwrap();
    f.assert_ok(&redone, 3);
    assert!(target.is_symlink());
    println!(
        "global adopt: original == library == link computedHash {hash}; exact snapshot + mode verified; undo/redo: 3 effects each"
    );
}

#[test]
fn project_adopt_keeps_copy_and_lock_unchanged_and_undo_releases_ownership() {
    let f = Fixture::new();
    let target = f.skill(&f.project, "demo");
    // Exact v1 shape: vercel-labs/skills src/local-lock.ts at
    // 92a9a34ac3bf88e695e468b90c7bf32f163d32c8. Local source paths are relative to the project, not HOME.
    let lock = r#"{"version":1,"skills":{"demo":{"source":"./source/demo","sourceType":"local","computedHash":"old-lock-hash"},"foreign":{"source":"other/repo","sourceType":"github","computedHash":"leave-me"}},"unknown":"preserve"}"#;
    fs::write(f.project.join("skills-lock.json"), lock).unwrap();
    let before = engine::snapshot(&f.project, &f.paths).unwrap();
    assert_eq!(
        list_foreign(&f.paths, &f.store, &f.home, Some(&f.project))
            .unwrap()
            .len(),
        1
    );
    let result = adopt(&f.paths, &f.store, &f.home, &f.names(), Some(&f.project)).unwrap();
    f.assert_ok(&result, 3);
    assert!(matches!(result.done[1], Action::Claim { .. }));
    assert!(!target.is_symlink());
    assert_eq!(engine::snapshot(&f.project, &f.paths).unwrap(), before);
    assert_eq!(
        content_hash(&target).unwrap(),
        content_hash(&f.paths.library.join("skills/demo")).unwrap()
    );
    assert!(
        list_foreign(&f.paths, &f.store, &f.home, Some(&f.project))
            .unwrap()
            .is_empty()
    );
    assert!(
        matches!(&f.library().skills["demo"].source, SkillSource::Local { origin: Some(origin) } if origin == &f.project.join("./source/demo"))
    );
    assert!(f.library().global.skills.is_empty());
    let undone = engine::undo(&f.paths, &f.store, None, "test", None).unwrap();
    f.assert_ok(&undone, 3);
    assert_eq!(engine::snapshot(&f.project, &f.paths).unwrap(), before);
    assert_eq!(
        list_foreign(&f.paths, &f.store, &f.home, Some(&f.project))
            .unwrap()
            .len(),
        1
    );
    assert!(!f.paths.library.join("skills/demo").exists());
    f.assert_ok(
        &engine::undo(&f.paths, &f.store, None, "test", None).unwrap(),
        3,
    );
    println!(
        "project adopt: copy/lock byte-identical, ledger claimed; undo releases claim without editing project"
    );
}

#[test]
fn foreign_edits_since_plan_and_rebinding_authorization_are_refused() {
    let f = Fixture::new();
    let target = f.skill(&f.home, "demo");
    let plan = plan_adopt(&f.paths, &f.store, &f.home, &f.names(), None).unwrap();
    fs::write(target.join("SKILL.md"), "edited after preview").unwrap();
    let before = engine::snapshot(&target, &f.paths).unwrap();
    let result = engine::apply(&f.paths, &f.store, &plan, "test", None).unwrap();
    assert!(result.done.is_empty());
    assert_eq!(result.issues.len(), 3);
    assert!(!f.paths.library.join("skills/demo").exists());
    assert_eq!(engine::snapshot(&target, &f.paths).unwrap(), before);
    let mut tampered = plan.clone();
    tampered.actions = vec![plan.actions[1].clone()];
    tampered.actions[0].before = engine::observe(&f.paths, &f.store, std::slice::from_ref(&target))
        .unwrap()
        .remove(0);
    tampered.actions[0].requires.clear();
    let result = engine::apply(&f.paths, &f.store, &tampered, "test", None).unwrap();
    assert!(result.done.is_empty());
    assert_eq!(result.issues[0].code, "E_FOREIGN_TARGET");
    tampered.actions[0].adoption = None;
    tampered.actions[0].approve_modified = true;
    let result = engine::apply(&f.paths, &f.store, &tampered, "test", None).unwrap();
    assert!(result.done.is_empty());
    assert_eq!(result.issues[0].code, "E_FOREIGN_TARGET");
    println!(
        "stale adopt: zero effects; rebinding before or approve_modified cannot bypass snapshot authorization"
    );
}

#[test]
fn failed_hash_verification_cannot_replace_original_or_write_provenance() {
    let f = Fixture::new();
    let target = f.skill(&f.home, "demo");
    let before = engine::snapshot(&target, &f.paths).unwrap();
    let mut plan = plan_adopt(&f.paths, &f.store, &f.home, &f.names(), None).unwrap();
    if let Action::CopyTree { computed_hash, .. } = &mut plan.actions[0].action {
        *computed_hash = Some("wrong hash".into());
    }
    let result = engine::apply(&f.paths, &f.store, &plan, "test", None).unwrap();
    assert!(result.done.is_empty());
    assert_eq!(result.issues.len(), 3);
    assert_eq!(engine::snapshot(&target, &f.paths).unwrap(), before);
    assert!(!f.paths.library.join("skills/demo").exists());
    assert!(!f.paths.library.join("library.toml").exists());
}

#[test]
fn collision_refuses_all_skill_actions_and_identical_library_content_is_reused() {
    let f = Fixture::new();
    let target = f.skill(&f.home, "demo");
    let imported = f.paths.library.join("skills/demo");
    fs::create_dir_all(&imported).unwrap();
    fs::write(imported.join("SKILL.md"), "different").unwrap();
    let before = f.content_tree();
    let plan = plan_adopt(&f.paths, &f.store, &f.home, &f.names(), None).unwrap();
    assert!(plan.actions.is_empty());
    assert_eq!(plan.issues[0].code, "E_NAME_COLLISION");
    assert_eq!(f.content_tree(), before);
    // A second name with identical existing content is reused without claiming or deleting the library copy.
    let other = f.skill(&f.home, "same");
    let same = f.skill(&f.paths.library, "unused");
    fs::rename(&same, f.paths.library.join("skills/same")).unwrap();
    let before_same = engine::snapshot(&other, &f.paths).unwrap();
    let result = adopt(&f.paths, &f.store, &f.home, &["same".into()], None).unwrap();
    f.assert_ok(&result, 2);
    f.assert_ok(
        &engine::undo(&f.paths, &f.store, None, "test", None).unwrap(),
        2,
    );
    assert!(f.paths.library.join("skills/same").is_dir());
    assert_eq!(engine::snapshot(&other, &f.paths).unwrap(), before_same);
    assert!(!target.is_symlink());
}

#[test]
fn foreign_symlink_is_copied_and_undo_restores_its_exact_raw_destination() {
    let f = Fixture::new();
    let source = f.skill(&f.project, "source");
    fs::create_dir_all(f.home.join(".agents/skills")).unwrap();
    let target = f.home.join(".agents/skills/demo");
    let raw = Path::new("../../../project/.agents/skills/source");
    symlink(raw, &target).unwrap();
    let source_before = engine::snapshot(&source, &f.paths).unwrap();
    let result = adopt(&f.paths, &f.store, &f.home, &f.names(), None).unwrap();
    f.assert_ok(&result, 3);
    assert_eq!(
        engine::snapshot(&f.paths.library.join("skills/demo"), &f.paths).unwrap(),
        source_before
    );
    f.assert_ok(
        &engine::undo(&f.paths, &f.store, None, "test", None).unwrap(),
        3,
    );
    assert_eq!(fs::read_link(&target).unwrap(), raw);
    assert_eq!(engine::snapshot(&source, &f.paths).unwrap(), source_before);
}

#[test]
fn broken_foreign_symlinks_invalid_locks_and_unsafe_names_never_mutate_content() {
    let f = Fixture::new();
    f.skill(&f.home, "demo");
    for text in ["not json", r#"{"version":2,"skills":{}}"#] {
        fs::write(f.home.join(".agents/.skill-lock.json"), text).unwrap();
        let before = engine::snapshot(&f.home, &f.paths).unwrap();
        assert_eq!(
            list_foreign(&f.paths, &f.store, &f.home, None)
                .unwrap_err()
                .code(),
            "E_CONFIG_PARSE"
        );
        assert_eq!(engine::snapshot(&f.home, &f.paths).unwrap(), before);
    }
    fs::remove_file(f.home.join(".agents/.skill-lock.json")).unwrap();
    assert_eq!(
        plan_adopt(&f.paths, &f.store, &f.home, &["../escape".into()], None)
            .unwrap_err()
            .code(),
        "E_INVALID_SKILL"
    );
    symlink("missing", f.home.join(".agents/skills/broken")).unwrap();
    assert_eq!(
        list_foreign(&f.paths, &f.store, &f.home, None)
            .unwrap()
            .len(),
        2
    );
    let broken = plan_adopt(&f.paths, &f.store, &f.home, &["broken".into()], None).unwrap();
    assert!(broken.actions.is_empty());
    assert_eq!(broken.issues[0].code, "E_INVALID_SKILL");
}

#[test]
fn shared_manifest_undo_restores_previous_scope_ownership() {
    let f = Fixture::new();
    f.skill(&f.home, "global");
    f.assert_ok(
        &adopt(&f.paths, &f.store, &f.home, &["global".into()], None).unwrap(),
        3,
    );
    let manifest = f.paths.library.join("library.toml");
    let before = engine::observe(&f.paths, &f.store, std::slice::from_ref(&manifest)).unwrap();
    f.skill(&f.project, "project");
    f.assert_ok(
        &adopt(
            &f.paths,
            &f.store,
            &f.home,
            &["project".into()],
            Some(&f.project),
        )
        .unwrap(),
        3,
    );
    f.assert_ok(
        &engine::undo(&f.paths, &f.store, None, "test", None).unwrap(),
        3,
    );
    assert_eq!(
        engine::observe(&f.paths, &f.store, std::slice::from_ref(&manifest)).unwrap(),
        before
    );
    f.assert_ok(
        &engine::undo(&f.paths, &f.store, None, "test", None).unwrap(),
        3,
    );
}

#[test]
fn project_github_and_provider_provenance_survive_library_round_trip() {
    let f = Fixture::new();
    f.skill(&f.project, "git");
    f.skill(&f.project, "npm");
    fs::write(f.project.join("skills-lock.json"), r#"{"version":1,"skills":{"git":{"source":"acme/repo","sourceType":"github","ref":"stable","skillPath":"skills/git/SKILL.md","computedHash":"abc"},"npm":{"source":"@acme/skills","sourceType":"node_modules","skillPath":"skills/npm/SKILL.md","version":"1.2.3","computedHash":"def"}}}"#).unwrap();
    let entries = list_foreign(&f.paths, &f.store, &f.home, Some(&f.project)).unwrap();
    assert!(
        matches!(&entries[0].provenance.as_ref().unwrap().source, SkillSource::Git {url, reference: Some(reference), path: Some(path)} if url == "https://github.com/acme/repo" && reference == "stable" && path == "skills/git")
    );
    assert!(
        matches!(&entries[1].provenance.as_ref().unwrap().source, SkillSource::Npx {source_type, ..} if source_type == "node_modules")
    );
    f.assert_ok(
        &adopt(
            &f.paths,
            &f.store,
            &f.home,
            &["git".into(), "npm".into()],
            Some(&f.project),
        )
        .unwrap(),
        5,
    );
    assert_eq!(
        f.library().skills["npm"].source,
        entries[1].provenance.as_ref().unwrap().source
    );
}

#[test]
fn edited_adopted_content_blocks_whole_undo_before_any_effect() {
    let f = Fixture::new();
    let target = f.skill(&f.home, "demo");
    f.assert_ok(
        &adopt(&f.paths, &f.store, &f.home, &f.names(), None).unwrap(),
        3,
    );
    fs::write(target.join("SKILL.md"), "user edit after adopt").unwrap();
    let before = f.content_tree();
    let ops = engine::list_ops(&f.store).unwrap();
    assert_eq!(
        engine::undo(&f.paths, &f.store, None, "test", None)
            .unwrap_err()
            .code(),
        "E_CONFIG_CHANGED"
    );
    assert_eq!(f.content_tree(), before);
    assert_eq!(engine::list_ops(&f.store).unwrap(), ops);
}
