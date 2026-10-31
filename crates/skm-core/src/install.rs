use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use crate::{
    Error,
    content_hash::{content_hash, is_ignored_dir},
    engine::{self, Action, ApplyResult, Observation, Ownership, Plan, PlannedAction},
    frontmatter::{self, Warning},
    git::GitRunner,
    global_skills::validate_name,
    library::{Library, SkillProvenance, SkillSource},
    paths::Paths,
    registry::Registry,
    source::Source,
    store::Store,
    target::{ItemKind, ItemRef, TargetClass},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSkill {
    pub name: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub files: Vec<PathBuf>,
    pub size: u64,
    pub has_scripts: bool,
    pub has_hooks: bool,
    pub warnings: Vec<Warning>,
}

#[derive(Debug)]
pub struct Preview {
    skills: Vec<DiscoveredSkill>,
    candidates: Vec<Candidate>,
    staging: tempfile::TempDir,
}

impl Preview {
    pub fn skills(&self) -> &[DiscoveredSkill] {
        &self.skills
    }
}

#[derive(Debug)]
struct Candidate {
    directory: PathBuf,
    hash: String,
    provenance: SkillProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CollisionChoice {
    #[default]
    Abort,
    Rename(String),
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub skill: usize,
    pub collision: CollisionChoice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameCollision {
    pub skill: usize,
    pub name: String,
    pub target: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkill {
    pub name: String,
    pub path: PathBuf,
    pub provenance: SkillProvenance,
}

#[derive(Debug)]
pub enum InstallResult {
    Collisions(Vec<NameCollision>),
    Applied {
        items: Vec<InstalledSkill>,
        result: ApplyResult,
    },
}

pub struct InstallPlan {
    pub plan: Plan,
    pub items: Vec<InstalledSkill>,
    _renamed: tempfile::TempDir,
}

impl InstallPlan {
    pub fn apply(mut self, paths: &Paths, store: &Store) -> Result<InstallResult, Error> {
        let scope_id = store.write(|connection| {
            connection.execute("INSERT INTO scopes(kind,name,mode,added_at) SELECT 'global','Global','link',unixepoch() WHERE NOT EXISTS (SELECT 1 FROM scopes WHERE kind='global')", [])?;
            Ok(connection.query_row("SELECT id FROM scopes WHERE kind='global' ORDER BY id LIMIT 1", [], |row| row.get(0))?)
        })?;
        let preview_scope = self.plan.scope_id;
        self.plan.scope_id = scope_id;
        for action in &mut self.plan.actions {
            if let Some(owner) = &mut action.owner {
                owner.scope_id = scope_id;
            }
            for required in &mut action.requires {
                if let Some(owner) = &mut required.ledger
                    && owner.scope_id == preview_scope
                {
                    owner.scope_id = scope_id;
                }
            }
        }
        let result = engine::apply(paths, store, &self.plan, "install", None)?;
        if !result
            .done
            .iter()
            .any(|action| action.target() == paths.library.join("library.toml"))
        {
            self.items.clear();
        }
        Ok(InstallResult::Applied {
            items: self.items,
            result,
        })
    }
}

/// Performs Git and filesystem work synchronously. Retain the preview until install completes.
pub fn preview(paths: &Paths, git: &GitRunner, source: &Source) -> Result<Preview, Error> {
    fs::create_dir_all(&paths.cache)?;
    let staging = tempfile::Builder::new()
        .prefix("install-")
        .tempdir_in(&paths.cache)?;
    let mut result = Preview {
        skills: Vec::new(),
        candidates: Vec::new(),
        staging,
    };
    if let Source::Local { path } = source {
        let root = fs::canonicalize(path)?;
        if !root.is_dir() {
            return Err(Error::InvalidSkill(
                "local source is not a directory".into(),
            ));
        }
        if fs::canonicalize(result.staging.path())?.starts_with(&root) {
            return Err(Error::InvalidSkill(
                "local source contains the install staging directory".into(),
            ));
        }
        let mut skills = Vec::new();
        discover_local(&root, &root, &mut skills)?;
        skills.sort();
        for path in skills {
            let source = root.join(&path);
            let directory = result.staging.path().join(result.skills.len().to_string());
            let before = engine::snapshot(&source, paths)?;
            engine::copy_tree(&source, &directory)?;
            if engine::snapshot(&source, paths)? != before
                || engine::snapshot(&directory, paths)? != before
            {
                return Err(Error::ConfigChanged(
                    "local source changed during preview".into(),
                ));
            }
            let fallback = source.file_name().unwrap_or_default().to_string_lossy();
            add_candidate(
                &mut result,
                paths,
                directory,
                path,
                &fallback,
                SkillProvenance {
                    source: SkillSource::Local {
                        origin: Some(source.clone()),
                    },
                    rev: None,
                    base_hash: None,
                    installed_at: None,
                },
            )?;
        }
    } else {
        let (url, reference, subpath, skill) = match source {
            Source::Git {
                url,
                reference,
                path,
            } => (url.clone(), reference.clone(), path.clone(), None),
            Source::GitHub { owner, repo, skill } => (
                format!(
                    "https://github.com/{owner}/{}.git",
                    repo.trim_end_matches(".git")
                ),
                None,
                None,
                skill.clone(),
            ),
            Source::SkillsSh { owner, repo, skill } => (
                format!(
                    "https://github.com/{owner}/{}.git",
                    repo.trim_end_matches(".git")
                ),
                None,
                None,
                Some(skill.clone()),
            ),
            Source::Local { .. } => unreachable!(),
        };
        let mirrors = paths.cache.join("git");
        fs::create_dir_all(&mirrors)?;
        let key = format!("{:x}", Sha256::digest(url.as_bytes()));
        let mut lock = fd_lock::RwLock::new(
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(mirrors.join(format!("{key}.lock")))?,
        );
        let _guard = lock.try_write().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Error::Locked("another preview is using this Git mirror".into())
            } else {
                error.into()
            }
        })?;
        let mirror = mirrors.join(key);
        let fresh = !mirror.exists();
        if fresh {
            let temporary = tempfile::tempdir_in(&mirrors)?;
            git.clone_bare(&url, &temporary.path().join("mirror"))?;
            fs::rename(temporary.path().join("mirror"), &mirror)?;
        }
        let repository = git.bare(&mirror)?;
        // `clone --branch` rejects commit SHAs; fetch accepts branches, tags and SHAs.
        let revision = if fresh && reference.is_none() {
            repository.rev_parse("HEAD")?
        } else {
            repository.fetch(reference.as_deref())?;
            repository.rev_parse("FETCH_HEAD")?
        };
        for (index, file) in repository.ls_tree(&revision)?.into_iter().enumerate() {
            if file.file_name().is_none_or(|name| name != "SKILL.md") {
                continue;
            }
            let path = file.parent().unwrap_or(Path::new("")).to_path_buf();
            if path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err(Error::InvalidSkill("unsafe repository skill path".into()));
            }
            if subpath
                .as_ref()
                .is_some_and(|prefix| !path.starts_with(prefix))
            {
                continue;
            }
            let fallback = path
                .file_name()
                .and_then(|name| name.to_str())
                .or_else(|| {
                    url.rsplit('/')
                        .next()
                        .map(|name| name.trim_end_matches(".git"))
                })
                .unwrap_or("skill");
            let unpacked = result.staging.path().join(format!("archive-{index}"));
            fs::create_dir(&unpacked)?;
            let archive = repository.archive(
                &revision,
                if path.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    &path
                },
            )?;
            let mut tar = tempfile::NamedTempFile::new_in(result.staging.path())?;
            tar.write_all(&archive)?;
            let output = Command::new("tar")
                .arg("-xf")
                .arg(tar.path())
                .arg("-C")
                .arg(&unpacked)
                .output()?;
            if !output.status.success() {
                return Err(Error::InvalidSkill(format!(
                    "Git archive extraction failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            let directory = unpacked.join(&path);
            add_candidate(
                &mut result,
                paths,
                directory,
                path.clone(),
                fallback,
                SkillProvenance {
                    source: SkillSource::Git {
                        url: url.clone(),
                        reference: reference.clone(),
                        path: Some(path.to_string_lossy().into_owned()),
                    },
                    rev: Some(revision.clone()),
                    base_hash: None,
                    installed_at: None,
                },
            )?;
            if skill
                .as_ref()
                .is_some_and(|name| result.skills.last().is_some_and(|item| &item.name != name))
            {
                result.skills.pop();
                result.candidates.pop();
            }
        }
    }
    if result.skills.is_empty() {
        return Err(Error::InvalidSkill(
            "source contains no matching SKILL.md".into(),
        ));
    }
    Ok(result)
}

fn discover_local(root: &Path, directory: &Path, skills: &mut Vec<PathBuf>) -> Result<(), Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() && !is_ignored_dir(&entry.file_name()) {
            discover_local(root, &entry.path(), skills)?;
        } else if kind.is_file() && entry.file_name() == "SKILL.md" {
            skills.push(directory.strip_prefix(root).unwrap().to_path_buf());
        }
    }
    Ok(())
}

fn add_candidate(
    result: &mut Preview,
    paths: &Paths,
    directory: PathBuf,
    path: PathBuf,
    fallback: &str,
    provenance: SkillProvenance,
) -> Result<(), Error> {
    let mut files = Vec::new();
    let mut size = 0;
    inspect_tree(&directory, &directory, &mut files, &mut size)?;
    if !fs::symlink_metadata(directory.join("SKILL.md"))?.is_file() {
        return Err(Error::InvalidSkill(
            "SKILL.md must be a regular file".into(),
        ));
    }
    let (metadata, warnings) = frontmatter::read_frontmatter(
        &mut BufReader::new(File::open(directory.join("SKILL.md"))?),
        fallback,
    )?;
    let name = metadata.name.unwrap_or_else(|| fallback.into());
    validate_name(&name)?;
    files.sort();
    let has_scripts = files.iter().any(|path| {
        path.components().any(|part| part.as_os_str() == "scripts")
            || matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("sh" | "py" | "js" | "ts" | "bash" | "rb" | "ps1")
            )
    });
    let has_hooks = files
        .iter()
        .any(|path| path.components().any(|part| part.as_os_str() == "hooks"));
    let hash = engine::snapshot(&directory, paths)?.hash.unwrap();
    result.skills.push(DiscoveredSkill {
        name,
        path,
        description: metadata.description,
        files,
        size,
        has_scripts,
        has_hooks,
        warnings,
    });
    result.candidates.push(Candidate {
        directory,
        hash,
        provenance,
    });
    Ok(())
}

