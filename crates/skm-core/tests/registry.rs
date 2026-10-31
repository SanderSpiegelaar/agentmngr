use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::Path;

use skm_core::config::{AgentPathOverrides, Config, FilePathOverride};
use skm_core::registry::{EMBEDDED_REGISTRY, Registry};

#[test]
fn embedded_registry_has_six_agents_and_spec_targets() {
    let registry = Registry::load().unwrap();
    assert_eq!(registry.agents.len(), 6);
    let claude = &registry.agents["claude-code"];
    assert!(!claude.skills.reads_universal);
    let note = claude.skills.note.as_ref().unwrap();
    assert!(note.contains("31005") && note.contains("16345"));
    assert_eq!(
        claude
            .mcp
            .as_ref()
            .unwrap()
            .project
            .as_ref()
            .unwrap()
            .also_read_by,
        ["copilot-cli"]
    );
    for (id, mcp, hooks, agents) in [
        (
            "claude-code",
            ".mcp.json",
            Some(".claude/settings.json"),
            ".claude/agents",
        ),
        (
            "codex",
            ".codex/config.toml",
            Some(".codex/hooks.json"),
            ".codex/agents",
        ),
        (
            "cursor",
            ".cursor/mcp.json",
            Some(".cursor/hooks.json"),
            ".cursor/agents",
        ),
        (
            "gemini-cli",
            ".gemini/settings.json",
            Some(".gemini/settings.json"),
            ".gemini/agents",
        ),
        ("opencode", "opencode.json", None, ".opencode/agents"),
        (
            "copilot-cli",
            ".github/mcp.json",
            Some(".github/hooks/skillsmgr.json"),
            ".github/agents",
        ),
    ] {
        let agent = &registry.agents[id];
        assert_eq!(
            agent.mcp.as_ref().unwrap().project.as_ref().unwrap().file,
            Path::new(mcp)
        );
        assert_eq!(
            agent
                .hooks
                .as_ref()
                .map(|h| h.project.as_ref().unwrap().file.as_path()),
            hooks.map(Path::new)
        );
        assert_eq!(
            agent.subagents.as_ref().unwrap().project.as_deref(),
            Some(Path::new(agents))
        );
        assert_eq!(agent.skills.project, Path::new(".agents/skills"));
    }
}

#[test]
fn load_rejects_absolute_escaping_paths_unknown_readers_and_duplicate_ids() {
    for invalid in ["/absolute", "~/absolute", "../escape", "C:\\absolute"] {
        let text = EMBEDDED_REGISTRY.replace(
            "project = \".agents/skills\"",
            &format!("project = {invalid:?}"),
        );
        assert!(Registry::from_toml(&text).is_err(), "{invalid}");
    }
    for original in [".mcp.json", ".claude/settings.json", ".claude/agents"] {
        let text = EMBEDDED_REGISTRY.replace(&format!("\"{original}\""), "\"/absolute\"");
        assert!(Registry::from_toml(&text).is_err());
    }
    assert!(
        Registry::from_toml(&EMBEDDED_REGISTRY.replace(
            "also_read_by = [\"cursor\"]",
            "also_read_by = [\"missing\"]"
        ))
        .is_err()
    );
    assert!(
        Registry::from_toml(&format!(
            "{EMBEDDED_REGISTRY}\n[agents.codex]\nname = 'Duplicate'\n"
        ))
        .is_err()
    );
}

#[test]
fn detection_uses_fake_home_and_path_and_keeps_enabled_independent() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join(".claude")).unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let executable = bin.join("codex");
    // An invalid executable would fail if detection tried to spawn it.
    std::fs::write(&executable, "not a program").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(bin.join("cursor"), "not executable").unwrap();
        std::fs::set_permissions(bin.join("cursor"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
    }
    std::fs::create_dir(bin.join("gemini")).unwrap();
    let registry = Registry::load()
        .unwrap()
        .resolve(home.path(), &BTreeMap::new(), &Config::default())
        .unwrap();
    let statuses = registry
        .detect(
            &std::env::join_paths([bin]).unwrap(),
            &BTreeSet::from(["opencode".into()]),
        )
        .unwrap();
    for status in statuses {
        assert_eq!(
            status.detected,
            matches!(status.id.as_str(), "claude-code" | "codex")
        );
        assert_eq!(status.enabled, status.id == "opencode");
    }
}

