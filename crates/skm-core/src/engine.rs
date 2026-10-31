//! Scoped desired-state planning and guarded filesystem application.
use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::target::{ItemRef, TargetClass};

mod apply;
mod filesystem;
mod undo;

pub use apply::{apply, observe};
pub(crate) use filesystem::link_hash;
pub use filesystem::snapshot;
pub use undo::{Operation, list_ops, undo};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Link {
        target: PathBuf,
        to: PathBuf,
    },
    WriteFile {
        target: PathBuf,
        bytes: Vec<u8>,
    },
    CopyTree {
        target: PathBuf,
        from: PathBuf,
        hash: String,
        computed_hash: Option<String>,
    },
    /// Change only the ledger for an explicitly adopted, matching project copy.
    Claim {
        target: PathBuf,
    },
    Trash {
        target: PathBuf,
    },
    Restore {
        target: PathBuf,
        from: PathBuf,
        hash: String,
    },
}

impl Action {
    pub fn target(&self) -> &std::path::Path {
        match self {
            Self::Link { target, .. }
            | Self::WriteFile { target, .. }
            | Self::CopyTree { target, .. }
            | Self::Claim { target }
            | Self::Trash { target }
            | Self::Restore { target, .. } => target,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetState {
    pub class: TargetClass,
    pub mode: Option<u32>,
    /// Exact filesystem fingerprint, including links and ignored tree content.
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ownership {
    pub scope_id: i64,
    pub agent_id: Option<String>,
    pub item: ItemRef,
    pub how: String,
    pub written_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub target: PathBuf,
    pub state: TargetState,
    pub ledger: Option<Ownership>,
    pub parents_safe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesiredContent {
    Absent,
    Link { to: PathBuf },
    File { bytes: Vec<u8> },
    Tree { from: PathBuf, hash: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredTarget {
    pub target: PathBuf,
    pub item: ItemRef,
    pub agent_id: Option<String>,
    pub content: DesiredContent,
    pub approve_modified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedAction {
    pub action: Action,
    pub before: Observation,
    /// Ownership to install after success; None removes the ledger row.
    pub owner: Option<Ownership>,
    pub approve_modified: bool,
    pub adoption: Option<AdoptionAuthorization>,
    /// Checked under the apply lock before this action, including earlier effects.
    pub requires: Vec<Observation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionAuthorization {
    observed: Observation,
}

impl PlannedAction {
    /// Explicit adoption permission cannot follow a later edit to `before`.
    pub fn authorize_adoption(&mut self) {
        self.adoption = Some(AdoptionAuthorization {
            observed: self.before.clone(),
        });
    }

    pub(super) fn adoption_matches(&self) -> bool {
        self.before.parents_safe
            && self.before.state.hash.is_some()
            && self
                .adoption
                .as_ref()
                .is_some_and(|authorization| authorization.observed == self.before)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIssue {
    pub target: PathBuf,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub scope_id: i64,
    pub actions: Vec<PlannedAction>,
    pub issues: Vec<PlanIssue>,
    pub summary: String,
}

#[derive(Debug)]
pub struct ApplyResult {
    pub op_id: i64,
    pub done: Vec<Action>,
    pub issues: Vec<PlanIssue>,
    pub failed: Vec<PlanIssue>,
    /// Reverse order, ready to become a guarded undo plan.
    pub inverse: Vec<PlannedAction>,
}

pub(crate) fn issue(target: PathBuf, code: &str, message: impl Into<String>) -> PlanIssue {
    PlanIssue {
        target,
        code: code.into(),
        message: message.into(),
    }
}

pub(crate) fn writable(observed: &Observation, scope_id: i64, approved: bool) -> bool {
    if !observed.parents_safe {
        return false;
    }
    match &observed.state.class {
        TargetClass::Absent | TargetClass::OurLink(_) => true,
        TargetClass::ForeignLink(_) => false,
        TargetClass::RealFile | TargetClass::RealDir => {
            observed.ledger.as_ref().is_some_and(|row| {
                row.scope_id == scope_id
                    && (approved || observed.state.hash.as_ref() == Some(&row.written_hash))
            })
        }
    }
}

/// Pure: only the supplied targets are reconciled; omissions never imply removal.
/// Repeated identical destinations collapse; conflicting shared destinations refuse.
pub fn plan(scope_id: i64, desired: &[DesiredTarget], current: &[Observation]) -> Plan {
    let mut plan = Plan {
        scope_id,
        ..Plan::default()
    };
    let mut grouped: BTreeMap<&PathBuf, Vec<&DesiredTarget>> = BTreeMap::new();
    for desired in desired {
        grouped.entry(&desired.target).or_default().push(desired);
    }
    for (target, requests) in grouped {
        let desired = requests[0];
        if requests
            .iter()
            .any(|other| other.content != desired.content || other.item != desired.item)
        {
            plan.issues.push(issue(
                target.clone(),
                "E_CONFIG_CHANGED",
                "conflicting shared target requests",
            ));
            continue;
        }
        let Some(observed) = current.iter().find(|value| &value.target == target) else {
            plan.issues.push(issue(
                target.clone(),
                "E_CONFIG_CHANGED",
                "target was not observed",
            ));
            continue;
        };
        if !writable(observed, scope_id, desired.approve_modified) {
            let code = if observed.ledger.is_some() {
                "E_PROJECT_EDITED"
            } else {
                "E_FOREIGN_TARGET"
            };
            plan.issues.push(issue(
                target.clone(),
                code,
                "target is foreign, modified, or has a symlink ancestor",
            ));
            continue;
        }
        let (action, how) = match &desired.content {
            DesiredContent::Absent if observed.state.class == TargetClass::Absent => continue,
            DesiredContent::Absent => (
                Action::Trash {
                    target: target.clone(),
                },
                "",
            ),
            DesiredContent::Link { to } => {
                if matches!(&observed.state.class, TargetClass::OurLink(item) if item == &desired.item)
                    && observed.state.hash == Some(filesystem::link_hash(to))
                {
                    continue;
                }
                (
                    Action::Link {
                        target: target.clone(),
                        to: to.clone(),
                    },
                    "link",
                )
            }
            DesiredContent::File { bytes } => {
                if observed.state.class == TargetClass::RealFile
                    && observed.state.hash
                        == Some(filesystem::file_hash(bytes, observed.state.mode))
                {
                    continue;
                }
                (
                    Action::WriteFile {
                        target: target.clone(),
                        bytes: bytes.clone(),
                    },
                    "file",
                )
            }
            DesiredContent::Tree { from, hash } => {
                if observed.state.class == TargetClass::RealDir
                    && observed.state.hash.as_ref() == Some(hash)
                {
                    continue;
                }
                (
                    Action::CopyTree {
                        target: target.clone(),
                        from: from.clone(),
                        hash: hash.clone(),
                        computed_hash: None,
                    },
                    "copy",
                )
            }
        };
        let owner = (!how.is_empty()).then(|| Ownership {
            scope_id,
            agent_id: desired.agent_id.clone(),
            item: desired.item.clone(),
            how: how.into(),
            written_hash: String::new(),
        });
        plan.actions.push(PlannedAction {
            action,
            before: observed.clone(),
            owner,
            approve_modified: desired.approve_modified,
            adoption: None,
            requires: Vec::new(),
        });
    }
    plan.summary = format!(
        "{} actions, {} issues",
        plan.actions.len(),
        plan.issues.len()
    );
    plan
}
