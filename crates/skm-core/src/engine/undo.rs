use std::sync::mpsc::Sender;

use super::{ApplyResult, Plan, PlannedAction, apply};
use crate::{Error, events::CoreEvent, paths::Paths, store::Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub id: i64,
    pub at: i64,
    pub actor: String,
    pub summary: String,
    pub result: String,
}

/// Most recent first, including undo operations and partial applies.
pub fn list_ops(store: &Store) -> Result<Vec<Operation>, Error> {
    store.read(|connection| {
        let mut query = connection
            .prepare("SELECT id,at,actor,summary,result FROM ops ORDER BY id DESC LIMIT 50")?;
        Ok(query
            .query_map([], |row| {
                Ok(Operation {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    actor: row.get(2)?,
                    summary: row.get(3)?,
                    result: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

/// Replays a recent operation's inverses, or the latest operation when omitted.
/// Conflicts found during locked preflight leave targets and history unchanged.
pub fn undo(
    paths: &Paths,
    store: &Store,
    op_id: Option<i64>,
    actor: &str,
    events: Option<&Sender<CoreEvent>>,
) -> Result<ApplyResult, Error> {
    let operations = list_ops(store)?;
    let operation = operations
        .iter()
        .find(|operation| op_id.is_none_or(|id| operation.id == id))
        .ok_or_else(|| Error::ConfigChanged("operation is not among the last 50 ops".into()))?;
    if operation.result == "applying" {
        return Err(Error::ConfigChanged(
            "operation did not finish applying".into(),
        ));
    }
    let (original, inverse) = store.read(|connection| {
        Ok(connection.query_row(
            "SELECT plan_json,inverse_json FROM ops WHERE id=?1",
            [operation.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?)
    })?;
    let mut plan: Plan =
        serde_json::from_str(&original).map_err(|error| Error::ConfigParse(error.to_string()))?;
    let inverse: Vec<PlannedAction> =
        serde_json::from_str(&inverse).map_err(|error| Error::ConfigParse(error.to_string()))?;
    plan.actions = inverse;
    plan.issues.clear();
    plan.summary = format!("Undo op {}: {}", operation.id, operation.summary);
    apply::apply_checked(paths, store, &plan, actor, events, true)
}
