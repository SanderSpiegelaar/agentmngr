#![cfg(unix)]

use std::{fs, process::Command};

#[test]
fn apply_database_failure_exits_one() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("user");
    fs::create_dir(&home).unwrap();
    let paths = skm_core::paths::Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
    let store = skm_core::store::Store::open(&paths).unwrap();
    fs::create_dir_all(paths.library.join("skills/review")).unwrap();
    fs::write(paths.library.join("skills/review/SKILL.md"), "review\n").unwrap();
    store.write(|connection| {
        connection.execute_batch("CREATE TRIGGER fail_ledger BEFORE INSERT ON ledger BEGIN SELECT RAISE(FAIL, 'test database failure'); END;")?;
        Ok(())
    }).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_skm"))
        .arg("--home")
        .arg(&paths.home)
        .args(["on", "review"])
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 failed"));
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn global_on_off_dry_run_and_foreign_exit_status() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("user");
    let manager = root.join("manager");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(manager.join("library/skills/review")).unwrap();
    fs::write(manager.join("library/skills/review/SKILL.md"), "review\n").unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_skm"))
            .arg("--home")
            .arg(&manager)
            .args(args)
            .env("HOME", &home)
            .env("SKILLSMGR_HOME", root.join("wrong-manager"))
            .output()
            .unwrap()
    };
    let target = home.join(".agents/skills/review");
    let dry = run(&["on", "review", "--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    let stdout = String::from_utf8(dry.stdout).unwrap();
    assert!(stdout.contains("1 actions, 0 issues") && stdout.contains("Link"));
    assert!(!target.exists());
    assert!(!manager.join("library/library.toml").exists());
    assert!(run(&["on", "review"]).status.success());
    assert_eq!(
        fs::read_link(&target).unwrap(),
        manager.join("library/skills/review")
    );
    assert!(run(&["--dry-run", "off", "review"]).status.success());
    assert!(target.is_symlink());
    assert!(run(&["off", "review"]).status.success());
    assert!(fs::symlink_metadata(&target).is_err());
    fs::write(&target, "foreign bytes\0").unwrap();
    for args in [
        vec!["on", "review"],
        vec!["off", "review"],
        vec!["on", "review", "--dry-run"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(3));
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("E_FOREIGN_TARGET")
        );
        assert_eq!(fs::read(&target).unwrap(), b"foreign bytes\0");
    }
    assert!(!home.join(".claude").exists());
    assert!(!root.join("wrong-manager").exists());
    println!(
        "CLI HOME + --home: on/off work; dry-run prints plan without changing desired state or links; foreign refusals exit 3 and preserve bytes"
    );
}
