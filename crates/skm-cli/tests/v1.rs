use serde_json::Value;
use skm_core::{engine, indexer, paths::Paths, store::Store};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{Duration, Instant},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    paths: Paths,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("user");
        let project = root.join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&project)
                .status()
                .unwrap()
                .success()
        );
        let mut paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
        paths.ensure_layout().unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            home,
            paths,
            project,
        };
        fixture.skill("review", "Review code");
        let store = Store::open(&fixture.paths).unwrap();
        indexer::full_reindex(&store, &fixture.paths).unwrap();
        fixture
    }

    fn skill(&self, name: &str, description: &str) {
        let directory = self.paths.library.join("skills").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nSkill body.\n"),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_skm"))
            .arg("--home")
            .arg(&self.paths.home)
            .args(args)
            .env("HOME", &self.home)
            .env_remove("SKILLSMGR_HOME")
            .output()
            .unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        self.normalize(&String::from_utf8(output.stdout).unwrap())
    }

    fn normalize(&self, text: &str) -> String {
        let mut text = text.replace(self.root.to_str().unwrap(), "<ROOT>");
        while let Some(start) = text.find("cache/install-") {
            let suffix = &text[start + 6..];
            let end = suffix.find('/').unwrap();
            text.replace_range(start + 6..start + 6 + end, "<preview>");
        }
        text
    }

    fn comparable(&self, bytes: &[u8], json: bool) -> String {
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        if !json {
            return self.normalize(&text);
        }
        let mut value: Value = serde_json::from_str(&text).unwrap();
        fn readable(value: &mut Value) {
            match value {
                Value::Object(object) => {
                    for (key, value) in object {
                        if key == "bytes" {
                            if let Some(bytes) = value.as_array() {
                                let bytes: Vec<_> =
                                    bytes.iter().map(|n| n.as_u64().unwrap() as u8).collect();
                                *value =
                                    Value::String(String::from_utf8_lossy(&bytes).into_owned());
                            }
                        } else {
                            readable(value);
                        }
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        readable(value);
                    }
                }
                _ => {}
            }
        }
        readable(&mut value);
        let text = self.normalize(&serde_json::to_string_pretty(&value).unwrap());
        let mut result = String::new();
        for line in text.lines() {
            if line.contains("installed_at =") {
                let mut line = line.to_owned();
                while let Some(start) = line.find("installed_at = ") {
                    let start = start + "installed_at = ".len();
                    let end = line[start..]
                        .find("\\n")
                        .map(|n| start + n)
                        .unwrap_or(line.len());
                    line.replace_range(start..end, "<TIMESTAMP>");
                    if !line[start..].contains("installed_at = ") {
                        break;
                    }
                }
                result.push_str(&line);
            } else {
                result.push_str(line);
            }
            result.push('\n');
        }
        result
    }

    fn dry_equals_apply(&self, args: &[&str], json: bool) -> String {
        let store = Store::open(&self.paths).unwrap();
        let state = || {
            store.read(|db| {
            let mut state=Vec::new();
            for query in [
                "SELECT json_group_array(json_array(id,kind,path,name,mode,auto_sync)) FROM scopes",
                "SELECT json_group_array(json_array(scope_id,set_name)) FROM project_sets",
                "SELECT json_group_array(json_array(scope_id,item_kind,item_name,agents_json)) FROM project_items",
                "SELECT json_group_array(json_array(id,detected,enabled,detected_at)) FROM agents",
                "SELECT json_group_array(json_array(scope_id,change_key,revision)) FROM project_pending",
                "SELECT json_group_array(json_array(id,result)) FROM ops",
            ] {state.push(db.query_row(query,[],|row|row.get::<_,String>(0))?);}
            Ok(state)
        }).unwrap()
        };
        let desired_before = state();
        let project_before = engine::snapshot(&self.project, &self.paths).unwrap();
        let last_op: i64 = store
            .read(|db| {
                Ok(db.query_row("SELECT COALESCE(max(id),0) FROM ops", [], |row| row.get(0))?)
            })
            .unwrap();
        let mut args = args.to_vec();
        if json {
            args.push("--json");
        }
        let before = engine::snapshot(&self.paths.library, &self.paths).unwrap();
        let changed = fs::read(self.paths.home.join(".changed")).ok();
        args.push("--dry-run");
        let dry = self.run(&args);
        assert!(dry.status.success(), "dry {args:?}: {dry:?}");
        assert_eq!(
            state(),
            desired_before,
            "dry-run changed SQLite desired state for {args:?}"
        );
        assert_eq!(
            engine::snapshot(&self.project, &self.paths).unwrap(),
            project_before
        );
        assert_eq!(
            engine::snapshot(&self.paths.library, &self.paths).unwrap(),
            before
        );
        assert_eq!(fs::read(self.paths.home.join(".changed")).ok(), changed);
        args.pop();
        args.push("--yes");
        let real = self.run(&args);
        assert!(real.status.success(), "apply {args:?}: {real:?}");
        if json {
            let printed: Value = serde_json::from_slice(&real.stdout).unwrap();
            if let Some(plans) = printed["plans"].as_array() {
                let printed: Vec<_> = plans
                    .iter()
                    .flat_map(|plan| plan["actions"].as_array().unwrap().iter().cloned())
                    .collect();
                let recorded: Vec<String> = store
                    .read(|db| {
                        let mut query =
                            db.prepare("SELECT plan_json FROM ops WHERE id>?1 ORDER BY id")?;
                        Ok(query
                            .query_map([last_op], |row| row.get(0))?
                            .collect::<Result<_, _>>()?)
                    })
                    .unwrap();
                let recorded: Vec<Value> = recorded
                    .iter()
                    .flat_map(|text| {
                        let plan: Value = serde_json::from_str(text).unwrap();
                        plan["actions"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|action| action["action"].clone())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                assert_eq!(
                    printed, recorded,
                    "printed plan differs from engine journal for {args:?}"
                );
            }
        }
        let dry = self.comparable(&dry.stdout, json);
        let real = self.comparable(&real.stdout, json);
        assert_eq!(dry, real, "{args:?}");
        real
    }
}

#[test]
fn human_and_json_read_snapshots() {
    let f = Fixture::new();
    f.text(&["set", "create", "toolkit"]);
    f.text(&["set", "add", "toolkit", "review"]);
    f.text(&["project", "add", f.project.to_str().unwrap()]);
    let commands = [
        vec!["list"],
        vec!["list", "--kind", "skill", "--scope", "global"],
        vec!["show", "review"],
        vec!["status"],
        vec!["set", "list"],
        vec!["set", "show", "toolkit"],
        vec!["project", "list"],
        vec!["doctor"],
    ];
    let mut human = String::new();
    let mut json = String::new();
    for args in commands {
        human.push_str(&format!("$ skm {}\n{}\n", args.join(" "), f.text(&args)));
        let mut json_args = args.clone();
        json_args.push("--json");
        let output = f.run(&json_args);
        assert!(output.status.success());
        json.push_str(&format!(
            "$ skm {} --json\n{}\n",
            args.join(" "),
            f.comparable(&output.stdout, true)
        ));
    }
    insta::assert_snapshot!("read_human", human);
    insta::assert_snapshot!("read_json", json);
}

#[test]
fn every_mutation_previews_the_applied_plan() {
    for json in [false, true] {
        let f = Fixture::new();
        let project = f.project.to_str().unwrap();
        for args in [
            vec!["agent", "enable", "claude-code"],
            vec!["agent", "disable", "claude-code"],
            vec!["agents", "detect"],
            vec!["set", "create", "toolkit"],
            vec!["set", "add", "toolkit", "review"],
            vec!["set", "rm", "toolkit", "review"],
            vec!["set", "add", "toolkit", "review"],
            vec!["project", "add", project],
            vec!["on", "review", "--scope", project],
            vec!["off", "review", "--scope", project],
            vec!["project", "use", project, "toolkit"],
            vec!["project", "mode", project, "link"],
            vec!["project", "mode", project, "copy"],
            vec!["project", "pause", project],
            vec!["project", "resume", project],
            vec!["project", "unuse", project, "toolkit"],
            vec!["on", "review"],
            vec!["off", "review"],
            vec!["project", "remove", project, "--clean"],
        ] {
            let plan = f.dry_equals_apply(&args, json);
            if args == ["on", "review"] {
                if json {
                    insta::assert_snapshot!("on_plan_json", plan);
                } else {
                    insta::assert_snapshot!("on_plan_human", plan);
                }
            }
        }
        let foreign = f.home.join(".agents/skills/foreign");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("SKILL.md"), "foreign").unwrap();
        f.dry_equals_apply(&["adopt", "--all"], json);
        f.dry_equals_apply(&["undo"], json);
        let source = f.root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: imported\n---\nImported body.\n",
        )
        .unwrap();
        let plan = f.dry_equals_apply(
            &[
                "add",
                source.to_str().unwrap(),
                "--on",
                "global",
                "--set",
                "toolkit",
            ],
            json,
        );
        if json {
            insta::assert_snapshot!("install_plan_json", plan);
        }
        assert!(f.home.join(".agents/skills/imported").is_symlink());
        assert!(f.text(&["list"]).contains("imported"));
        f.dry_equals_apply(&["remove", "imported", "--off"], json);
        assert!(!f.paths.library.join("skills/imported").exists());
        f.dry_equals_apply(&["set", "delete", "toolkit"], json);
        f.dry_equals_apply(&["remove", "review"], json);
    }
}

