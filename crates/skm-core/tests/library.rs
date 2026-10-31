use std::path::Path;
use std::process::Command;

use skm_core::library::{Library, SkillProvenance, SkillSource};

const AGENTS: &[&str] = &["claude-code", "codex", "cursor", "gemini"];
const LIBRARY: &str = r#"# Library header
schema = 1
future = "keep me"

[global]
skills = ["zebra", "alpha"] # intentionally not sorted
mcp = { github = ["codex", "claude-code"] }
plugins = { tools = ["cursor"] }

# First skill stays first
[skills.zebra]
source = { kind = "local", origin = "/work/zebra", future = true }
rev = "old-zebra" # keep zebra comment

# Second skill stays second
[skills.alpha]
source = { kind = "git", url = "https://example.com/skills", ref = "main", path = "skills/alpha" }
rev = "old-alpha" # keep alpha comment
base_hash = "sha256:abc"
installed_at = 2026-10-31T18:00:00+01:00

[sets.rust]
description = "Rust projects"
skills = ["zebra", "alpha"]
mcp = { docs = ["*"] }
hooks = { fmt = ["claude-code", "codex"] }
subagents = { reviewer = ["gemini"] }
"#;

#[test]
fn round_trip_preserves_comments_ordering_and_unknown_fields() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("library.toml");
    std::fs::write(&path, LIBRARY).unwrap();
    let mut library = Library::load(&path, AGENTS).unwrap();
    library.save(&path, AGENTS).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), LIBRARY);

    library.skills.get_mut("alpha").unwrap().rev = Some("new-alpha".into());
    library.save(&path, AGENTS).unwrap();
    let updated = std::fs::read_to_string(&path).unwrap();
    assert_eq!(updated, LIBRARY.replace("old-alpha", "new-alpha"));
    assert_eq!(Library::load(&path, AGENTS).unwrap(), library);
    assert!(!path.with_extension("toml.skm-tmp").exists());
}

#[test]
fn multi_kind_set_validates_agents_and_wildcards() {
    let library = Library::parse(LIBRARY, AGENTS).unwrap();
    let set = &library.sets["rust"].items;
    assert_eq!(set.skills, ["zebra", "alpha"]);
    assert_eq!(set.mcp["docs"], ["*"]);
    assert_eq!(set.hooks["fmt"], ["claude-code", "codex"]);
    assert_eq!(set.subagents["reviewer"], ["gemini"]);
    for (known, unknown) in [
        ("gemini", "unknown-subagent"),
        ("claude-code", "unknown-hook"),
        ("cursor", "unknown-plugin"),
        ("codex", "unknown-mcp"),
    ] {
        let error = Library::parse(&LIBRARY.replace(known, unknown), AGENTS).unwrap_err();
        assert_eq!(error.code(), "E_CONFIG_PARSE");
        assert!(error.to_string().contains(unknown));
    }
    assert!(Library::parse("schema = 2", AGENTS).is_err());
    assert!(Library::parse("schema = [", AGENTS).is_err());
    assert!(Library::parse("[global]", AGENTS).is_err());
    assert!(
        Library::parse(
            "schema = 1\n[sets.rust]\nplugins = { tools = ['cursor'] }",
            AGENTS
        )
        .is_err()
    );
}

#[test]
fn generated_output_is_sorted_and_provenance_round_trips() {
    let mut library = Library::parse(LIBRARY, AGENTS).unwrap();
    library.skills.insert(
        "beta".into(),
        SkillProvenance {
            source: SkillSource::Local { origin: None },
            rev: None,
            base_hash: None,
            installed_at: None,
        },
    );
    let text = library.render("", AGENTS).unwrap();
    assert!(text.contains("skills = [\"alpha\", \"zebra\"]"));
    assert!(text.contains("github = [\"claude-code\", \"codex\"]"));
    assert!(text.find("[skills.alpha]").unwrap() < text.find("[skills.beta]").unwrap());
    assert!(text.find("[skills.beta]").unwrap() < text.find("[skills.zebra]").unwrap());
    assert!(text.contains("source = { kind = \"git\""));
    assert!(!text.contains("[skills.alpha.source]"));
    let parsed = Library::parse(&text, AGENTS).unwrap();
    assert_eq!(parsed.skills, library.skills);
    assert_eq!(parsed.render("", AGENTS).unwrap(), text);
    assert_eq!(parsed.render(&text, AGENTS).unwrap(), text);
}

