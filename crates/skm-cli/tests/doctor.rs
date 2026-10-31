use std::process::Command;

struct Fixture {
    _root: tempfile::TempDir,
    user: std::path::PathBuf,
    paths: skm_core::paths::Paths,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        std::fs::create_dir(&user).unwrap();
        let mut paths =
            skm_core::paths::Paths::resolve(Some(&root.path().join("manager")), None, None)
                .unwrap();
        paths.ensure_layout().unwrap();
        Self {
            _root: root,
            user,
            paths,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_skm"));
        command
            .arg("--home")
            .arg(&self.paths.home)
            .arg("doctor")
            .env("HOME", &self.user)
            .env_remove("SKILLSMGR_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("COPILOT_HOME");
        command
    }

    fn assert_check(&self, id: &str, status: &str, code: i32, hint: &str, detail: &str) {
        for json in [false, true] {
            let mut command = self.command();
            if json {
                command.arg("--json");
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(code), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            if json {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                let check = value["checks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["id"] == id)
                    .unwrap();
                assert_eq!(check["status"], status, "{text}");
                assert!(!check["fix"].as_str().unwrap().is_empty());
                assert!(check["fix"].as_str().unwrap().contains(hint), "{text}");
                assert!(
                    check["message"].as_str().unwrap().contains(detail),
                    "{text}"
                );
            } else {
                assert!(text.contains(&format!("{status} {id}:")), "{text}");
                if status != "ok" {
                    assert!(text.contains(hint), "{text}");
                }
                assert!(text.contains(detail), "{text}");
            }
        }
    }

    fn project(&self) -> std::path::PathBuf {
        let project = self._root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let store = skm_core::store::Store::open(&self.paths).unwrap();
        skm_core::projects::register(&store, &project).unwrap();
        project
    }

    fn write(path: impl AsRef<std::path::Path>, text: &str) {
        let path = path.as_ref();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

#[test]
fn doctor_healthy_checks_have_hints_and_no_secrets_placeholder() {
    let fixture = Fixture::new();
    for id in [
        "git",
        "data-dir",
        "registry",
        "broken-links",
        "agent-configs",
        "foreign-conflicts",
        "edited-projects",
    ] {
        fixture.assert_check(id, "ok", 0, "", "Passed");
    }
    let output = fixture.command().arg("--json").output().unwrap();
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("secrets")
    );
    assert!(!fixture.paths.db_file.exists());
}

#[cfg(unix)]
#[test]
fn doctor_missing_and_old_git_report_install_hint() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let bin = fixture._root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for version in [None, Some("2.29.9"), Some("2.30.0")] {
        if let Some(version) = version {
            let git = bin.join("git");
            std::fs::write(&git, format!("#!/bin/sh\necho 'git version {version}'\n")).unwrap();
            std::fs::set_permissions(git, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        for json in [false, true] {
            let mut command = fixture.command();
            command.env("PATH", &bin);
            if json {
                command.arg("--json");
            }
            let output = command.output().unwrap();
            let good = version == Some("2.30.0");
            assert_eq!(
                output.status.code(),
                Some(if good { 0 } else { 1 }),
                "{output:?}"
            );
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(text.contains("Install Git >=2.30"), json || !good, "{text}");
            if json {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(
                    value["checks"][0]["status"],
                    if good { "ok" } else { "fail" }
                );
            } else {
                assert!(
                    text.contains(if good { "ok git:" } else { "fail git:" }),
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn doctor_unwritable_layout_is_a_check_not_a_startup_error() {
    let fixture = Fixture::new();
    std::fs::remove_dir(&fixture.paths.logs).unwrap();
    std::fs::write(&fixture.paths.logs, "not a directory").unwrap();
    fixture.assert_check("data-dir", "fail", 1, "--home", "File exists");
}

#[cfg(unix)]
#[test]
fn doctor_read_only_data_directory_has_permission_hint() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    std::fs::set_permissions(&fixture.paths.home, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = tempfile::NamedTempFile::new_in(&fixture.paths.home).is_ok();
    let output = fixture.command().arg("--json").output().unwrap();
    std::fs::set_permissions(&fixture.paths.home, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        return;
    } // Privileged test runners can bypass directory permissions.
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let check = value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "data-dir")
        .unwrap();
    assert_eq!(check["status"], "fail");
    assert!(
        check["fix"]
            .as_str()
            .unwrap()
            .contains("Make the data directory writable")
    );
}

#[test]
fn doctor_rejects_invalid_registry_overrides_with_hint() {
    let fixture = Fixture::new();
    for text in [
        "[agents.unknown]\ndetect = []",
        "[agents.codex.mcp.project]\nfile = '../escape.toml'",
        "[agents.opencode.hooks.user]\nfile = '/tmp/hooks.json'",
        "theme = [",
    ] {
        std::fs::write(&fixture.paths.config_file, text).unwrap();
        fixture.assert_check("registry", "fail", 1, "config.toml registry overrides", "");
    }
}

#[cfg(unix)]
#[test]
fn doctor_broken_global_and_project_links_have_restore_hint() {
    let fixture = Fixture::new();
    let project = fixture.project();
    for root in [&fixture.user, &project] {
        let skills = root.join(".agents/skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::os::unix::fs::symlink(
            fixture.paths.library.join("skills/missing"),
            skills.join("missing"),
        )
        .unwrap();
    }
    fixture.assert_check(
        "broken-links",
        "warn",
        3,
        "Restore missing link destinations",
        "missing",
    );
    let output = fixture.command().arg("--json").output().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let check = value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "broken-links")
        .unwrap();
    let message = check["message"].as_str().unwrap();
    assert!(message.contains(fixture.user.to_str().unwrap()));
    assert!(message.contains(project.to_str().unwrap()));
}

#[test]
fn doctor_parses_user_and_project_configs_from_resolved_registry() {
    let fixture = Fixture::new();
    let project = fixture.project();
    let overridden = fixture.user.join("custom.toml");
    std::fs::write(&fixture.paths.config_file, format!("[agents.codex.mcp.user]\nfile = {:?}\n[agents.codex.mcp.project]\nfile = 'custom.toml'", overridden)).unwrap();
    for file in [
        &overridden,
        &project.join("custom.toml"),
        &fixture.user.join(".claude/settings.json"),
        &project.join("opencode.json"),
    ] {
        Fixture::write(file, "invalid config {");
        fixture.assert_check(
            "agent-configs",
            "fail",
            1,
            "Fix the syntax",
            file.file_name().unwrap().to_str().unwrap(),
        );
        std::fs::remove_file(file).unwrap();
    }
    Fixture::write(
        fixture.user.join(".config/opencode/opencode.json"),
        "{\n// comment\n\"mcp\": {\"url\": \"https://example.com\", /* trailing */},\n}",
    );
    fixture.assert_check("agent-configs", "ok", 0, "Fix the syntax", "Passed");
}

#[test]
fn doctor_foreign_entries_have_adopt_hint() {
    let fixture = Fixture::new();
    let project = fixture.project();
    for root in [&fixture.user, &project] {
        Fixture::write(
            root.join(".agents/skills/foreign/SKILL.md"),
            "foreign content",
        );
    }
    fixture.assert_check("foreign-conflicts", "warn", 3, "skm adopt", "foreign");
}

#[test]
fn doctor_edited_project_entries_have_preserve_hint_without_claiming() {
    let fixture = Fixture::new();
    let project = fixture.project();
    Fixture::write(
        fixture.paths.library.join("skills/review/SKILL.md"),
        "library content",
    );
    Fixture::write(
        project.join(".agents/skills/review/SKILL.md"),
        "local edits",
    );
    fixture.assert_check(
        "edited-projects",
        "warn",
        3,
        "preserve local changes",
        "review",
    );
    let store = skm_core::store::Store::open(&fixture.paths).unwrap();
    let count: i64 = store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM ledger", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        std::fs::read_to_string(project.join(".agents/skills/review/SKILL.md")).unwrap(),
        "local edits"
    );
}

#[test]
fn doctor_detects_edits_to_owned_project_copies() {
    let fixture = Fixture::new();
    let project = fixture.project();
    let target = project.join(".agents/skills/review/SKILL.md");
    Fixture::write(
        fixture.paths.library.join("skills/review/SKILL.md"),
        "original content",
    );
    Fixture::write(&target, "original content");
    let store = skm_core::store::Store::open(&fixture.paths).unwrap();
    skm_core::projects::register_with_report(&store, &project).unwrap();
    fixture.assert_check(
        "edited-projects",
        "ok",
        0,
        "preserve local changes",
        "Passed",
    );
    Fixture::write(&target, "teammate edits");
    fixture.assert_check(
        "edited-projects",
        "warn",
        3,
        "preserve local changes",
        "review",
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "teammate edits");
}

#[test]
fn doctor_bundle_redacts_secrets_in_every_archive_entry_and_replaces_old_archive() {
    let fixture = Fixture::new();
    let secrets = [
        "plain-token-value",
        "private-key-value",
        "nested-secret-value",
        "password-value",
        "auth-value",
        "ordinary-env-value",
        "ghp_1234567890abcdefghijklmnopqrstuv",
        "sk-proj-1234567890abcdefghijklmnop",
        "github_pat_1234567890abcdef",
        "xoxb-123456789-abc",
        "AKIA1234567890ABCDEF",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.signature",
        "library-secret-value",
        "library-env-value",
        "bearer-value-without-provider-prefix",
    ];
    std::fs::write(&fixture.paths.config_file, format!("theme = 'dark'\ntoken = '{}'\napi_key = '{}'\n[credentials]\nclient_secret = '{}'\npassword = '{}'\nauthorization = '{}'\n[env]\nORDINARY = '{}'\n[agents.codex]\ndetect = ['{}']\n", secrets[0], secrets[1], secrets[2], secrets[3], secrets[4], secrets[5], secrets[6])).unwrap();
    Fixture::write(
        fixture.paths.library.join("library.toml"),
        &format!(
            "schema = 1\n[global]\n[skills.review.source]\nkind = 'local'\norigin = '{}'\n[extra]\npassword = '{}'\n[extra.env]\nNORMAL = '{}'\n",
            secrets[7], secrets[12], secrets[13]
        ),
    );
    Fixture::write(
        fixture.paths.logs.join("fixture.log"),
        &format!(
            "healthy log line\n{}\nAuthorization: Bearer {}\n",
            secrets[..14].join("\n"),
            secrets[14]
        ),
    );
    let bundle = fixture._root.path().join("report.zip");
    // A preexisting archive must not retain entries that aren't in the new bundle.
    Fixture::write(fixture._root.path().join("stale.txt"), secrets[0]);
    assert!(
        Command::new("/usr/bin/zip")
            .current_dir(fixture._root.path())
            .arg("-q")
            .arg(&bundle)
            .arg("stale.txt")
            .status()
            .unwrap()
            .success()
    );
    let output = fixture
        .command()
        .arg("--bundle")
        .arg(&bundle)
        .arg("--json")
        .env("ZIPOPT", "--invalid")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let listing = Command::new("/usr/bin/unzip")
        .args(["-Z1"])
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(listing.status.success());
    let listing = String::from_utf8(listing.stdout).unwrap();
    for name in [
        "config.toml",
        "library.toml",
        "registry-overrides.toml",
        "logs/fixture.log",
    ] {
        assert!(listing.lines().any(|line| line == name), "{listing}");
    }
    assert!(!listing.contains("stale.txt"));
    for entry in listing.lines().filter(|entry| !entry.ends_with('/')) {
        let output = Command::new("/usr/bin/unzip")
            .arg("-p")
            .arg(&bundle)
            .arg(entry)
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        for secret in secrets {
            assert!(
                !text.contains(secret),
                "secret {secret} leaked in {entry}: {text}"
            );
        }
        if entry == "config.toml" {
            toml::from_str::<toml::Value>(&text).unwrap();
            assert!(text.contains("dark"));
            assert!(text.contains("[REDACTED]"));
        }
        if entry == "logs/fixture.log" {
            assert!(text.contains("healthy log line"));
        }
    }
}

#[test]
fn doctor_bundle_omits_unparseable_config_and_unsafe_log_contents() {
    let fixture = Fixture::new();
    let secret = "unparseable-config-secret";
    std::fs::write(
        &fixture.paths.config_file,
        format!("token = '{secret}'\ninvalid = ["),
    )
    .unwrap();
    Fixture::write(fixture.paths.logs.join("fixture.log"), secret);
    let bundle = fixture._root.path().join("invalid.zip");
    let output = fixture
        .command()
        .arg("--bundle")
        .arg(&bundle)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(bundle.is_file());
    let output = Command::new("/usr/bin/unzip")
        .arg("-p")
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains(secret));
    assert!(text.contains("Invalid config omitted for safety"));
    assert!(text.contains("Log contents omitted"));
}

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
            .env("HOME", root.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("COPILOT_HOME")
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
                home.canonicalize().unwrap()
            } else {
                home.canonicalize().unwrap().join(suffix)
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
            .env("HOME", root.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("COPILOT_HOME")
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
            command
                .env("HOME", root.path())
                .env_remove("CLAUDE_CONFIG_DIR")
                .env_remove("COPILOT_HOME");
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
