use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use skm_core::{
    content_hash::content_hash,
    engine,
    git::GitRunner,
    install::{self, CollisionChoice, InstallResult, InstalledSkill, Preview, Selection},
    library::{Library, SkillSource},
    paths::Paths,
    source::Source,
    store::Store,
};

fn setup(root: &Path) -> (Paths, Store, GitRunner) {
    let mut paths = Paths::resolve(Some(&root.join("manager")), None, None).unwrap();
    paths.ensure_layout().unwrap();
    let store = Store::open(&paths).unwrap();
    let git = GitRunner::new(Duration::from_secs(10)).unwrap();
    (paths, store, git)
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("missing-config"))
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn skill(root: &Path, name: &str) -> PathBuf {
    let directory = root.join(name);
    fs::create_dir_all(directory.join("scripts")).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Test {name}\n---\nBody\n"),
    )
    .unwrap();
    fs::write(
        directory.join("scripts/run.sh"),
        b"#!/bin/sh\nprintf hello\n",
    )
    .unwrap();
    directory
}

fn fixture(root: &Path) -> Source {
    let repository = root.join("repo");
    fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-b", "main"]);
    for name in ["alpha", "beta", "gamma"] {
        skill(&repository.join("skills"), name);
    }
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "-m", "skills"]);
    Source::Git {
        url: format!("file://{}", repository.display()),
        reference: Some("main".into()),
        path: None,
    }
}

fn selection(preview: &Preview, name: &str, collision: CollisionChoice) -> Selection {
    Selection {
        skill: preview
            .skills()
            .iter()
            .position(|item| item.name == name)
            .unwrap(),
        collision,
    }
}

fn applied(result: InstallResult) -> Vec<InstalledSkill> {
    match result {
        InstallResult::Applied { items, result } => {
            assert!(result.issues.is_empty(), "{:?}", result.issues);
            assert!(result.failed.is_empty(), "{:?}", result.failed);
            items
        }
        other => panic!("expected applied install, got {other:?}"),
    }
}

#[test]
fn git_installs_only_selected_skills_with_provenance_and_pristine_bases() {
    let root = tempfile::tempdir().unwrap();
    let (paths, store, git) = setup(root.path());
    let source = fixture(root.path());
    let preview = install::preview(&paths, &git, &source).unwrap();
    assert_eq!(preview.skills().len(), 3);
    let alpha = &preview.skills()[0];
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.path, Path::new("skills/alpha"));
    assert_eq!(alpha.description.as_deref(), Some("Test alpha"));
    assert!(alpha.has_scripts);
    assert_eq!(alpha.files.len(), 2);
    assert_eq!(
        alpha.size,
        alpha
            .files
            .iter()
            .map(
                |file| fs::metadata(root.path().join("repo/skills/alpha").join(file))
                    .unwrap()
                    .len()
            )
            .sum::<u64>()
    );
    let selected = [
        selection(&preview, "alpha", CollisionChoice::Abort),
        selection(&preview, "gamma", CollisionChoice::Abort),
    ];
    let installed = applied(install::install(&paths, &store, &preview, &selected).unwrap());
    assert_eq!(installed.len(), 2);
    assert!(!paths.library.join("skills/beta").exists());
    let library = Library::load(&paths.library.join("library.toml"), &[]).unwrap();
    assert_eq!(library.skills.len(), 2);
    for item in installed {
        let provenance = &library.skills[&item.name];
        assert_eq!(provenance, &item.provenance);
        assert!(provenance.rev.as_ref().is_some_and(|rev| rev.len() == 40));
        assert!(provenance.installed_at.is_some());
        assert!(
            matches!(&provenance.source, SkillSource::Git { reference: Some(reference), path: Some(path), .. } if reference == "main" && path == &format!("skills/{}", item.name))
        );
        let hash = content_hash(&item.path).unwrap();
        assert_eq!(provenance.base_hash.as_deref(), Some(hash.as_str()));
        let base = paths.cache.join("base").join(&item.name).join(hash);
        assert_eq!(
            engine::snapshot(&item.path, &paths).unwrap(),
            engine::snapshot(&base, &paths).unwrap()
        );
    }
}