#[test]
fn add_directly_into_copy_and_link_projects() {
    for mode in ["copy", "link"] {
        let f = Fixture::new();
        let project = f.project.to_str().unwrap();
        f.text(&["project", "add", project]);
        f.text(&["project", "mode", project, mode]);
        let source = f.root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: imported\n---\nImported body.\n",
        )
        .unwrap();
        f.dry_equals_apply(&["add", source.to_str().unwrap(), "--on", project], true);
        let target = f.project.join(".agents/skills/imported");
        assert!(target.is_dir());
        assert_eq!(target.is_symlink(), mode == "link");
        f.dry_equals_apply(&["remove", "imported", "--off"], true);
        assert!(!target.exists());
    }
}

#[test]
fn structured_errors_and_exit_codes() {
    let f = Fixture::new();
    for (args, code, error) in [
        (vec!["list", "--kind", "unknown", "--json"], 2, "E_USAGE"),
        (vec!["show", "missing", "--json"], 1, "E_INVALID_SKILL"),
        (
            vec!["agent", "enable", "unknown", "--json"],
            1,
            "E_CONFIG_PARSE",
        ),
    ] {
        let output = f.run(&args);
        assert_eq!(output.status.code(), Some(code));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], error);
        assert!(value["fix"].is_string());
    }
    f.text(&["on", "review"]);
    let output = f.run(&["remove", "review", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    insta::assert_snapshot!("refusal_json", f.comparable(&output.stdout, true));
    let target = f.home.join(".agents/skills/review");
    fs::remove_file(&target).unwrap();
    fs::write(&target, "foreign").unwrap();
    let output = f.run(&["on", "review", "--json"]);
    assert_eq!(output.status.code(), Some(3));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "E_FOREIGN_TARGET");
}

