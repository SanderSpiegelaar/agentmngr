use std::{fs, path::Path};

use skm_core::{
    engine, foreign_skills,
    library::{ItemSelection, ItemSet, Library},
    paths::Paths,
    projects::{self, ProjectSkillState},
    registry::Registry,
    store::Store,
};

fn machine(home: &Path) -> (Paths, Store) {
    fs::create_dir_all(home).unwrap();
    let mut paths = Paths::resolve(None, Some(&home.join("manager")), Some(home)).unwrap();
    paths.ensure_layout().unwrap();
    let store = Store::open(&paths).unwrap();
    (paths, store)
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn skill(root: &Path, name: &str, content: &str) {
    let target = root.join(name);
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), content).unwrap();
}

#[test]
fn teammate_clone_reattaches_without_foreign_rows_or_repository_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first_home = root.join("alice");
    let (first_paths, first_store) = machine(&first_home);
    skill(
        &first_paths.library.join("skills"),
        "review",
        "Review code.\n",
    );
    let mut library = Library::default();
    library.sets.insert(
        "team".into(),
        ItemSet {
            items: ItemSelection {
                skills: vec!["review".into()],
                ..ItemSelection::default()
            },
            ..ItemSet::default()
        },
    );
    library.sets.insert(
        "missing".into(),
        ItemSet {
            items: ItemSelection {
                skills: vec!["absent".into()],
                ..ItemSelection::default()
            },
            ..ItemSet::default()
        },
    );
    library
        .save(&first_paths.library.join("library.toml"), &[])
        .unwrap();
    let repo = first_home.join("repo");
    fs::create_dir(&repo).unwrap();
    let project = projects::register(&first_store, &repo).unwrap();
    let registry = Registry::load().unwrap();
    projects::use_set(&first_store, &library, project.id, "team").unwrap();
    let applied = projects::apply_project(
        &first_paths,
        &first_store,
        &project,
        &library,
        &registry,
        "test",
        None,
    )
    .unwrap();
    assert!(applied.issues.is_empty());
    assert!(applied.failed.is_empty());

    let second_home = root.join("bob");
    let (paths, store) = machine(&second_home);
    copy_tree(&first_paths.library, &paths.library);
    let clone = second_home.join("repo");
    copy_tree(&repo, &clone);
    let before = engine::snapshot(&clone, &paths).unwrap();
    assert_eq!(
        foreign_skills::list_foreign(&paths, &store, &second_home, Some(&clone))
            .unwrap()
            .len(),
        1
    );
    let registration = projects::register_with_report(&store, &clone).unwrap();
    assert_eq!(
        registration.reattachment.skills["review"],
        ProjectSkillState::Attached
    );
    assert_eq!(registration.reattachment.suggested_sets, ["team"]);
    assert!(
        foreign_skills::list_foreign(&paths, &store, &second_home, Some(&clone))
            .unwrap()
            .is_empty()
    );
    assert_eq!(engine::snapshot(&clone, &paths).unwrap(), before);
    assert!(
        projects::used_sets(&store, registration.project.id)
            .unwrap()
            .is_empty()
    );
    let target = clone.join(".agents/skills/review");
    let observation = engine::observe(&paths, &store, &[target])
        .unwrap()
        .remove(0);
    let owner = observation.ledger.unwrap();
    assert_eq!(owner.scope_id, registration.project.id);
    assert_eq!(owner.how, "copy");
    assert_eq!(observation.state.hash.as_ref(), Some(&owner.written_hash));
    let ops = engine::list_ops(&store).unwrap().len();
    assert_eq!(
        projects::register(&store, &clone).unwrap(),
        registration.project
    );
    assert_eq!(engine::list_ops(&store).unwrap().len(), ops);

    fs::write(
        clone.join(".agents/skills/review/SKILL.md"),
        "Edited by Bob.",
    )
    .unwrap();
    let edited = projects::register_with_report(&store, &clone).unwrap();
    assert_eq!(
        edited.reattachment.skills["review"],
        ProjectSkillState::Edited
    );
    let edited_clone = second_home.join("edited-clone");
    copy_tree(&clone, &edited_clone);
    let edited = projects::register_with_report(&store, &edited_clone).unwrap();
    assert_eq!(
        edited.reattachment.skills["review"],
        ProjectSkillState::Edited
    );
    assert!(
        engine::observe(
            &paths,
            &store,
            &[edited_clone.join(".agents/skills/review")]
        )
        .unwrap()[0]
            .ledger
            .is_none()
    );
}

#[test]
fn unknown_and_hash_excluded_differences_are_not_claimed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (paths, store) = machine(&root);
    let repo = root.join("repo");
    let skills = repo.join(".agents/skills");
    skill(&paths.library.join("skills"), "review", "Review.");
    skill(&skills, "review", "Review.");
    skill(&skills, "unknown", "Unknown.");
    fs::create_dir_all(skills.join("review/node_modules")).unwrap();
    fs::write(skills.join("review/node_modules/extra"), "not in library").unwrap();
    let before = engine::snapshot(&repo, &paths).unwrap();
    let result = projects::register_with_report(&store, &repo).unwrap();
    assert_eq!(
        result.reattachment.skills["review"],
        ProjectSkillState::Edited
    );
    assert_eq!(
        result.reattachment.skills["unknown"],
        ProjectSkillState::Foreign
    );
    assert_eq!(engine::snapshot(&repo, &paths).unwrap(), before);
    assert_eq!(
        foreign_skills::list_foreign(&paths, &store, temp.path(), Some(&repo))
            .unwrap()
            .len(),
        2
    );
    let plan = projects::plan_skills(
        &paths,
        &store,
        &result.project,
        &ItemSelection {
            skills: vec!["review".into()],
            ..ItemSelection::default()
        },
        &["review".into()].into(),
    )
    .unwrap();
    assert!(plan.actions.is_empty());
    assert!(!plan.issues.is_empty());
    assert!(
        projects::direct_items(&store, result.project.id)
            .unwrap()
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn symlinked_skills_root_never_grants_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (paths, store) = machine(&root);
    skill(&paths.library.join("skills"), "review", "Review.");
    let external = root.join("external");
    copy_tree(&paths.library.join("skills"), &external);
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".agents")).unwrap();
    std::os::unix::fs::symlink(&external, repo.join(".agents/skills")).unwrap();
    let report = projects::register_with_report(&store, &repo).unwrap();
    assert_ne!(
        report.reattachment.skills["review"],
        ProjectSkillState::Attached
    );
    assert!(
        engine::observe(&paths, &store, &[repo.join(".agents/skills/review")]).unwrap()[0]
            .ledger
            .is_none()
    );
}

#[test]
fn malformed_locks_refuse_before_registration_or_claims() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (paths, store) = machine(&root);
    skill(&paths.library.join("skills"), "review", "Review.");
    let repo = root.join("repo");
    skill(&repo.join(".agents/skills"), "review", "Review.");
    fs::write(repo.join("skills-lock.json"), "{broken").unwrap();
    assert!(projects::register(&store, &repo).is_err());
    assert!(projects::list(&store).unwrap().is_empty());
    assert!(
        engine::observe(&paths, &store, &[repo.join(".agents/skills/review")]).unwrap()[0]
            .ledger
            .is_none()
    );
}
