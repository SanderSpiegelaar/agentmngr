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
