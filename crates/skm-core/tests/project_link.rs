use std::{fs, path::Path, process::Command};

use skm_core::{
    engine::{self, Action},
    library::Library,
    paths::Paths,
    projects::{self, Project, ProjectMode},
    registry::Registry,
    store::Store,
    target::{ItemKind, ItemRef},
};

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(repo)
        .env("HOME", repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.join("missing-config"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

struct Fixture {
    _temp: tempfile::TempDir,
    paths: Paths,
    store: Store,
    registry: Registry,
    project: Project,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
        paths.ensure_layout().unwrap();
        let store = Store::open(&paths).unwrap();
        let registry = Registry::load().unwrap();
        let repo = root.join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        let project = projects::register(&store, &repo).unwrap();
        let source = paths.library.join("skills/alpha");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("SKILL.md"), "alpha\n").unwrap();
        fs::write(source.join("nested/payload"), [0, 255, 42, 10]).unwrap();
        projects::set_direct_item(
            &store,
            &registry,
            project.id,
            &ItemRef {
                kind: ItemKind::Skill,
                name: "alpha".into(),
            },
            None,
        )
        .unwrap();
        Self {
            _temp: temp,
            paths,
            store,
            registry,
            project,
        }
    }

    fn mode(&mut self, mode: ProjectMode) {
        projects::set_mode(&self.store, self.project.id, mode).unwrap();
        self.project.mode = mode;
    }

    fn plan(&self) -> engine::Plan {
        projects::plan_project(
            &self.paths,
            &self.store,
            &self.project,
            &Library::default(),
            &self.registry,
        )
        .unwrap()
    }

    fn apply(&self) {
        let result = projects::apply_project(
            &self.paths,
            &self.store,
            &self.project,
            &Library::default(),
            &self.registry,
            "test",
            None,
        )
        .unwrap();
        assert!(result.issues.is_empty(), "{:?}", result.issues);
        assert!(result.failed.is_empty(), "{:?}", result.failed);
    }
}

#[test]
fn link_mode_is_git_clean_and_exclude_contains_only_created_paths_then_disappears() {
    let mut f = Fixture::new();
    f.mode(ProjectMode::Link);
    let repo = &f.project.path;
    let exclude = repo.join(".git/info/exclude");
    let original = b"# keep this\r\nlocal-only\n# non-UTF8 \xff and no trailing newline";
    fs::write(&exclude, original).unwrap();
    let preview = f.plan();
    assert!(preview.issues.is_empty());
    assert!(!repo.join(".agents").exists());
    assert_eq!(fs::read(&exclude).unwrap(), original);
    f.apply();
    assert_eq!(
        fs::read_link(repo.join(".agents/skills/alpha")).unwrap(),
        f.paths.library.join("skills/alpha")
    );
    assert!(!repo.join("skills-lock.json").exists());
    assert!(git(repo, &["status", "--porcelain", "--untracked-files=all"]).is_empty());
    assert!(git(repo, &["diff", "--cached", "--name-only"]).is_empty());
    let expected = [
        b"# BEGIN skillsmgr managed paths\n/.agents/skills/alpha\n# END skillsmgr managed paths\n"
            .as_slice(),
        original,
    ]
    .concat();
    assert_eq!(fs::read(&exclude).unwrap(), expected);
    assert!(f.plan().actions.is_empty());

    fs::write(repo.join(".agents/skills/foreign"), "not ours").unwrap();
    assert_eq!(
        git(repo, &["status", "--porcelain", "--untracked-files=all"]),
        b"?? .agents/skills/foreign\n"
    );
    projects::remove_direct_item(
        &f.store,
        f.project.id,
        &ItemRef {
            kind: ItemKind::Skill,
            name: "alpha".into(),
        },
    )
    .unwrap();
    f.apply();
    assert!(!repo.join(".agents/skills/alpha").exists());
    assert_eq!(fs::read(&exclude).unwrap(), original);
    assert_eq!(
        fs::read(repo.join(".agents/skills/foreign")).unwrap(),
        b"not ours"
    );
    assert!(f.plan().actions.is_empty());
}

