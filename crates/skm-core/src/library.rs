use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, TableLike};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub schema: u32,
    #[serde(default)]
    pub global: ItemSelection,
    #[serde(default)]
    pub sets: BTreeMap<String, ItemSet>,
    #[serde(default)]
    pub skills: BTreeMap<String, SkillProvenance>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            schema: 1,
            global: ItemSelection::default(),
            sets: BTreeMap::new(),
            skills: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSelection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hooks: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subagents: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub items: ItemSelection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProvenance {
    pub source: SkillSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<toml::value::Datetime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SkillSource {
    Git {
        url: String,
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Local {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<PathBuf>,
    },
}

impl Library {
    pub fn parse(text: &str, known_agents: &[&str]) -> Result<Self, Error> {
        let library: Self = toml::from_str(text).map_err(config_error)?;
        library.validate(known_agents)?;
        Ok(library)
    }

    pub fn load(path: &Path, known_agents: &[&str]) -> Result<Self, Error> {
        Self::parse(&std::fs::read_to_string(path)?, known_agents)
    }

    pub fn validate(&self, known_agents: &[&str]) -> Result<(), Error> {
        if self.schema != 1 {
            return Err(config_error(format!(
                "unsupported library schema {}",
                self.schema
            )));
        }
        for selection in std::iter::once(&self.global).chain(self.sets.values().map(|s| &s.items)) {
            for entries in [
                &selection.mcp,
                &selection.hooks,
                &selection.subagents,
                &selection.plugins,
            ] {
                for (name, agents) in entries {
                    for agent in agents {
                        if agent != "*" && !known_agents.contains(&agent.as_str()) {
                            return Err(config_error(format!(
                                "unknown agent {agent:?} for {name:?}"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Only changed model fields are rewritten; unchanged formatting and unknown fields stay put.
    /// Generated arrays are sorted, while unchanged arrays retain their original order.
    pub fn render(&self, existing: &str, known_agents: &[&str]) -> Result<String, Error> {
        self.validate(known_agents)?;
        let mut document = existing.parse::<DocumentMut>().map_err(config_error)?;
        let previous = if existing.trim().is_empty() {
            DocumentMut::new()
        } else {
            let library: Self = toml::from_str(existing).map_err(config_error)?;
            library.document()?
        };
        let desired = self.document()?;
        sync_table(
            document.as_table_mut(),
            previous.as_table(),
            desired.as_table(),
        );
        Ok(document.to_string())
    }

    pub fn save(&self, path: &Path, known_agents: &[&str]) -> Result<(), Error> {
        let existing = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let text = self.render(&existing, known_agents)?;
        let temporary = path.with_extension("toml.skm-tmp");
        let mut file = std::fs::File::create(&temporary)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }

    fn document(&self) -> Result<DocumentMut, Error> {
        let mut sorted = self.clone();
        for selection in std::iter::once(&mut sorted.global)
            .chain(sorted.sets.values_mut().map(|s| &mut s.items))
        {
            selection.skills.sort();
            for entries in [
                &mut selection.mcp,
                &mut selection.hooks,
                &mut selection.subagents,
                &mut selection.plugins,
            ] {
                for agents in entries.values_mut() {
                    agents.sort();
                }
            }
        }
        let mut document = toml_edit::ser::to_document(&sorted).map_err(config_error)?;
        for key in ["global", "sets", "skills"] {
            let table = document[key]
                .clone()
                .into_table()
                .map_err(|_| config_error("expected table"))?;
            document[key] = Item::Table(table);
        }
        for key in ["sets", "skills"] {
            let group = document[key].as_table_mut().unwrap();
            group.set_implicit(true);
            for (_, item) in group.iter_mut() {
                let table = item
                    .clone()
                    .into_table()
                    .map_err(|_| config_error("expected item table"))?;
                *item = Item::Table(table);
            }
        }
        Ok(document)
    }
}

fn sync_table(actual: &mut dyn TableLike, previous: &dyn TableLike, desired: &dyn TableLike) {
    for (key, _) in previous.iter() {
        if !desired.contains_key(key) {
            actual.remove(key);
        }
    }
    for (key, value) in desired.iter() {
        let old = previous.get(key);
        // Compare normalized model values, not the original spelling or ordering.
        if old.is_some_and(|old| same_item(old, value)) {
            continue;
        }
        if let (Some(actual), Some(old), Some(new)) = (
            actual.get_mut(key).and_then(Item::as_table_like_mut),
            old.and_then(Item::as_table_like),
            value.as_table_like(),
        ) {
            sync_table(actual, old, new);
        } else {
            let mut replacement = value.clone();
            if let (Some(old), Some(new)) = (
                actual.get(key).and_then(Item::as_value),
                replacement.as_value_mut(),
            ) {
                *new.decor_mut() = old.decor().clone();
            }
            actual.insert(key, replacement);
        }
    }
}

fn same_item(left: &Item, right: &Item) -> bool {
    match (left.as_table_like(), right.as_table_like()) {
        (Some(left), Some(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, value)| right.get(key).is_some_and(|other| same_item(value, other)))
        }
        _ => left.to_string() == right.to_string(),
    }
}

fn config_error(error: impl std::fmt::Display) -> Error {
    Error::ConfigParse(format!("library.toml: {error}"))
}