#[test]
fn paused_set_changes_wait_for_resume_and_reindex_preview_rolls_back() {
    let f = Fixture::new();
    let project = f.project.to_str().unwrap();
    f.text(&["set", "create", "toolkit"]);
    f.text(&["project", "add", project]);
    f.text(&["project", "use", project, "toolkit"]);
    f.text(&["project", "pause", project]);
    f.dry_equals_apply(&["set", "add", "toolkit", "review"], true);
    assert!(!f.project.join(".agents/skills/review").exists());
    let status: Value = serde_json::from_str(&f.text(&["status", "--json"])).unwrap();
    assert_eq!(status["pending"], 1);
    f.dry_equals_apply(&["project", "resume", project], true);
    assert!(f.project.join(".agents/skills/review").is_dir());
    let status: Value = serde_json::from_str(&f.text(&["status", "--json"])).unwrap();
    assert_eq!(status["pending"], 0);
    f.skill("new-skill", "New skill");
    let store = Store::open(&f.paths).unwrap();
    let before = indexer::load(&store).unwrap();
    let preview = f.text(&["doctor", "--reindex", "--dry-run", "--json"]);
    assert_eq!(indexer::load(&store).unwrap(), before);
    let applied = f.text(&["doctor", "--reindex", "--json"]);
    assert_eq!(preview, applied);
    assert_eq!(indexer::load(&store).unwrap().len(), 2);
}