#[test]
fn copy_link_copy_round_trip_restores_exact_bytes_and_lock() {
    let mut f = Fixture::new();
    f.apply();
    let repo = f.project.path.clone();
    let lock = fs::read(repo.join("skills-lock.json")).unwrap();
    let original_exclude = fs::read(repo.join(".git/info/exclude")).unwrap();
    let before = engine::snapshot(&repo.join(".agents/skills/alpha"), &f.paths).unwrap();
    f.mode(ProjectMode::Link);
    let preview = f.plan();
    assert!(
        preview
            .actions
            .iter()
            .any(|action| matches!(&action.action, Action::Link { .. }))
    );
    assert!(preview.actions.iter().any(|action| matches!(&action.action, Action::Trash { target } if target == &repo.join("skills-lock.json"))));
    assert_eq!(fs::read(repo.join("skills-lock.json")).unwrap(), lock);
    f.apply();
    assert!(!repo.join("skills-lock.json").exists());
    assert_eq!(
        fs::read(repo.join(".agents/skills/alpha/nested/payload")).unwrap(),
        [0, 255, 42, 10]
    );
    assert!(git(&repo, &["status", "--porcelain", "--untracked-files=all"]).is_empty());
    f.mode(ProjectMode::Copy);
    assert!(
        f.plan()
            .actions
            .iter()
            .any(|action| matches!(action.action, Action::CopyTree { .. }))
    );
    f.apply();
    assert_eq!(
        engine::snapshot(&repo.join(".agents/skills/alpha"), &f.paths).unwrap(),
        before
    );
    assert_eq!(fs::read(repo.join("skills-lock.json")).unwrap(), lock);
    assert_eq!(
        fs::read(repo.join(".git/info/exclude")).unwrap(),
        original_exclude
    );
    assert!(f.plan().actions.is_empty());
}

#[test]
fn mode_switch_preserves_foreign_lock_entries_and_unknown_fields() {
    let mut f = Fixture::new();
    let target = f.project.path.join("skills-lock.json");
    let foreign = "{  \"source\": \"elsewhere\", \"computedHash\":\"keep\", \"custom\" : [1,  2] }";
    fs::write(
        &target,
        format!("{{\"version\":1,\"skills\":{{\"foreign\":{foreign}}},\"unknown\":true}}"),
    )
    .unwrap();
    f.apply();
    let lock = fs::read(&target).unwrap();
    f.mode(ProjectMode::Link);
    f.apply();
    let text = fs::read_to_string(&target).unwrap();
    assert!(text.contains(foreign));
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["skills"].as_object().unwrap().len(), 1);
    assert_eq!(value["unknown"], true);
    f.mode(ProjectMode::Copy);
    f.apply();
    assert_eq!(fs::read(&target).unwrap(), lock);
}

#[test]
fn stale_exclude_and_failed_skill_changes_do_not_publish_metadata() {
    let mut f = Fixture::new();
    f.mode(ProjectMode::Link);
    let exclude = f.project.path.join(".git/info/exclude");
    let plan = f.plan();
    fs::write(&exclude, "teammate-rule\n").unwrap();
    let result = engine::apply(&f.paths, &f.store, &plan, "test", None).unwrap();
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.code == "E_CONFIG_CHANGED")
    );
    assert_eq!(fs::read(&exclude).unwrap(), b"teammate-rule\n");
    f.apply();
    let original = fs::read(&exclude).unwrap();
    f.mode(ProjectMode::Copy);
    let plan = f.plan();
    fs::write(
        f.paths.library.join("skills/alpha/SKILL.md"),
        "changed after preview",
    )
    .unwrap();
    let result = engine::apply(&f.paths, &f.store, &plan, "test", None).unwrap();
    assert!(
        result
            .issues
            .iter()
            .chain(&result.failed)
            .any(|issue| issue.code == "E_CONFIG_CHANGED")
    );
    assert_eq!(fs::read(&exclude).unwrap(), original);
    assert!(!f.project.path.join("skills-lock.json").exists());
}