#[test]
fn creates_a_library_and_adds_or_removes_individual_items() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("library.toml");
    Library::default().save(&path, AGENTS).unwrap();
    assert_eq!(Library::load(&path, AGENTS).unwrap(), Library::default());

    let mut library = Library::parse(LIBRARY, AGENTS).unwrap();
    library
        .skills
        .insert("new-skill".into(), library.skills["alpha"].clone());
    library.skills.remove("zebra");
    library
        .sets
        .insert("new-set".into(), library.sets["rust"].clone());
    let text = library.render(LIBRARY, AGENTS).unwrap();
    assert!(!text.contains("[skills.zebra]"));
    assert!(text.contains("[skills.new-skill]"));
    assert!(text.contains("[sets.new-set]"));
    library.sets.get_mut("new-set").unwrap().items.skills.sort();
    assert_eq!(Library::parse(&text, AGENTS).unwrap(), library);
}

#[test]
fn edits_and_removals_preserve_unknown_inline_fields() {
    let mut library = Library::parse(LIBRARY, AGENTS).unwrap();
    if let SkillSource::Local { origin } = &mut library.skills.get_mut("zebra").unwrap().source {
        *origin = Some("/new/zebra".into());
    }
    library.skills.get_mut("alpha").unwrap().base_hash = None;
    library
        .sets
        .get_mut("rust")
        .unwrap()
        .items
        .mcp
        .insert("docs".into(), vec!["codex".into()]);
    let text = library.render(LIBRARY, AGENTS).unwrap();
    assert!(text.contains("future = true"));
    assert!(text.contains("future = \"keep me\""));
    assert!(text.contains("origin = \"/new/zebra\""));
    assert!(!text.contains("base_hash"));
    assert_eq!(Library::parse(&text, AGENTS).unwrap(), library);
}

#[test]
fn failed_validation_and_io_do_not_overwrite_the_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("library.toml");
    let mut library = Library::parse(LIBRARY, AGENTS).unwrap();
    std::fs::write(&path, LIBRARY).unwrap();
    library.schema = 2;
    assert!(library.save(&path, AGENTS).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), LIBRARY);
    assert!(Library::load(root.path(), AGENTS).is_err());
    assert!(
        Library::default()
            .save(&root.path().join("missing/library.toml"), AGENTS)
            .is_err()
    );
    std::fs::write(&path, "schema = [").unwrap();
    assert!(Library::default().save(&path, AGENTS).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "schema = [");
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Library test")
        .env("GIT_AUTHOR_EMAIL", "library@example.invalid")
        .env("GIT_COMMITTER_NAME", "Library test")
        .env("GIT_COMMITTER_EMAIL", "library@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn independent_item_edits_merge_cleanly_in_git() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("library.toml");
    git(root.path(), &["init", "-b", "base"]);
    std::fs::write(&path, LIBRARY).unwrap();
    git(root.path(), &["add", "library.toml"]);
    git(root.path(), &["commit", "-m", "base"]);

    for (branch, skill, rev) in [
        ("edit-alpha", "alpha", "new-alpha"),
        ("edit-zebra", "zebra", "new-zebra"),
    ] {
        git(root.path(), &["checkout", "-b", branch, "base"]);
        let mut library = Library::load(&path, AGENTS).unwrap();
        library.skills.get_mut(skill).unwrap().rev = Some(rev.into());
        library.save(&path, AGENTS).unwrap();
        git(root.path(), &["add", "library.toml"]);
        git(root.path(), &["commit", "-m", branch]);
    }
    let output = git(root.path(), &["merge", "--no-edit", "edit-alpha"]);
    println!("{output}");
    assert!(git(root.path(), &["ls-files", "--unmerged"]).is_empty());
    let merged = Library::load(&path, AGENTS).unwrap();
    assert_eq!(merged.skills["alpha"].rev.as_deref(), Some("new-alpha"));
    assert_eq!(merged.skills["zebra"].rev.as_deref(), Some("new-zebra"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        LIBRARY
            .replace("old-alpha", "new-alpha")
            .replace("old-zebra", "new-zebra")
    );
}
