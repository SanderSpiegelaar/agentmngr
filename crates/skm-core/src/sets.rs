//! Synced set definitions. Persist changes with `Library::save`.
use std::collections::BTreeMap;

use crate::{
    Error,
    library::{ItemSelection, ItemSet, Library},
    target::{ItemKind, ItemRef},
};

impl Library {
    pub fn create_set(&mut self, name: &str, description: Option<String>) -> Result<(), Error> {
        if name.trim().is_empty() {
            return Err(Error::ConfigParse("set name cannot be empty".into()));
        }
        if self.sets.contains_key(name) {
            return Err(Error::NameCollision(format!("set {name:?} already exists")));
        }
        self.sets.insert(
            name.into(),
            ItemSet {
                description,
                ..ItemSet::default()
            },
        );
        Ok(())
    }

    pub fn update_set(
        &mut self,
        name: &str,
        set: ItemSet,
        known_agents: &[&str],
    ) -> Result<(), Error> {
        if !self.sets.contains_key(name) {
            return Err(Error::ConfigParse(format!("unknown set {name:?}")));
        }
        let mut candidate = self.clone();
        candidate.sets.insert(name.into(), set);
        candidate.validate(known_agents)?;
        *self = candidate;
        Ok(())
    }

    pub fn delete_set(&mut self, name: &str) -> Result<ItemSet, Error> {
        self.sets
            .remove(name)
            .ok_or_else(|| Error::ConfigParse(format!("unknown set {name:?}")))
    }
}

impl ItemSelection {
    pub fn set_item(&mut self, item: &ItemRef, agents: Option<&[String]>) -> Result<(), Error> {
        crate::global_skills::validate_name(&item.name)?;
        if item.kind == ItemKind::Plugin {
            return Err(Error::ConfigParse(
                "plugins cannot belong to sets or projects".into(),
            ));
        }
        if item.kind == ItemKind::Skill {
            if !self.skills.contains(&item.name) {
                self.skills.push(item.name.clone());
                self.skills.sort();
            }
        } else {
            let mut agents = agents
                .map(<[String]>::to_vec)
                .unwrap_or_else(|| vec!["*".into()]);
            agents.sort();
            agents.dedup();
            self.entries_mut(item.kind)
                .unwrap()
                .insert(item.name.clone(), agents);
        }
        Ok(())
    }

    pub fn remove_item(&mut self, item: &ItemRef) {
        if item.kind == ItemKind::Skill {
            self.skills.retain(|name| name != &item.name);
        } else if let Some(entries) = self.entries_mut(item.kind) {
            entries.remove(&item.name);
        }
    }

    pub fn entries(&self, kind: ItemKind) -> Option<&BTreeMap<String, Vec<String>>> {
        match kind {
            ItemKind::Skill => None,
            ItemKind::Mcp => Some(&self.mcp),
            ItemKind::Hook => Some(&self.hooks),
            ItemKind::Subagent => Some(&self.subagents),
            ItemKind::Plugin => Some(&self.plugins),
        }
    }

    pub(crate) fn entries_mut(
        &mut self,
        kind: ItemKind,
    ) -> Option<&mut BTreeMap<String, Vec<String>>> {
        match kind {
            ItemKind::Skill => None,
            ItemKind::Mcp => Some(&mut self.mcp),
            ItemKind::Hook => Some(&mut self.hooks),
            ItemKind::Subagent => Some(&mut self.subagents),
            ItemKind::Plugin => Some(&mut self.plugins),
        }
    }
}