#[test]
fn edited_copy_refuses_mode_switch_without_touching_lock_or_excludes() {
    let mut f = Fixture::new();
    f.apply();
    let repo = &f.project.path;
    let lock = fs::read(repo.join("skills-lock.json")).unwrap();
    let exclude = fs::read(repo.join(".git/info/exclude")).unwrap();
    fs::write(repo.join(".agents/skills/alpha/SKILL.md"), "local edit").unwrap();
    f.mode(ProjectMode::Link);
    let plan = f.plan();
    assert!(plan.actions.is_empty());
    assert!(
        plan.issues
            .iter()
            .any(|issue| issue.code == "E_PROJECT_EDITED")
    );
    assert_eq!(
        fs::read(f.project.path.join("skills-lock.json")).unwrap(),
        lock
    );
    assert_eq!(
        fs::read(f.project.path.join(".git/info/exclude")).unwrap(),
        exclude
    );
    assert_eq!(
        fs::read(f.project.path.join(".agents/skills/alpha/SKILL.md")).unwrap(),
        b"local edit"
    );
}

#[test]
fn linked_git_worktree_uses_git_resolved_exclude_path() {
    let mut f = Fixture::new();
    git(
        &f.project.path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let worktree = f.project.path.parent().unwrap().join("linked");
    git(
        &f.project.path,
        &[
            "worktree",
            "add",
            "-b",
            "linked",
            worktree.to_str().unwrap(),
        ],
    );
    let exclude = f.project.path.join(".git/info/exclude");
    let original = fs::read(&exclude).unwrap();
    f.project = projects::register(&f.store, &worktree).unwrap();
    projects::set_direct_item(
        &f.store,
        &f.registry,
        f.project.id,
        &ItemRef {
            kind: ItemKind::Skill,
            name: "alpha".into(),
        },
        None,
    )
    .unwrap();
    f.mode(ProjectMode::Link);
    f.apply();
    assert!(
        git(
            &worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        )
        .is_empty()
    );
    assert!(
        fs::read_to_string(&exclude)
            .unwrap()
            .contains("/.agents/skills/alpha\n")
    );
    f.mode(ProjectMode::Copy);
    f.apply();
    assert_eq!(fs::read(&exclude).unwrap(), original);
}

#[test]
fn exclude_patterns_escape_skill_names_without_hiding_other_paths() {
    let mut f = Fixture::new();
    let name = "star*[x] ";
    fs::create_dir_all(f.paths.library.join("skills").join(name)).unwrap();
    fs::write(
        f.paths.library.join("skills").join(name).join("SKILL.md"),
        "odd name",
    )
    .unwrap();
    projects::set_direct_item(
        &f.store,
        &f.registry,
        f.project.id,
        &ItemRef {
            kind: ItemKind::Skill,
            name: name.into(),
        },
        None,
    )
    .unwrap();
    f.mode(ProjectMode::Link);
    f.apply();
    assert!(
        git(
            &f.project.path,
            &["status", "--porcelain", "--untracked-files=all"]
        )
        .is_empty()
    );
    fs::write(f.project.path.join(".agents/skills/starZZx"), "foreign").unwrap();
    assert_eq!(
        git(
            &f.project.path,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        b"?? .agents/skills/starZZx\n"
    );
}

#[test]
fn empty_link_project_creates_nothing_and_removes_an_empty_owned_copy_lock() {
    let mut f = Fixture::new();
    projects::remove_direct_item(
        &f.store,
        f.project.id,
        &ItemRef {
            kind: ItemKind::Skill,
            name: "alpha".into(),
        },
    )
    .unwrap();
    f.mode(ProjectMode::Link);
    let before = fs::read(f.project.path.join(".git/info/exclude")).unwrap();
    assert!(f.plan().actions.is_empty());
    assert!(!f.project.path.join(".agents").exists());
    assert!(!f.project.path.join("skills-lock.json").exists());
    assert_eq!(
        fs::read(f.project.path.join(".git/info/exclude")).unwrap(),
        before
    );

    projects::set_direct_item(
        &f.store,
        &f.registry,
        f.project.id,
        &ItemRef {
            kind: ItemKind::Skill,
            name: "alpha".into(),
        },
        None,
    )
    .unwrap();
    f.mode(ProjectMode::Copy);
    f.apply();
    projects::remove_direct_item(
        &f.store,
        f.project.id,
        &ItemRef {
            kind: ItemKind::Skill,
            name: "alpha".into(),
        },
    )
    .unwrap();
    f.apply();
    assert!(f.project.path.join("skills-lock.json").exists());
    f.mode(ProjectMode::Link);
    f.apply();
    assert!(!f.project.path.join("skills-lock.json").exists());
}
