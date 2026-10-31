use std::{fs, process::Command};

use skm_core::{paths::Paths, registry::Registry, store::Store};

#[test]
fn agents_preserves_enabled_state_and_persists_detection() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::resolve(Some(root.path()), None, None).unwrap();
    let store = Store::open(&paths).unwrap();
    store.write(|connection| {
        connection.execute("INSERT INTO agents (id, detected, enabled, version) VALUES ('claude-code', 0, 1, 'existing-version')", [])?;
        Ok(())
    }).unwrap();
    let marker = root.path().join("detected-agent");
    fs::write(&marker, "").unwrap();
    // Override every detection path so the test is independent of installed agents.
    let registry = Registry::load().unwrap();
    let mut config = String::new();
    for id in registry.agents.keys() {
        config.push_str(&format!(
            "[agents.{id}]\ndetect = [{:?}]\n",
            marker.to_str().unwrap()
        ));
    }
    fs::write(&paths.config_file, config).unwrap();
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_skm"))
            .args(["--home", root.path().to_str().unwrap(), "agents"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    let stdout = run();
    let claude: Vec<_> = stdout
        .lines()
        .find(|line| line.starts_with("claude-code"))
        .unwrap()
        .split_whitespace()
        .collect();
    assert_eq!(&claude[claude.len() - 2..], &["true", "true"]);
    store
        .read(|connection| {
            let enabled: (bool, bool, String, i64) = connection.query_row(
                "SELECT detected, enabled, version, detected_at FROM agents WHERE id='claude-code'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            assert_eq!(
                (enabled.0, enabled.1, enabled.2.as_str()),
                (true, true, "existing-version")
            );
            assert!(enabled.3 > 0);
            let disabled: i64 = connection.query_row(
                "SELECT count(*) FROM agents WHERE detected=1 AND enabled=0",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(disabled as usize, registry.agents.len() - 1);
            Ok(())
        })
        .unwrap();
    fs::remove_file(marker).unwrap();
    run();
    store
        .read(|connection| {
            let status: (bool, bool) = connection.query_row(
                "SELECT detected, enabled FROM agents WHERE id='claude-code'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(status, (false, true));
            Ok(())
        })
        .unwrap();
    println!(
        "agents: stored enablement and version preserved; detection upserted true then false; new agents stay disabled"
    );
}