#[cfg(unix)]
#[test]
fn second_install_reuses_mirror_and_fetches_new_revision() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let (paths, store, git) = setup(root.path());
    let source = fixture(root.path());
    let preview = install::preview(&paths, &git, &source).unwrap();
    applied(
        install::install(
            &paths,
            &store,
            &preview,
            &[selection(&preview, "alpha", CollisionChoice::Abort)],
        )
        .unwrap(),
    );
    let mirror = fs::read_dir(paths.cache.join("git"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    let inode = fs::metadata(&mirror).unwrap().ino();
    fs::write(mirror.join("reuse-sentinel"), b"keep").unwrap();
    fs::write(
        root.path().join("repo/skills/beta/new.bin"),
        b"new revision",
    )
    .unwrap();
    git_commit(root.path());
    let second = install::preview(&paths, &git, &source).unwrap();
    let items = applied(
        install::install(
            &paths,
            &store,
            &second,
            &[selection(&second, "beta", CollisionChoice::Abort)],
        )
        .unwrap(),
    );
    assert_eq!(fs::metadata(&mirror).unwrap().ino(), inode);
    assert_eq!(fs::read(mirror.join("reuse-sentinel")).unwrap(), b"keep");
    assert_eq!(
        fs::read(items[0].path.join("new.bin")).unwrap(),
        b"new revision"
    );
}

#[cfg(unix)]
fn git_commit(root: &Path) {
    git(&root.join("repo"), &["add", "."]);
    git(&root.join("repo"), &["commit", "-m", "update"]);
}

#[test]
fn collision_is_typed_and_rename_and_replace_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let (paths, store, git) = setup(root.path());
    let source = fixture(root.path());
    let preview = install::preview(&paths, &git, &source).unwrap();
    let target = paths.library.join("skills/alpha");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("keep"), b"old bytes").unwrap();
    let result = install::install(
        &paths,
        &store,
        &preview,
        &[
            selection(&preview, "beta", CollisionChoice::Abort),
            selection(&preview, "alpha", CollisionChoice::Abort),
        ],
    )
    .unwrap();
    assert!(
        matches!(result, InstallResult::Collisions(collisions) if collisions.len() == 1 && collisions[0].name == "alpha")
    );
    assert!(!paths.library.join("skills/beta").exists());
    assert!(!paths.library.join("library.toml").exists());
    assert_eq!(fs::read(target.join("keep")).unwrap(), b"old bytes");
    let renamed = applied(
        install::install(
            &paths,
            &store,
            &preview,
            &[selection(
                &preview,
                "alpha",
                CollisionChoice::Rename("renamed".into()),
            )],
        )
        .unwrap(),
    );
    let (metadata, _) = skm_core::frontmatter::read_skill(&renamed[0].path).unwrap();
    assert_eq!(metadata.name.as_deref(), Some("renamed"));
    assert_eq!(fs::read(target.join("keep")).unwrap(), b"old bytes");
    let installed = applied(
        install::install(
            &paths,
            &store,
            &preview,
            &[selection(&preview, "alpha", CollisionChoice::Replace)],
        )
        .unwrap(),
    );
    assert_eq!(installed[0].path, target);
    assert!(!target.join("keep").exists());
    assert!(fs::read_dir(&paths.trash).unwrap().any(|op| {
        fs::read_dir(op.unwrap().path()).unwrap().any(|entry| {
            fs::read(entry.unwrap().path().join("keep")).ok().as_deref() == Some(b"old bytes")
        })
    }));
}

