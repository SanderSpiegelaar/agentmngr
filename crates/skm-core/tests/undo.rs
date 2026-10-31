#![cfg(unix)]

use std::{fs, os::unix::fs::symlink, path::PathBuf};

use skm_core::{
    engine::{self, ApplyResult, DesiredContent, DesiredTarget, Observation, Plan},
    paths::Paths,
    store::Store,
    target::{ItemKind, ItemRef},
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
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths::resolve(Some(&root.join("home")), None, None).unwrap();
        let store = Store::open(&paths).unwrap();
        store
            .write(|connection| {
                connection.execute(
                    "INSERT INTO scopes(id,kind,name,added_at) VALUES(1,'global','Global',0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let source = root.join("source");
        fs::create_dir_all(source.join("empty")).unwrap();
        fs::create_dir(source.join(".git")).unwrap();
        fs::write(source.join("SKILL.md"), b"original\0bytes").unwrap();
        fs::write(source.join(".git/config"), b"included in undo hash").unwrap();
        symlink("SKILL.md", source.join("alias")).unwrap();
        Self {
            _temp: temp,
            root,
            paths,
            store,
        }
    }

    fn desired(&self, target: PathBuf, content: DesiredContent) -> DesiredTarget {
        DesiredTarget {
            target,
            item: ItemRef {
                kind: ItemKind::Skill,
                name: "review".into(),
            },
            agent_id: None,
            content,
            approve_modified: false,
        }
    }

    fn tree(&self, from: PathBuf) -> DesiredContent {
        DesiredContent::Tree {
            hash: engine::snapshot(&from, &self.paths).unwrap().hash.unwrap(),
            from,
        }
    }

    fn observations(&self, targets: &[PathBuf]) -> Vec<Observation> {
        engine::observe(&self.paths, &self.store, targets).unwrap()
    }

    fn apply(&self, desired: &[DesiredTarget]) -> ApplyResult {
        let targets: Vec<_> = desired
            .iter()
            .map(|request| request.target.clone())
            .collect();
        let mut plan = engine::plan(1, desired, &self.observations(&targets));
        // Adoption must copy its source before replacing that source with a link.
        plan.actions.sort_by_key(|action| {
            targets
                .iter()
                .position(|target| target == action.action.target())
                .unwrap()
        });
        assert!(plan.issues.is_empty(), "{:?}", plan.issues);
        let result = engine::apply(&self.paths, &self.store, &plan, "test", None).unwrap();
        assert!(result.issues.is_empty(), "{:?}", result.issues);
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        assert_eq!(result.done.len(), desired.len());
        result
    }

    fn undo(&self, id: Option<i64>, before: &[Observation]) -> ApplyResult {
        let result = engine::undo(&self.paths, &self.store, id, "test-undo", None).unwrap();
        assert!(result.issues.is_empty(), "{:?}", result.issues);
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        let targets: Vec<_> = before
            .iter()
            .map(|observation| observation.target.clone())
            .collect();
        assert_eq!(self.observations(&targets), before);
        let latest = engine::list_ops(&self.store).unwrap().remove(0);
        assert_eq!(latest.id, result.op_id);
        assert!(latest.summary.starts_with("Undo op "));
        assert_eq!(latest.result, "ok");
        result
    }
}

#[test]
fn undo_on_restores_absence_and_undo_itself_is_undoable() {
    let fixture = Fixture::new();
    let target = fixture.root.join("global/review");
    let before = fixture.observations(std::slice::from_ref(&target));
    let applied = fixture.apply(&[fixture.desired(
        target.clone(),
        DesiredContent::Link {
            to: fixture.paths.library.join("skills/review"),
        },
    )]);
    let after = fixture.observations(&[target]);
    let undone = fixture.undo(None, &before);
    assert!(undone.op_id > applied.op_id);
    fixture.undo(Some(undone.op_id), &after);
}

#[test]
fn undo_off_restores_exact_link_and_ledger() {
    let fixture = Fixture::new();
    let target = fixture.root.join("global/review");
    fixture.apply(&[fixture.desired(
        target.clone(),
        DesiredContent::Link {
            to: fixture.paths.library.join("skills/review"),
        },
    )]);
    let before = fixture.observations(std::slice::from_ref(&target));
    let applied = fixture.apply(&[fixture.desired(target, DesiredContent::Absent)]);
    fixture.undo(Some(applied.op_id), &before);
}

#[test]
fn undo_adopt_actions_restores_original_tree_and_removes_library_copy() {
    let fixture = Fixture::new();
    let target = fixture.root.join("global/review");
    // T-020 supplies foreign adoption approval. Seed ownership here to exercise
    // the same CopyTree + replacement/Trash + Link effects without bypassing it.
    fixture.apply(&[fixture.desired(target.clone(), fixture.tree(fixture.root.join("source")))]);
    let library = fixture.paths.library.join("skills/review");
    let before = fixture.observations(&[library.clone(), target.clone()]);
    let applied = fixture.apply(&[
        fixture.desired(library, fixture.tree(target.clone())),
        fixture.desired(
            target,
            DesiredContent::Link {
                to: fixture.paths.library.join("skills/review"),
            },
        ),
    ]);
    fixture.undo(Some(applied.op_id), &before);
}

#[test]
fn undo_remove_restores_exact_library_tree() {
    let fixture = Fixture::new();
    let target = fixture.paths.library.join("skills/review");
    fixture.apply(&[fixture.desired(target.clone(), fixture.tree(fixture.root.join("source")))]);
    let before = fixture.observations(std::slice::from_ref(&target));
    let applied = fixture.apply(&[fixture.desired(target, DesiredContent::Absent)]);
    fixture.undo(Some(applied.op_id), &before);
}

#[test]
fn undo_project_apply_restores_tree_file_and_absence() {
    let fixture = Fixture::new();
    let tree = fixture.root.join("project/.agents/skills/review");
    let file = fixture.root.join("project/skills-lock.json");
    let created = fixture.root.join("project/new-file");
    fixture.apply(&[
        fixture.desired(tree.clone(), fixture.tree(fixture.root.join("source"))),
        fixture.desired(
            file.clone(),
            DesiredContent::File {
                bytes: b"old lock".to_vec(),
            },
        ),
    ]);
    let before = fixture.observations(&[created.clone(), file.clone(), tree.clone()]);
    fs::write(fixture.root.join("source/SKILL.md"), b"updated").unwrap();
    let applied = fixture.apply(&[
        fixture.desired(tree, fixture.tree(fixture.root.join("source"))),
        fixture.desired(
            file,
            DesiredContent::File {
                bytes: b"new lock".to_vec(),
            },
        ),
        fixture.desired(
            created,
            DesiredContent::File {
                bytes: b"new file".to_vec(),
            },
        ),
    ]);
    fixture.undo(Some(applied.op_id), &before);
}

#[test]
fn conflicting_undo_hash_checks_every_target_before_any_change() {
    let fixture = Fixture::new();
    let edited = fixture.root.join("a-edited");
    let untouched = fixture.root.join("z-untouched");
    let applied = fixture.apply(&[
        fixture.desired(
            edited.clone(),
            DesiredContent::File {
                bytes: b"original".to_vec(),
            },
        ),
        fixture.desired(untouched.clone(), fixture.tree(fixture.root.join("source"))),
    ]);
    let metadata = fs::metadata(&edited).unwrap();
    fs::write(&edited, b"modified").unwrap();
    fs::File::open(&edited)
        .unwrap()
        .set_modified(metadata.modified().unwrap())
        .unwrap();
    let before = fixture.observations(&[edited, untouched]);
    let history = engine::list_ops(&fixture.store).unwrap();
    let error = engine::undo(
        &fixture.paths,
        &fixture.store,
        Some(applied.op_id),
        "test",
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), "E_CONFIG_CHANGED");
    assert_eq!(
        fixture.observations(
            &before
                .iter()
                .map(|row| row.target.clone())
                .collect::<Vec<_>>()
        ),
        before
    );
    assert_eq!(engine::list_ops(&fixture.store).unwrap(), history);
}

#[test]
fn history_limits_explicit_selection_to_last_50_and_allows_nonlatest() {
    let fixture = Fixture::new();
    let target = fixture.root.join("older");
    let before = fixture.observations(std::slice::from_ref(&target));
    let applied =
        fixture.apply(&[fixture.desired(target, DesiredContent::File { bytes: vec![1] })]);
    let empty = Plan {
        scope_id: 1,
        ..Plan::default()
    };
    for _ in 0..49 {
        engine::apply(&fixture.paths, &fixture.store, &empty, "test", None).unwrap();
    }
    let history = engine::list_ops(&fixture.store).unwrap();
    assert_eq!(history.len(), 50);
    assert_eq!(history.last().unwrap().id, applied.op_id);
    fixture.undo(Some(applied.op_id), &before);
    let error = engine::undo(
        &fixture.paths,
        &fixture.store,
        Some(applied.op_id),
        "test",
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), "E_CONFIG_CHANGED");
    assert_eq!(engine::list_ops(&fixture.store).unwrap().len(), 50);
}

