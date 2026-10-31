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
    }
    assert!(!home.exists(), "doctor should only report paths");
}