#[test]
fn environment_then_config_overrides_resolve_user_paths_and_validate_project_paths() {
    let home = tempfile::tempdir().unwrap();
    let claude = home.path().join("custom-claude");
    let copilot = home.path().join("custom-copilot");
    std::fs::create_dir(&claude).unwrap();
    std::fs::create_dir(&copilot).unwrap();
    let env = BTreeMap::from([
        ("CLAUDE_CONFIG_DIR".into(), claude.clone().into_os_string()),
        ("COPILOT_HOME".into(), copilot.clone().into_os_string()),
    ]);
    let registry = Registry::load().unwrap();
    let resolved = registry
        .resolve(home.path(), &env, &Config::default())
        .unwrap();
    assert_eq!(
        resolved.agents["claude-code"]
            .mcp
            .as_ref()
            .unwrap()
            .user
            .as_ref()
            .unwrap()
            .file,
        claude.join(".claude.json")
    );
    assert_eq!(
        resolved.agents["claude-code"]
            .hooks
            .as_ref()
            .unwrap()
            .user
            .as_ref()
            .unwrap()
            .file,
        claude.join("settings.json")
    );
    assert_eq!(
        resolved.agents["copilot-cli"]
            .subagents
            .as_ref()
            .unwrap()
            .user
            .as_ref()
            .unwrap(),
        &copilot.join("agents")
    );
    let statuses = resolved.detect(OsStr::new(""), &BTreeSet::new()).unwrap();
    assert_eq!(statuses.iter().filter(|s| s.detected).count(), 2);
    let mut overrides = AgentPathOverrides {
        detect: Some(vec!["~/not-installed".into()]),
        ..Default::default()
    };
    overrides.mcp.user = Some(FilePathOverride {
        file: "~/explicit.json".into(),
    });
    overrides.hooks.project = Some(FilePathOverride {
        file: ".custom/hooks.json".into(),
    });
    overrides.subagents.user = Some("~/explicit-agents".into());
    overrides.subagents.project = Some(".custom/agents".into());
    let mut config = Config::default();
    config.agents.insert("claude-code".into(), overrides);
    let resolved = registry.resolve(home.path(), &env, &config).unwrap();
    let agent = &resolved.agents["claude-code"];
    assert_eq!(
        agent.mcp.as_ref().unwrap().user.as_ref().unwrap().file,
        home.path().join("explicit.json")
    );
    assert_eq!(
        agent.hooks.as_ref().unwrap().project.as_ref().unwrap().file,
        Path::new(".custom/hooks.json")
    );
    assert_eq!(
        agent.subagents.as_ref().unwrap().user.as_ref().unwrap(),
        &home.path().join("explicit-agents")
    );
    assert_eq!(
        agent.subagents.as_ref().unwrap().project.as_deref(),
        Some(Path::new(".custom/agents"))
    );
    assert!(
        !resolved
            .detect(OsStr::new(""), &BTreeSet::new())
            .unwrap()
            .iter()
            .find(|s| s.id == "claude-code")
            .unwrap()
            .detected
    );
    config
        .agents
        .get_mut("claude-code")
        .unwrap()
        .hooks
        .project
        .as_mut()
        .unwrap()
        .file = "/invalid".into();
    assert!(registry.resolve(home.path(), &env, &config).is_err());
}

#[test]
fn seventh_agent_is_toml_only_and_detects_without_code_changes() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join(".seventh")).unwrap();
    let text = format!(
        "{EMBEDDED_REGISTRY}\n[agents.seventh]\nname = 'Seventh'\ndetect = ['~/.seventh', 'bin:seventh']\nskills = {{ reads_universal = true, user = '~/.agents/skills', project = '.agents/skills' }}\n"
    );
    let registry = Registry::from_toml(&text)
        .unwrap()
        .resolve(home.path(), &BTreeMap::new(), &Config::default())
        .unwrap();
    assert_eq!(registry.agents.len(), 7);
    assert!(
        registry
            .detect(OsStr::new(""), &BTreeSet::new())
            .unwrap()
            .iter()
            .find(|s| s.id == "seventh")
            .unwrap()
            .detected
    );
}
