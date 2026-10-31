use std::{path::Path, process::Command};

use skm_core::{
    engine::{self, DesiredContent, DesiredTarget},
    paths::Paths,
    store::Store,
    target::{ItemKind, ItemRef},
};

fn skm(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_skm"))
        .arg("--home")
        .arg(home)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn undo_cli_lists_selects_defaults_and_reports_conflicts() {
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
    let target = root.join("target");
    let desired = DesiredTarget {
        target: target.clone(),
        item: ItemRef {
            kind: ItemKind::Skill,
            name: "review".into(),
        },
        agent_id: None,
        content: DesiredContent::File {
            bytes: b"managed".to_vec(),
        },
        approve_modified: false,
    };
    let observations = engine::observe(&paths, &store, std::slice::from_ref(&target)).unwrap();
    let plan = engine::plan(1, &[desired], &observations);
    let applied = engine::apply(&paths, &store, &plan, "cli-test", None).unwrap();
    let output = skm(&paths.home, &["undo", "--list"]);
    assert!(output.status.success());
    let list = String::from_utf8(output.stdout).unwrap();
    assert!(list.contains("ID\tAT\tACTOR\tRESULT\tSUMMARY"));
    assert!(list.contains("cli-test\tok\t1 actions, 0 issues"));
    assert!(target.exists());

    let output = skm(&paths.home, &["undo", &applied.op_id.to_string()]);
    assert!(output.status.success(), "{:?}", output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("1 undone")
    );
    assert!(!target.exists());
    let output = skm(&paths.home, &["undo"]);
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(std::fs::read(&target).unwrap(), b"managed");

    std::fs::write(&target, b"edited").unwrap();
    let output = skm(&paths.home, &["undo"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("E_CONFIG_CHANGED")
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"edited");

    let output = skm(&paths.home, &["undo", "1", "--list"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn undo_cli_empty_history_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let output = skm(temp.path(), &["undo"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("E_CONFIG_CHANGED")
    );
}
