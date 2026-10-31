use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: Theme,
    pub sync_remote: Option<String>,
    pub update_interval_hours: u64,
    pub agents: BTreeMap<String, AgentPathOverrides>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            sync_remote: None,
            update_interval_hours: 6,
            agents: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentPathOverrides {
    pub detect: Option<Vec<String>>,
    pub mcp: FilePathOverrides,
    pub hooks: FilePathOverrides,
    pub subagents: DirectoryPathOverrides,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilePathOverrides {
    pub user: Option<FilePathOverride>,
    pub project: Option<FilePathOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePathOverride {
    pub file: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectoryPathOverrides {
    pub user: Option<PathBuf>,
    pub project: Option<PathBuf>,
}

impl Config {
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        toml::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("toml.skm-tmp");
        let mut file = std::fs::File::create(&temporary)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_empty_config_use_defaults() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        std::fs::write(&path, "").unwrap();
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        assert_eq!(Config::default().theme, Theme::System);
        assert_eq!(Config::default().update_interval_hours, 6);
    }

    #[test]
    fn preferences_and_agent_path_overrides_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
theme = "dark"
sync_remote = "git@example.com:skills.git"
update_interval_hours = 12
[agents.claude-code]
detect = ["/custom/claude", "bin:claude"]
[agents.claude-code.mcp.user]
file = "/custom/claude.json"
[agents.claude-code.hooks.project]
file = ".custom/settings.json"
[agents.claude-code.subagents]
user = "/custom/agents"
project = ".custom/agents"
"#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.theme, Theme::Dark);
        assert_eq!(config.update_interval_hours, 12);
        let agent = &config.agents["claude-code"];
        assert_eq!(
            agent.mcp.user.as_ref().unwrap().file,
            Path::new("/custom/claude.json")
        );
        assert_eq!(
            agent.hooks.project.as_ref().unwrap().file,
            Path::new(".custom/settings.json")
        );
        assert_eq!(
            agent.subagents.user.as_deref(),
            Some(Path::new("/custom/agents"))
        );
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        assert!(!path.with_extension("toml.skm-tmp").exists());
    }

    #[test]
    fn malformed_config_and_io_failures_are_errors() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(&path, "theme = [").unwrap();
        assert_eq!(
            Config::load(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(Config::load(root.path()).is_err());
        assert!(
            Config::default()
                .save(&root.path().join("missing/config.toml"))
                .is_err()
        );
    }
}