fn inspect_tree(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
    size: &mut u64,
) -> Result<(), Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            inspect_tree(root, &path, files, size)?;
        } else if kind.is_file() {
            *size += entry.metadata()?.len();
            files.push(path.strip_prefix(root).unwrap().to_path_buf());
        } else if kind.is_symlink() {
            if !fs::canonicalize(&path)?.starts_with(fs::canonicalize(root)?) {
                return Err(Error::InvalidSkill(
                    "skill symlink escapes its directory".into(),
                ));
            }
            files.push(path.strip_prefix(root).unwrap().to_path_buf());
        } else {
            return Err(Error::InvalidSkill(
                "unsupported skill filesystem entry".into(),
            ));
        }
    }
    Ok(())
}

/// Collisions return before any library writes. Apply issues and failures remain visible to callers.
pub fn install(
    paths: &Paths,
    store: &Store,
    preview: &Preview,
    selection: &[Selection],
) -> Result<InstallResult, Error> {
    match plan_install(paths, store, preview, selection)? {
        Ok(plan) => plan.apply(paths, store),
        Err(collisions) => Ok(InstallResult::Collisions(collisions)),
    }
}

pub fn plan_install(
    paths: &Paths,
    store: &Store,
    preview: &Preview,
    selection: &[Selection],
) -> Result<Result<InstallPlan, Vec<NameCollision>>, Error> {
    if selection.is_empty() {
        return Err(Error::InvalidSkill("select at least one skill".into()));
    }
    let registry = Registry::load()?;
    let known_agents: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    let manifest = paths.library.join("library.toml");
    let manifest_before = engine::observe(paths, store, std::slice::from_ref(&manifest))?.remove(0);
    if !manifest_before.parents_safe
        || !matches!(
            manifest_before.state.class,
            TargetClass::Absent | TargetClass::RealFile
        )
    {
        return Err(Error::ForeignTarget(
            "library.toml is not a safe regular file".into(),
        ));
    }
    let existing = match fs::read_to_string(&manifest) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut library = if existing.is_empty() {
        Library::default()
    } else {
        Library::parse(&existing, &known_agents)?
    };
    let mut selected = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut collisions = Vec::new();
    let mut targets = Vec::new();
    for choice in selection {
        let skill = preview
            .skills
            .get(choice.skill)
            .ok_or_else(|| Error::InvalidSkill("selection is outside preview".into()))?;
        if !selected.insert(choice.skill) {
            return Err(Error::NameCollision("skill selected twice".into()));
        }
        let name = match &choice.collision {
            CollisionChoice::Rename(name) => name.clone(),
            _ => skill.name.clone(),
        };
        validate_name(&name)?;
        let target = paths.library.join("skills").join(&name);
        let before = engine::observe(paths, store, std::slice::from_ref(&target))?.remove(0);
        let duplicate = !names.insert(name.clone());
        let exists =
            before.state.class != TargetClass::Absent || library.skills.contains_key(&name);
        if duplicate || (exists && choice.collision != CollisionChoice::Replace) {
            collisions.push(NameCollision {
                skill: choice.skill,
                name: name.clone(),
                target: target.clone(),
            });
        }
        targets.push((choice, name, target, before));
    }
    if !collisions.is_empty() {
        return Ok(Err(collisions));
    }
    let scope_id = store.read(|connection| {
        Ok(connection.query_row(
            "SELECT COALESCE((SELECT id FROM scopes WHERE kind='global' ORDER BY id LIMIT 1),0)",
            [],
            |row| row.get(0),
        )?)
    })?;
    let timestamp: String = store.read(|connection| {
        Ok(
            connection.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |row| {
                row.get(0)
            })?,
        )
    })?;
    let renamed = tempfile::tempdir_in(&paths.cache)?;
    let mut plan = Plan {
        scope_id,
        actions: Vec::new(),
        issues: Vec::new(),
        summary: "Install skills".into(),
    };
    let mut prerequisites = vec![manifest_before.clone()];
    let mut items = Vec::new();
    for (choice, name, target, before) in targets {
        let candidate = &preview.candidates[choice.skill];
        if engine::snapshot(&candidate.directory, paths)?.hash.as_ref() != Some(&candidate.hash) {
            return Err(Error::ConfigChanged("preview content changed".into()));
        }
        let mut directory = candidate.directory.clone();
        if name != preview.skills[choice.skill].name {
            directory = renamed.path().join(&name);
            engine::copy_tree(&candidate.directory, &directory)?;
            rename_frontmatter(&directory.join("SKILL.md"), &name)?;
        }
        let base_hash = content_hash(&directory)?;
        let mut provenance = candidate.provenance.clone();
        provenance.base_hash = Some(base_hash.clone());
        provenance.installed_at = Some(
            timestamp
                .parse()
                .map_err(|error| Error::ConfigParse(format!("invalid timestamp: {error}")))?,
        );
        let base = paths.cache.join("base").join(&name).join(&base_hash);
        let base_before = engine::observe(paths, store, std::slice::from_ref(&base))?.remove(0);
        let base_after = if base_before.state.class == TargetClass::Absent {
            plan_copy(
                paths,
                &mut plan,
                &directory,
                base_before,
                &name,
                vec![manifest_before.clone()],
            )?
        } else {
            if base_before.state != engine::snapshot(&directory, paths)? {
                return Err(Error::ConfigChanged("base snapshot is not pristine".into()));
            }
            base_before
        };
        prerequisites.push(base_after.clone());
        let after = plan_copy(
            paths,
            &mut plan,
            &directory,
            before,
            &name,
            vec![manifest_before.clone(), base_after],
        )?;
        prerequisites.push(after);
        library.skills.insert(name.clone(), provenance.clone());
        items.push(InstalledSkill {
            name,
            path: target,
            provenance,
        });
    }
    if !items.is_empty() {
        let mut write = PlannedAction {
            action: Action::WriteFile {
                target: manifest.clone(),
                bytes: library.render(&existing, &known_agents)?.into_bytes(),
            },
            before: manifest_before,
            owner: Some(Ownership {
                scope_id,
                agent_id: None,
                item: ItemRef {
                    kind: ItemKind::Skill,
                    name: "library.toml".into(),
                },
                how: "file".into(),
                written_hash: String::new(),
            }),
            requires: prerequisites,
            approve_modified: false,
            adoption: None,
        };
        if write.before.state.class != TargetClass::Absent {
            write.authorize_adoption();
        }
        plan.actions.push(write);
    }
    Ok(Ok(InstallPlan {
        plan,
        items,
        _renamed: renamed,
    }))
}