#[test]
fn non_skill_selections_round_trip_and_project_plugins_refuse() {
    let f = Fixture::new();
    f.text(&["agent", "enable", "claude-code"]);
    f.text(&["set", "create", "toolkit"]);
    f.dry_equals_apply(
        &[
            "set",
            "add",
            "toolkit",
            "mcp:server",
            "--agent",
            "claude-code",
        ],
        true,
    );
    f.dry_equals_apply(&["on", "mcp:server", "--agent", "claude-code"], true);
    let library =
        skm_core::library::Library::load(&f.paths.library.join("library.toml"), &["claude-code"])
            .unwrap();
    assert_eq!(library.global.mcp["server"], ["claude-code"]);
    f.dry_equals_apply(&["off", "mcp:server"], true);
    f.dry_equals_apply(&["on", "plugin:tools", "--agent", "claude-code"], true);
    f.dry_equals_apply(&["off", "plugin:tools"], true);
    let project = f.project.to_str().unwrap();
    f.text(&["project", "add", project]);
    f.dry_equals_apply(
        &[
            "on",
            "mcp:server",
            "--scope",
            project,
            "--agent",
            "claude-code",
        ],
        true,
    );
    f.dry_equals_apply(&["off", "mcp:server", "--scope", project], true);
    let output = f.run(&["on", "plugin:tools", "--scope", project, "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let output: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output["code"], "E_CONFIG_PARSE");
}

#[test]
fn installation_preserves_project_owned_manifest_prerequisites() {
    let f = Fixture::new();
    let foreign = f.project.join(".agents/skills/foreign");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("SKILL.md"), "Foreign body.\n").unwrap();
    f.text(&[
        "adopt",
        foreign.to_str().unwrap(),
        "--project",
        f.project.to_str().unwrap(),
    ]);
    let source = f.root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: imported\n---\nImported body.\n",
    )
    .unwrap();
    f.dry_equals_apply(&["add", source.to_str().unwrap(), "--on", "global"], true);
    assert!(f.paths.library.join("skills/foreign").is_dir());
    assert!(f.paths.library.join("skills/imported").is_dir());
}

#[test]
fn registration_previews_claims_and_unregister_preserves_files() {
    let f = Fixture::new();
    let target = f.project.join(".agents/skills/review");
    fs::create_dir_all(&target).unwrap();
    fs::copy(
        f.paths.library.join("skills/review/SKILL.md"),
        target.join("SKILL.md"),
    )
    .unwrap();
    let before = engine::snapshot(&target, &f.paths).unwrap();
    let project = f.project.to_str().unwrap();
    let plan = f.dry_equals_apply(&["project", "add", project], true);
    assert!(plan.contains("Claim"));
    f.dry_equals_apply(&["project", "remove", project], true);
    assert_eq!(engine::snapshot(&target, &f.paths).unwrap(), before);
    let store = Store::open(&f.paths).unwrap();
    assert!(skm_core::projects::list(&store).unwrap().is_empty());
}

#[test]
fn unscoped_list_uses_the_index_without_reparsing_library_files() {
    let f = Fixture::new();
    fs::write(f.paths.library.join("library.toml"), "not valid TOML [").unwrap();
    fs::remove_file(f.paths.library.join("skills/review/SKILL.md")).unwrap();
    let output = f.text(&["list"]);
    assert!(output.contains("review"));
    assert!(output.contains("Review code"));
    assert_eq!(f.run(&["list", "--scope", "global"]).status.code(), Some(1));
}

