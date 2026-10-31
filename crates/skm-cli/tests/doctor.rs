use std::process::Command;

#[test]
fn doctor_prints_resolved_paths_and_accepts_global_home_after_subcommand() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("cli");
    for arguments in [
        vec!["--home", home.to_str().unwrap(), "doctor"],
        vec!["doctor", "--home", home.to_str().unwrap()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_skm"))
            .args(arguments)
            .env("SKILLSMGR_HOME", root.path().join("env"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        for (label, suffix) in [
            ("home", ""),
            ("library", "library"),
            ("cache", "cache"),
            ("trash", "trash"),
            ("logs", "logs"),
            ("config", "config.toml"),
            ("db", "state.db"),
            ("lock", "lock"),
        ] {
            let path = if suffix.is_empty() {
                home.clone()
            } else {
                home.join(suffix)
            };
            assert!(
                stdout
                    .lines()
                    .any(|line| line == format!("{label}: {}", path.display())),
                "{stdout}"
            );
        }
        for directory in ["library", "cache", "trash", "logs"] {
            assert!(home.join(directory).is_dir());
        }
        let logs: Vec<_> = std::fs::read_dir(home.join("logs"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].to_str().unwrap().starts_with("skillsmgr.log."));
        let sentinel = home.join("library/keep");
        if sentinel.exists() {
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"unchanged");
        } else {
            std::fs::write(sentinel, b"unchanged").unwrap();
        }
    }
    assert!(!home.join("state.db").exists());
    println!(
        "doctor twice: library/cache/trash/logs and one log file created; existing contents preserved; no DB opened"
    );
}

#[test]
fn doctor_reindex_rebuilds_identical_rows() {
    use skm_core::{indexer, paths::Paths, store::Store};
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::resolve(Some(root.path()), None, None).unwrap();
    let skill = paths.library.join("skills/test");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: test\ndescription: Test\n---\n",
    )
    .unwrap();
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_skm"))
            .args([
                "--home",
                root.path().to_str().unwrap(),
                "doctor",
                "--reindex",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(run().contains("1 total, 1 inserted"));
    let store = Store::open(&paths).unwrap();
    let expected = indexer::load(&store).unwrap();
    let output = run();
    assert!(output.contains("1 total, 0 inserted, 0 updated, 0 removed, 1 unchanged"));
    assert_eq!(indexer::load(&store).unwrap(), expected);
    println!(
        "doctor --reindex twice: every IndexRow column identical; {}",
        output.lines().next().unwrap()
    );
}

#[test]
fn doctor_validates_library_agent_ids_against_registry() {
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("library");
    std::fs::create_dir_all(&library).unwrap();
    let manifest = library.join("library.toml");
    for reindex in [false, true] {
        for (agent, success) in [("claude-code", true), ("*", true), ("not-an-agent", false)] {
            std::fs::write(
                &manifest,
                format!("schema = 1\n[global]\nmcp = {{ test = ['{agent}'] }}\n"),
            )
            .unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_skm"));
            command.args(["--home", root.path().to_str().unwrap(), "doctor"]);
            if reindex {
                command.arg("--reindex");
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.success(), success, "{output:?}");
            if !success {
                let stderr = String::from_utf8(output.stderr).unwrap();
                assert!(stderr.contains("unknown agent"), "{stderr}");
                assert!(stderr.contains("not-an-agent"), "{stderr}");
            }
        }
    }
    println!(
        "doctor library validation: registered IDs and wildcard accepted; unknown IDs rejected, with and without --reindex"
    );
}