#[test]
fn local_import_is_byte_exact_and_preview_is_stable() {
    let root = tempfile::tempdir().unwrap();
    let (paths, store, git) = setup(root.path());
    let local = skill(root.path(), "local");
    fs::write(local.join("binary"), [0, 255, 13, 10, 128]).unwrap();
    fs::create_dir(local.join("empty")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            local.join("scripts/run.sh"),
            fs::Permissions::from_mode(0o751),
        )
        .unwrap();
        std::os::unix::fs::symlink("binary", local.join("binary-link")).unwrap();
    }
    let hash = content_hash(&local).unwrap();
    let state = engine::snapshot(&local, &paths).unwrap();
    let preview = install::preview(
        &paths,
        &git,
        &Source::Local {
            path: local.clone(),
        },
    )
    .unwrap();
    fs::write(local.join("binary"), b"changed after preview").unwrap();
    let items = applied(
        install::install(
            &paths,
            &store,
            &preview,
            &[selection(&preview, "local", CollisionChoice::Abort)],
        )
        .unwrap(),
    );
    assert_eq!(content_hash(&items[0].path).unwrap(), hash);
    assert_eq!(engine::snapshot(&items[0].path, &paths).unwrap(), state);
    assert_eq!(
        fs::read(items[0].path.join("binary")).unwrap(),
        [0, 255, 13, 10, 128]
    );
    assert_eq!(
        items[0].provenance.source,
        SkillSource::Local {
            origin: Some(local.canonicalize().unwrap())
        }
    );
}

#[test]
fn git_subpath_filters_preview_and_root_skill_is_supported() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _, runner) = setup(root.path());
    let mut source = fixture(root.path());
    if let Source::Git { path, .. } = &mut source {
        *path = Some("skills/beta".into());
    }
    let preview = install::preview(&paths, &runner, &source).unwrap();
    assert_eq!(preview.skills().len(), 1);
    assert_eq!(preview.skills()[0].name, "beta");
    fs::write(
        root.path().join("repo/SKILL.md"),
        "---\nname: root\n---\nRoot",
    )
    .unwrap();
    git(&root.path().join("repo"), &["add", "."]);
    git(&root.path().join("repo"), &["commit", "-m", "root"]);
    if let Source::Git { path, .. } = &mut source {
        *path = None;
    }
    let preview = install::preview(&paths, &runner, &source).unwrap();
    assert_eq!(preview.skills().len(), 4);
    assert!(
        preview
            .skills()
            .iter()
            .any(|skill| skill.name == "root" && skill.path.as_os_str().is_empty())
    );
}

#[test]
fn replacing_same_revision_reuses_pristine_base_and_rejects_tampering() {
    let root = tempfile::tempdir().unwrap();
    let (paths, store, git) = setup(root.path());
    let source = fixture(root.path());
    let preview = install::preview(&paths, &git, &source).unwrap();
    let selected = [selection(&preview, "alpha", CollisionChoice::Replace)];
    let first = applied(install::install(&paths, &store, &preview, &selected).unwrap());
    let base = paths
        .cache
        .join("base/alpha")
        .join(first[0].provenance.base_hash.as_ref().unwrap());
    fs::write(first[0].path.join("local-edit"), b"edited").unwrap();
    let second = applied(install::install(&paths, &store, &preview, &selected).unwrap());
    assert!(!second[0].path.join("local-edit").exists());
    assert_eq!(
        second[0].provenance.base_hash,
        first[0].provenance.base_hash
    );
    fs::write(base.join("SKILL.md"), b"tampered baseline").unwrap();
    let before = fs::read(paths.library.join("library.toml")).unwrap();
    let error = install::install(&paths, &store, &preview, &selected).unwrap_err();
    assert_eq!(error.code(), "E_CONFIG_CHANGED");
    assert_eq!(
        fs::read(paths.library.join("library.toml")).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn local_preview_rejects_links_outside_skill() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _, git) = setup(root.path());
    let local = skill(root.path(), "local");
    fs::write(root.path().join("private"), b"not skill content").unwrap();
    std::os::unix::fs::symlink("../private", local.join("escape")).unwrap();
    assert!(install::preview(&paths, &git, &Source::Local { path: local }).is_err());
    assert!(!paths.library.join("library.toml").exists());
}

#[test]
fn local_preview_rejects_source_containing_its_own_staging_directory() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _, git) = setup(root.path());
    fs::write(root.path().join("SKILL.md"), "---\nname: root\n---\nBody").unwrap();
    let error = install::preview(
        &paths,
        &git,
        &Source::Local {
            path: root.path().into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), "E_INVALID_SKILL");
}
