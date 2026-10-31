#![cfg(unix)]

use std::{fs, process::Command};

use skm_core::{content_hash::content_hash, engine, paths::Paths};

#[test]
fn foreign_adopt_dry_run_and_undo_preserve_original_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("user");
    let mut paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
    paths.ensure_layout().unwrap();
    let original = home.join(".agents/skills/demo");
    fs::create_dir_all(original.join("empty")).unwrap();
    fs::write(original.join("SKILL.md"), b"original\0skill\n").unwrap();
    let before = engine::snapshot(&original, &paths).unwrap();
    let hash = content_hash(&original).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_skm"))
            .arg("--home")
            .arg(&paths.home)
            .args(args)
            .env("HOME", &home)
            .output()
            .unwrap();
        println!(
            "skm {}: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(run(&["foreign"]).contains("demo"));
    let dry = run(&["adopt", "demo", "--dry-run"]);
    assert!(dry.contains("CopyTree") && dry.contains("Link") && dry.contains("WriteFile"));
    assert_eq!(engine::snapshot(&original, &paths).unwrap(), before);
    assert!(!paths.library.join("skills/demo").exists());
    assert!(!paths.library.join("library.toml").exists());
    run(&["adopt", "demo"]);
    assert!(original.is_symlink());
    assert_eq!(content_hash(&original).unwrap(), hash);
    assert_eq!(
        content_hash(&paths.library.join("skills/demo")).unwrap(),
        hash
    );
    run(&["undo"]);
    assert_eq!(engine::snapshot(&original, &paths).unwrap(), before);
    assert!(!paths.library.join("skills/demo").exists());
    println!("adopt: original == library == link hash; undo restored exact original snapshot");
}

#[test]
fn project_cli_claims_copies_and_collision_exits_three() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("user");
    let project = root.join("project");
    fs::create_dir(&home).unwrap();
    let target = project.join(".agents/skills/demo");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), "project bytes").unwrap();
    let mut paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
    paths.ensure_layout().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_skm"))
            .arg("--home")
            .arg(&paths.home)
            .args(args)
            .env("HOME", &home)
            .output()
            .unwrap()
    };
    let dir = project.to_str().unwrap();
    let before = engine::snapshot(&project, &paths).unwrap();
    let dry = run(&["adopt", "demo", "--project", dir, "--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    assert!(String::from_utf8_lossy(&dry.stdout).contains("Claim"));
    assert_eq!(engine::snapshot(&project, &paths).unwrap(), before);
    assert!(!paths.library.join("skills/demo").exists());
    let result = run(&["adopt", "demo", "--project", dir]);
    assert!(result.status.success(), "{result:?}");
    assert_eq!(engine::snapshot(&project, &paths).unwrap(), before);
    let listed = run(&["foreign", "--project", dir]);
    assert!(listed.status.success());
    assert!(listed.stdout.is_empty());
    assert!(run(&["undo"]).status.success());
    assert_eq!(engine::snapshot(&project, &paths).unwrap(), before);
    fs::create_dir_all(paths.library.join("skills/demo")).unwrap();
    fs::write(paths.library.join("skills/demo/SKILL.md"), "collision").unwrap();
    let result = run(&["adopt", "demo", "--project", dir]);
    assert_eq!(result.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_NAME_COLLISION"));
    assert_eq!(engine::snapshot(&project, &paths).unwrap(), before);
    println!("project CLI: dry-run Claim; adopt/undo preserve tree; collision exits 3");
}