#[test]
fn empty_history_and_unknown_ids_refuse_without_recording_an_op() {
    let fixture = Fixture::new();
    for id in [None, Some(42)] {
        let error = engine::undo(&fixture.paths, &fixture.store, id, "test", None).unwrap_err();
        assert_eq!(error.code(), "E_CONFIG_CHANGED");
        assert!(engine::list_ops(&fixture.store).unwrap().is_empty());
    }
}

#[test]
fn edited_backup_refuses_before_other_inverse_actions() {
    let fixture = Fixture::new();
    let target = fixture.root.join("a-file");
    fixture.apply(&[fixture.desired(
        target.clone(),
        DesiredContent::File {
            bytes: b"old".to_vec(),
        },
    )]);
    let applied = fixture.apply(&[
        fixture.desired(
            target.clone(),
            DesiredContent::File {
                bytes: b"new".to_vec(),
            },
        ),
        fixture.desired(
            fixture.root.join("z-created"),
            DesiredContent::File { bytes: vec![1] },
        ),
    ]);
    let engine::Action::Restore { from, .. } = &applied.inverse[1].action else {
        panic!("expected restore")
    };
    fs::write(from, b"tampered").unwrap();
    let before = fixture.observations(&[target, fixture.root.join("z-created")]);
    assert_eq!(
        engine::undo(
            &fixture.paths,
            &fixture.store,
            Some(applied.op_id),
            "test",
            None
        )
        .unwrap_err()
        .code(),
        "E_CONFIG_CHANGED"
    );
    assert_eq!(
        fixture.observations(
            &before
                .iter()
                .map(|row| row.target.clone())
                .collect::<Vec<_>>()
        ),
        before
    );
}

#[test]
fn undo_partial_apply_reverts_only_completed_actions() {
    let fixture = Fixture::new();
    let source = fixture.root.join("source");
    let tree = fixture.root.join("a-tree");
    let file = fixture.root.join("z-file");
    let targets = [tree.clone(), file.clone()];
    let before = fixture.observations(&targets);
    let desired = [
        fixture.desired(tree, fixture.tree(source.clone())),
        fixture.desired(file, DesiredContent::File { bytes: vec![1] }),
    ];
    let plan = engine::plan(1, &desired, &before);
    fs::write(source.join("SKILL.md"), b"changed source").unwrap();
    let applied = engine::apply(&fixture.paths, &fixture.store, &plan, "test", None).unwrap();
    assert_eq!(applied.done.len(), 1);
    assert_eq!(applied.issues[0].code, "E_CONFIG_CHANGED");
    let undone = fixture.undo(Some(applied.op_id), &before);
    assert_eq!(undone.done.len(), 1);
}