#[test]
fn search_and_agents_have_human_and_json_snapshots() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .starts_with("GET /api/search?q=review&limit=50 ")
            );
            let body = r#"{"skills":[{"source":"acme/tools","skillId":"review","name":"Code review","installs":123}]}"#;
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    let mut snapshots = Vec::new();
    for json in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_skm"));
        command
            .arg("--home")
            .arg(&f.paths.home)
            .args(["search", "review"])
            .env("HOME", &f.home)
            .env("SKILLSMGR_SKILLS_API", &address)
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY");
        if json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        snapshots.push(f.comparable(&output.stdout, json));
    }
    server.join().unwrap();
    insta::assert_snapshot!("search_human", snapshots[0]);
    insta::assert_snapshot!("search_json", snapshots[1]);
    for json in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_skm"));
        command
            .arg("--home")
            .arg(&f.paths.home)
            .args(["agents", "detect"])
            .env("HOME", &f.home)
            .env("PATH", "");
        if json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        if json {
            insta::assert_snapshot!("agents_json", f.comparable(&output.stdout, true));
        } else {
            insta::assert_snapshot!("agents_human", f.comparable(&output.stdout, false));
        }
    }
}

#[test]
fn partial_failure_is_one_structured_document_and_stale_undo_refuses_in_preview() {
    let f = Fixture::new();
    let store = Store::open(&f.paths).unwrap();
    store.write(|db|{db.execute_batch("CREATE TRIGGER fail_ledger BEFORE INSERT ON ledger BEGIN SELECT RAISE(FAIL, 'test database failure'); END;")?;Ok(())}).unwrap();
    let output = f.run(&["on", "review", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "E_DATABASE");
    assert!(value["failed"].as_array().unwrap().len() == 1);
    store
        .write(|db| {
            db.execute_batch("DROP TRIGGER fail_ledger;")?;
            Ok(())
        })
        .unwrap();
    let target = f.home.join(".agents/skills/review");
    if target.is_symlink() {
        fs::remove_file(&target).unwrap();
    }
    f.text(&["on", "review"]);
    fs::remove_file(&target).unwrap();
    fs::write(&target, "edited").unwrap();
    for args in [vec!["undo", "--json", "--dry-run"], vec!["undo", "--json"]] {
        let output = f.run(&args);
        assert_eq!(output.status.code(), Some(1));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["code"], "E_CONFIG_CHANGED");
    }
}

#[test]
#[ignore = "cargo test -p skm-cli --release --test v1 list_1000_skills_cold -- --ignored --nocapture"]
fn list_1000_skills_cold() {
    if cfg!(debug_assertions) {
        panic!("run this benchmark with --release");
    }
    let f = Fixture::new();
    for n in 1..1000 {
        f.skill(&format!("skill-{n:04}"), "Benchmark skill");
    }
    let mut library = skm_core::library::Library::default();
    for name in
        std::iter::once("review".to_owned()).chain((1..1000).map(|n| format!("skill-{n:04}")))
    {
        library.skills.insert(
            name.clone(),
            skm_core::library::SkillProvenance {
                source: skm_core::library::SkillSource::Local {
                    origin: Some(f.paths.library.join("skills").join(name)),
                },
                rev: None,
                base_hash: None,
                installed_at: None,
            },
        );
    }
    library
        .save(&f.paths.library.join("library.toml"), &[])
        .unwrap();
    let store = Store::open(&f.paths).unwrap();
    indexer::full_reindex(&store, &f.paths).unwrap();
    drop(store);
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let output = f.run(&["list"]);
        let elapsed = start.elapsed();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        assert_eq!(output.lines().count(), 1001);
        assert!(output.contains("skill-0999"));
        times.push(elapsed);
    }
    println!("cold-process list, indexed 1000 skills, 5 runs, output fully captured: {times:?}");
    assert!(
        times.iter().all(|time| *time <= Duration::from_millis(50)),
        "{times:?}"
    );
}