fn plan_copy(
    paths: &Paths,
    plan: &mut Plan,
    source: &Path,
    before: Observation,
    name: &str,
    requires: Vec<Observation>,
) -> Result<Observation, Error> {
    let state = engine::snapshot(source, paths)?;
    let owner = Ownership {
        scope_id: plan.scope_id,
        agent_id: None,
        item: ItemRef {
            kind: ItemKind::Skill,
            name: name.into(),
        },
        how: "copy".into(),
        written_hash: state.hash.clone().unwrap(),
    };
    let after = Observation {
        target: before.target.clone(),
        state: state.clone(),
        ledger: Some(owner.clone()),
        parents_safe: true,
    };
    let mut action = PlannedAction {
        action: Action::CopyTree {
            target: before.target.clone(),
            from: source.to_path_buf(),
            hash: state.hash.unwrap(),
            computed_hash: Some(content_hash(source)?),
        },
        before,
        owner: Some(owner),
        requires,
        approve_modified: true,
        adoption: None,
    };
    if action.before.state.class != TargetClass::Absent {
        action.authorize_adoption();
    }
    plan.actions.push(action);
    Ok(after)
}

fn rename_frontmatter(path: &Path, name: &str) -> Result<(), Error> {
    let bytes = fs::read(path)?;
    let first = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    if first.strip_suffix(b"\r").unwrap_or(first) != b"---" {
        let mut mapping = serde_yaml_ng::Mapping::new();
        mapping.insert("name".into(), name.into());
        let yaml = serde_yaml_ng::to_string(&mapping)
            .map_err(|error| Error::InvalidSkill(error.to_string()))?;
        let mut renamed = format!("---\n{yaml}---\n").into_bytes();
        renamed.extend_from_slice(&bytes);
        fs::write(path, renamed)?;
        return Ok(());
    }
    let start = first.len() + 1;
    if start > bytes.len() {
        return Err(Error::InvalidSkill(
            "cannot rename unclosed frontmatter".into(),
        ));
    }
    let mut end = start;
    for line in bytes[start..].split_inclusive(|byte| *byte == b'\n') {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        if trimmed.strip_suffix(b"\r").unwrap_or(trimmed) == b"---" {
            let mut mapping: serde_yaml_ng::Mapping = serde_yaml_ng::from_slice(&bytes[start..end])
                .map_err(|error| Error::InvalidSkill(error.to_string()))?;
            mapping.insert("name".into(), name.into());
            let yaml = serde_yaml_ng::to_string(&mapping)
                .map_err(|error| Error::InvalidSkill(error.to_string()))?;
            let mut renamed = format!("---\n{yaml}---\n").into_bytes();
            renamed.extend_from_slice(&bytes[end + line.len()..]);
            fs::write(path, renamed)?;
            return Ok(());
        }
        end += line.len();
    }
    Err(Error::InvalidSkill(
        "cannot rename unclosed frontmatter".into(),
    ))
}
