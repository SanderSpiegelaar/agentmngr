use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::{Error, config::Config};

pub const EMBEDDED_REGISTRY: &str = include_str!("../registry/agents.toml");

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub agents: BTreeMap<String, Agent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub name: String,
    pub detect: Vec<String>,
    pub config_dir_env: Option<String>,
    pub config_dir: Option<PathBuf>,
    pub skills: Skills,
    pub mcp: Option<FileTargets>,
    pub hooks: Option<FileTargets>,
    pub subagents: Option<DirectoryTargets>,
    pub plugins: Option<Plugins>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skills {
    pub reads_universal: bool,
    pub user: PathBuf,
    pub project: PathBuf,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTargets {
    pub dialect: String,
    pub requires: Option<String>,
    pub user: Option<FileTarget>,
    pub project: Option<FileTarget>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTarget {
    pub file: PathBuf,
    pub config_dir_file: Option<PathBuf>,
    pub format: Option<String>,
    pub key: Option<String>,
    pub write_via: Option<String>,
    #[serde(default)]
    pub also_read_by: Vec<String>,
    pub trust: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryTargets {
    pub format: String,
    pub user: Option<PathBuf>,
    pub project: Option<PathBuf>,
    #[serde(default)]
    pub also_read_by: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugins {
    pub driver: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub id: String,
    pub name: String,
    pub detected: bool,
    pub enabled: bool,
}

impl Registry {
    pub fn load() -> Result<Self, Error> {
        Self::from_toml(EMBEDDED_REGISTRY)
    }

    pub fn from_toml(text: &str) -> Result<Self, Error> {
        // TOML tables reject duplicate agent IDs before deserialization succeeds.
        let registry: Self = toml::from_str(text)
            .map_err(|error| Error::ConfigParse(format!("agent registry: {error}")))?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn validate(&self) -> Result<(), Error> {
        for (id, agent) in &self.agents {
            let mut projects = vec![&agent.skills.project];
            let mut readers = Vec::new();
            for targets in [&agent.mcp, &agent.hooks].into_iter().flatten() {
                if let Some(target) = &targets.project {
                    projects.push(&target.file);
                }
                for target in [&targets.user, &targets.project].into_iter().flatten() {
                    readers.extend(&target.also_read_by);
                }
            }
            if let Some(targets) = &agent.subagents {
                projects.extend(targets.project.iter());
                readers.extend(&targets.also_read_by);
            }
            for path in projects {
                let text = path.to_string_lossy();
                if path.as_os_str().is_empty()
                    || path.is_absolute()
                    || text.starts_with('~')
                    || text.contains('\\')
                    || text.contains(':')
                    || path
                        .components()
                        .any(|part| matches!(part, Component::ParentDir))
                {
                    return Err(Error::ConfigParse(format!(
                        "agent {id}: project path must stay relative: {}",
                        path.display()
                    )));
                }
            }
            for reader in readers {
                if !self.agents.contains_key(reader) {
                    return Err(Error::ConfigParse(format!(
                        "agent {id}: unknown also_read_by id {reader}"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        home: &Path,
        env: &BTreeMap<String, OsString>,
        config: &Config,
    ) -> Result<Self, Error> {
        let mut resolved = self.clone();
        for (id, agent) in &mut resolved.agents {
            let env_root = agent
                .config_dir_env
                .as_ref()
                .and_then(|key| env.get(key))
                .filter(|value| !value.is_empty())
                .map(PathBuf::from);
            let root = agent.config_dir.as_ref();
            let expand = |path: &Path| {
                if let Some((root, replacement)) = root.zip(env_root.as_ref())
                    && let Ok(suffix) = path.strip_prefix(root)
                {
                    return replacement.join(suffix);
                }
                expand_home(path, home)
            };
            for entry in &mut agent.detect {
                if !entry.starts_with("bin:") {
                    *entry = expand(Path::new(entry)).to_string_lossy().into_owned();
                }
            }
            agent.skills.user = expand(&agent.skills.user);
            for targets in [&mut agent.mcp, &mut agent.hooks].into_iter().flatten() {
                if let Some(target) = &mut targets.user {
                    target.file = match (&env_root, &target.config_dir_file) {
                        (Some(root), Some(file)) => root.join(file),
                        _ => expand(&target.file),
                    };
                }
            }
            if let Some(target) = agent
                .subagents
                .as_mut()
                .and_then(|target| target.user.as_mut())
            {
                *target = expand(target);
            }
            if let Some(overrides) = config.agents.get(id) {
                if let Some(detect) = &overrides.detect {
                    agent.detect = detect
                        .iter()
                        .map(|entry| {
                            if entry.starts_with("bin:") {
                                entry.clone()
                            } else {
                                expand_home(Path::new(entry), home)
                                    .to_string_lossy()
                                    .into_owned()
                            }
                        })
                        .collect();
                }
                for (targets, paths) in [
                    (&mut agent.mcp, &overrides.mcp),
                    (&mut agent.hooks, &overrides.hooks),
                ] {
                    if let Some(targets) = targets {
                        for (target, path, user) in [
                            (&mut targets.user, &paths.user, true),
                            (&mut targets.project, &paths.project, false),
                        ] {
                            if let Some(path) = path {
                                let target = target.as_mut().ok_or_else(|| {
                                    Error::ConfigParse(format!(
                                        "agent {id}: override has no registry target"
                                    ))
                                })?;
                                target.file = if user {
                                    expand_home(&path.file, home)
                                } else {
                                    path.file.clone()
                                };
                            }
                        }
                    }
                }
                if let Some(targets) = &mut agent.subagents {
                    if let Some(path) = &overrides.subagents.user {
                        targets.user = Some(expand_home(path, home));
                    }
                    if let Some(path) = &overrides.subagents.project {
                        targets.project = Some(path.clone());
                    }
                }
            }
            agent.config_dir = agent.config_dir.as_ref().map(|path| expand(path));
        }
        resolved.validate()?;
        Ok(resolved)
    }

    pub fn detect(
        &self,
        path: &OsStr,
        enabled: &BTreeSet<String>,
    ) -> Result<Vec<AgentStatus>, Error> {
        self.validate()?;
        Ok(self
            .agents
            .iter()
            .map(|(id, agent)| AgentStatus {
                id: id.clone(),
                name: agent.name.clone(),
                detected: agent.detect.iter().any(|entry| {
                    if let Some(bin) = entry.strip_prefix("bin:") {
                        !bin.is_empty()
                            && !bin.contains(['/', '\\'])
                            && std::env::split_paths(path).any(|dir| executable(&dir.join(bin)))
                    } else {
                        Path::new(entry).exists()
                    }
                }),
                enabled: enabled.contains(id),
            })
            .collect())
    }
}

fn expand_home(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        home.to_owned()
    } else if let Ok(suffix) = path.strip_prefix("~/") {
        home.join(suffix)
    } else {
        path.to_owned()
    }
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
