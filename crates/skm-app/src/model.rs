use std::collections::{BTreeMap, BTreeSet};

use skm_core::{engine::Operation, indexer::IndexRow, projects::Project};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub path: String,
}

impl From<IndexRow> for Item {
    fn from(row: IndexRow) -> Self {
        Self {
            id: row.id,
            kind: row.kind,
            name: row.name,
            path: row.rel_path,
        }
    }
}

pub struct Snapshot {
    pub items: Vec<Item>,
    pub projects: Vec<Project>,
    pub sets: Vec<String>,
    pub activity: Vec<Operation>,
}

pub struct ItemChanges {
    pub ids: BTreeSet<i64>,
    pub rows: Vec<Item>,
}

#[derive(Default)]
pub struct Model {
    pub items: BTreeMap<i64, Item>,
    pub row_updates: BTreeMap<i64, usize>,
}

impl Model {
    pub fn apply(&mut self, changes: ItemChanges) -> Vec<i64> {
        let mut rows: BTreeMap<_, _> = changes
            .rows
            .into_iter()
            .map(|item| (item.id, item))
            .collect();
        for id in &changes.ids {
            if let Some(item) = rows.remove(id) {
                self.items.insert(*id, item);
            } else {
                self.items.remove(id);
            }
            *self.row_updates.entry(*id).or_default() += 1;
        }
        changes.ids.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, name: &str) -> Item {
        Item {
            id,
            kind: "skill".into(),
            name: name.into(),
            path: format!("skills/{name}"),
        }
    }

    #[test]
    fn item_changes_update_insert_and_delete_only_named_rows() {
        let mut model = Model::default();
        model.apply(ItemChanges {
            ids: [1, 2].into(),
            rows: vec![item(1, "one"), item(2, "two")],
        });
        let untouched = model.items[&2].clone();
        assert_eq!(
            model.apply(ItemChanges {
                ids: [1, 3].into(),
                rows: vec![item(1, "renamed"), item(3, "three"), item(2, "unrequested")]
            }),
            [1, 3]
        );
        assert_eq!(model.items[&1].name, "renamed");
        assert_eq!(model.items[&3].name, "three");
        model.apply(ItemChanges {
            ids: [1].into(),
            rows: vec![],
        });
        assert!(!model.items.contains_key(&1));
        assert_eq!(model.items[&2], untouched);
        assert_eq!(model.row_updates[&2], 1);
        assert_eq!(model.row_updates[&1], 3);
        assert_eq!(model.row_updates[&3], 1);
    }
}
