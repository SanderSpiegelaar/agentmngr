#![cfg(unix)]

use std::{
    fs,
    os::unix::{fs::symlink, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use proptest::prelude::*;
use skm_core::{
    engine::{self, Action, DesiredContent, DesiredTarget, Observation, Plan},
    events::{self, CoreEvent},
    paths::Paths,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        // macOS's /var is itself a symlink; target paths use their physical root.
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths::resolve(Some(&root.join("home")), None, None).unwrap();
        let store = Store::open(&paths).unwrap();
        add_scope(&store);
        let source = paths.library.join("skills/review");
        fs::create_dir_all(source.join("scripts")).unwrap();
        fs::write(source.join("SKILL.md"), b"review skill").unwrap();
        fs::write(source.join("scripts/check"), b"#!/bin/sh\n").unwrap();
        symlink("SKILL.md", source.join("alias")).unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        Self {
            _temp: temp,
            root,
            paths,
            store,
        }
    }

    fn desired(&self, target: PathBuf, mode: u8) -> DesiredTarget {
        let source = self.paths.library.join("skills/review");
        DesiredTarget {
            target,
            item: item(),
            agent_id: None,
            content: match mode % 4 {
                0 => DesiredContent::Absent,
                1 => DesiredContent::Link { to: source },
                2 => DesiredContent::File {
                    bytes: b"complete rendered content".to_vec(),
                },
                _ => DesiredContent::Tree {
                    hash: engine::snapshot(&source, &self.paths)
                        .unwrap()
                        .hash
                        .unwrap(),
                    from: source,
                },
            },
            approve_modified: false,
        }
    }

    fn plan(&self, desired: &[DesiredTarget]) -> Plan {
        let targets: Vec<_> = desired
            .iter()
            .map(|request| request.target.clone())
            .collect();
        engine::plan(
            1,
            desired,
            &engine::observe(&self.paths, &self.store, &targets).unwrap(),
        )
    }

    fn apply(&self, plan: &Plan) -> engine::ApplyResult {
        engine::apply(&self.paths, &self.store, plan, "test", None).unwrap()
    }
}

fn item() -> ItemRef {
    ItemRef {
        kind: ItemKind::Skill,
        name: "review".into(),
    }
}

fn add_scope(store: &Store) {
    store
        .write(|connection| {
            connection.execute(
                "INSERT INTO scopes(id,kind,name,added_at) VALUES(1,'global','Global',0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/engine.proptest-regressions")))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn foreign_trees_are_never_planned_or_applied(entries in prop::collection::vec((0u8..5, 0u8..4, any::<u64>()), 1..24)) {
        let fixture = Fixture::new();
        let mut desired = Vec::new();
        let mut foreign = Vec::new();
        for (index, (class, mode, payload)) in entries.into_iter().enumerate() {
            let target = fixture.root.join(format!("target-{index}"));
            match class {
                0 => {},
                1 => symlink(fixture.paths.library.join("skills/review"), &target).unwrap(),
                2 => symlink(format!("foreign-{payload}"), &target).unwrap(),
                3 => {
                    fs::create_dir_all(target.join("nested/empty")).unwrap();
                    fs::write(target.join("nested/user-file"), payload.to_le_bytes()).unwrap();
                    symlink("user-file", target.join("nested/user-link")).unwrap();
                },
                _ => fs::write(&target, payload.to_le_bytes()).unwrap(),
            }
            if class >= 2 {
                foreign.push((target.clone(), engine::snapshot(&target, &fixture.paths).unwrap()));
            }
            desired.push(fixture.desired(target, mode));
        }
        let plan = fixture.plan(&desired);
        for action in &plan.actions {
            prop_assert!(!foreign.iter().any(|(target, _)| target == action.action.target()));
        }
        let result = fixture.apply(&plan);
        prop_assert!(result.failed.is_empty(), "{:?}", result.failed);
        for action in &result.done {
            prop_assert!(!foreign.iter().any(|(target, _)| target == action.target()));
        }
        for (target, before) in foreign {
            prop_assert_eq!(engine::snapshot(&target, &fixture.paths).unwrap(), before);
        }
        prop_assert!(fixture.plan(&desired).actions.is_empty());
    }

    #[test]
    fn apply_then_replan_is_empty(modes in prop::collection::vec(0u8..4, 1..20)) {
        let fixture = Fixture::new();
        let desired: Vec<_> = modes.into_iter().enumerate().map(|(index, mode)| fixture.desired(fixture.root.join(format!("target-{index}")), mode)).collect();
        let result = fixture.apply(&fixture.plan(&desired));
        prop_assert!(result.issues.is_empty(), "{:?}", result.issues);
        prop_assert!(result.failed.is_empty(), "{:?}", result.failed);
        let second = fixture.plan(&desired);
        prop_assert!(second.actions.is_empty());
        prop_assert!(second.issues.is_empty());
    }
}

#[test]
fn changed_targets_and_symlink_ancestors_are_refused() {
    let fixture = Fixture::new();
    let target = fixture.root.join("target");
    let plan = fixture.plan(&[fixture.desired(target.clone(), 1)]);
    fs::write(&target, b"user data").unwrap();
    let result = fixture.apply(&plan);
    assert!(result.done.is_empty());
    assert_eq!(result.issues[0].code, "E_CONFIG_CHANGED");
    assert_eq!(fs::read(&target).unwrap(), b"user data");
    let external = fixture.root.join("external");
    fs::create_dir(&external).unwrap();
    symlink(&external, fixture.root.join("alias")).unwrap();
    let unsafe_target = fixture.root.join("alias/untouched");
    let plan = fixture.plan(&[fixture.desired(unsafe_target, 2)]);
    assert!(plan.actions.is_empty());
    assert_eq!(plan.issues[0].code, "E_FOREIGN_TARGET");
    assert!(!external.join("untouched").exists());
}

#[test]
fn ledger_trash_inverse_and_events_cover_on_update_off() {
    let fixture = Fixture::new();
    let target = fixture.root.join("target");
    for mode in [1, 2, 3, 0] {
        let before = engine::snapshot(&target, &fixture.paths).unwrap();
        let plan = fixture.plan(&[fixture.desired(target.clone(), mode)]);
        let (sender, receiver) = events::channel();
        let applied =
            engine::apply(&fixture.paths, &fixture.store, &plan, "test", Some(&sender)).unwrap();
        assert_eq!(
            applied.done.len(),
            1,
            "{:?} {:?}",
            applied.issues,
            applied.failed
        );
        assert!(matches!(
            receiver.recv().unwrap(),
            CoreEvent::ItemsChanged { .. }
        ));
        assert!(matches!(receiver.recv().unwrap(), CoreEvent::ApplyDone));
        assert!(fixture.paths.home.join(".changed").exists());
        let stored: Vec<engine::PlannedAction> = fixture
            .store
            .read(|connection| {
                let inverse: String = connection.query_row(
                    "SELECT inverse_json FROM ops WHERE id=?1",
                    [applied.op_id],
                    |row| row.get(0),
                )?;
                Ok(serde_json::from_str(&inverse).unwrap())
            })
            .unwrap();
        assert_eq!(stored, applied.inverse);
        let undo = Plan {
            scope_id: 1,
            actions: stored,
            issues: vec![],
            summary: "inverse".into(),
        };
        let undone = fixture.apply(&undo);
        assert_eq!(
            undone.done.len(),
            1,
            "{:?} {:?}",
            undone.issues,
            undone.failed
        );
        assert_eq!(engine::snapshot(&target, &fixture.paths).unwrap(), before);
        let reapplied = fixture.apply(&fixture.plan(&[fixture.desired(target.clone(), mode)]));
        assert_eq!(reapplied.done.len(), 1);
        let observed = engine::observe(
            &fixture.paths,
            &fixture.store,
            std::slice::from_ref(&target),
        )
        .unwrap()
        .remove(0);
        if mode != 0 {
            assert_eq!(
                observed.ledger.unwrap().written_hash,
                observed.state.hash.unwrap()
            );
        } else {
            assert!(observed.ledger.is_none());
        }
    }
}

#[test]
fn modified_owned_files_require_approval_and_inverse_refuses_edits() {
    let fixture = Fixture::new();
    let target = fixture.root.join("rendered");
    fixture.apply(&fixture.plan(&[fixture.desired(target.clone(), 2)]));
    fs::write(&target, b"user edit").unwrap();
    let mut desired = fixture.desired(target.clone(), 2);
    let refused = fixture.plan(&[desired.clone()]);
    assert!(refused.actions.is_empty());
    assert_eq!(refused.issues[0].code, "E_PROJECT_EDITED");
    desired.approve_modified = true;
    let applied = fixture.apply(&fixture.plan(&[desired]));
    assert_eq!(applied.done.len(), 1);
    let inverse = Plan {
        scope_id: 1,
        actions: applied.inverse,
        ..Plan::default()
    };
    fs::write(&target, b"changed after apply").unwrap();
    let refused = fixture.apply(&inverse);
    assert!(refused.done.is_empty());
    assert_eq!(refused.issues[0].code, "E_CONFIG_CHANGED");
}

#[test]
fn shared_destinations_deduplicate_and_conflicts_refuse() {
    let fixture = Fixture::new();
    let desired = fixture.desired(fixture.root.join("shared"), 2);
    assert_eq!(
        fixture
            .plan(&[desired.clone(), desired.clone()])
            .actions
            .len(),
        1
    );
    let mut other = desired.clone();
    other.content = DesiredContent::File {
        bytes: b"different dialect".to_vec(),
    };
    let conflict = fixture.plan(&[desired, other]);
    assert!(conflict.actions.is_empty());
    assert_eq!(conflict.issues[0].code, "E_CONFIG_CHANGED");
}

#[test]
fn source_changes_and_partial_failure_are_recorded() {
    let fixture = Fixture::new();
    let desired = [
        fixture.desired(fixture.root.join("a-tree"), 3),
        fixture.desired(fixture.root.join("b-file"), 2),
    ];
    let plan = fixture.plan(&desired);
    fs::write(
        fixture.paths.library.join("skills/review/SKILL.md"),
        "changed",
    )
    .unwrap();
    let result = fixture.apply(&plan);
    assert_eq!(result.done.len(), 1);
    assert_eq!(result.issues[0].code, "E_CONFIG_CHANGED");
    assert!(!fixture.root.join("a-tree").exists());
    assert_eq!(result.inverse.len(), 1);
    let result = fixture
        .store
        .read(|connection| {
            Ok(connection.query_row(
                "SELECT result FROM ops WHERE id=?1",
                [result.op_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert_eq!(result, "issues");
}

const KILL_COUNT: usize = 96;
const PAYLOAD_SIZE: usize = 128 * 1024;

fn kill_desired(root: &Path, paths: &Paths) -> Vec<DesiredTarget> {
    (0..KILL_COUNT)
        .map(|index| DesiredTarget {
            target: root.join(format!("target-{index:03}")),
            item: item(),
            agent_id: None,
            approve_modified: false,
            content: if index % 2 == 0 {
                DesiredContent::File {
                    bytes: vec![b'X'; PAYLOAD_SIZE],
                }
            } else {
                let source = paths.library.join("skills/large");
                DesiredContent::Tree {
                    hash: engine::snapshot(&source, paths).unwrap().hash.unwrap(),
                    from: source,
                }
            },
        })
        .collect()
}

#[test]
fn kill_apply_child() {
    let Some(root) = std::env::var_os("SKM_KILL_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let paths = Paths::resolve(Some(&root.join("home")), None, None).unwrap();
    let store = Store::open(&paths).unwrap();
    let desired = kill_desired(&root, &paths);
    let targets: Vec<_> = desired
        .iter()
        .map(|request| request.target.clone())
        .collect();
    let observed = engine::observe(&paths, &store, &targets).unwrap();
    let plan = engine::plan(1, &desired, &observed);
    engine::apply(&paths, &store, &plan, "kill-child", None).unwrap();
    panic!("parent failed to kill child mid-apply");
}

#[test]
fn sigkill_mid_apply_keeps_every_target_old_absent_or_complete() {
    let fixture = Fixture::new();
    let source = fixture.paths.library.join("skills/large");
    fs::create_dir(&source).unwrap();
    for index in 0..4 {
        fs::write(
            source.join(format!("part-{index}")),
            vec![b'T'; PAYLOAD_SIZE],
        )
        .unwrap();
    }
    let old = fixture.root.join("target-000");
    let mut desired = fixture.desired(old.clone(), 2);
    desired.content = DesiredContent::File {
        bytes: b"old".to_vec(),
    };
    fixture.apply(&fixture.plan(&[desired]));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "kill_apply_child", "--nocapture"])
        .env("SKM_KILL_TEST_ROOT", &fixture.root)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let updated = fs::read(&old).is_ok_and(|bytes| bytes.len() == PAYLOAD_SIZE);
        let in_staging = fs::read_dir(&fixture.root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".skm-tmp")
        });
        if updated && in_staging {
            child.kill().unwrap();
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("child exited before kill: {status}");
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("timed out waiting for mid-apply staging");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(9));
    let complete_tree = engine::snapshot(&source, &fixture.paths).unwrap().hash;
    let mut completed = 0;
    let mut absent = 0;
    for index in 0..KILL_COUNT {
        let target = fixture.root.join(format!("target-{index:03}"));
        if !target.exists() {
            absent += 1;
            continue;
        }
        if index % 2 == 0 {
            let bytes = fs::read(&target).unwrap();
            assert!(bytes == vec![b'X'; PAYLOAD_SIZE] || (index == 0 && bytes == b"old"));
        } else {
            assert_eq!(
                engine::snapshot(&target, &fixture.paths).unwrap().hash,
                complete_tree
            );
        }
        completed += 1;
    }
    assert!(
        completed > 0 && absent > 0,
        "kill must interrupt, not precede or follow apply"
    );
    let empty = Plan {
        scope_id: 1,
        ..Plan::default()
    };
    fixture.apply(&empty);
    eprintln!(
        "SIGKILL proof: signal=9, {completed} complete targets, {absent} absent targets, zero partial targets"
    );
}

#[test]
fn observations_are_serializable() {
    let fixture = Fixture::new();
    let target = fixture.root.join("target");
    let observed = engine::observe(&fixture.paths, &fixture.store, &[target]).unwrap();
    let encoded = serde_json::to_string(&observed).unwrap();
    let decoded: Vec<Observation> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(observed, decoded);
    assert_eq!(decoded[0].state.class, TargetClass::Absent);
    let plan = fixture.plan(&[fixture.desired(fixture.root.join("link"), 1)]);
    assert!(matches!(plan.actions[0].action, Action::Link { .. }));
}

#[test]
fn lock_holder_child() {
    let Some(root) = std::env::var_os("SKM_LOCK_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("home/lock"))
        .unwrap();
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write().unwrap();
    fs::write(root.join("lock-ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn interprocess_lock_blocks_only_apply_and_releases_on_death() {
    let fixture = Fixture::new();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_holder_child"])
        .env("SKM_LOCK_TEST_ROOT", &fixture.root)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.root.join("lock-ready").exists() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("lock holder did not start");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let plan = fixture.plan(&[fixture.desired(fixture.root.join("target"), 2)]);
    assert_eq!(plan.actions.len(), 1);
    let locked = engine::apply(&fixture.paths, &fixture.store, &plan, "test", None).unwrap_err();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(locked.code(), "E_LOCKED");
    assert_eq!(fixture.apply(&plan).done.len(), 1);
}

#[test]
fn failed_action_does_not_block_independent_actions() {
    let fixture = Fixture::new();
    let desired = [
        fixture.desired(fixture.root.join("a-tree"), 3),
        fixture.desired(fixture.root.join("b-file"), 2),
    ];
    let plan = fixture.plan(&desired);
    fs::rename(
        fixture.paths.library.join("skills/review"),
        fixture.paths.library.join("skills/moved"),
    )
    .unwrap();
    let applied = fixture.apply(&plan);
    assert_eq!(applied.failed.len(), 1);
    assert_eq!(applied.failed[0].code, "E_IO");
    assert_eq!(applied.done.len(), 1);
    assert_eq!(applied.inverse.len(), 1);
    assert!(!fixture.root.join("a-tree").exists());
    assert!(fixture.root.join("b-file").is_file());
    assert!(fixture.paths.home.join(".changed").exists());
}
