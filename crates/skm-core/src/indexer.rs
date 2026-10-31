use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufReader};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Error, frontmatter, library::Library, paths::Paths, registry::Registry, store::Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub mtime_ns: i64,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub rel_path: String,
    pub stamp: Stamp,
    /// SKILL.md for skills, plugin.json for plugins; absent for single-file items.
    pub file_stamp: Option<Stamp>,
    pub content_hash: Option<String>,
    pub meta_json: String,
    pub problems_json: String,
    pub source_json: Option<String>,
    pub indexed_at: i64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub inserted: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

/// Startup reads SQLite only, even if the library is unavailable.
pub fn load(store: &Store) -> Result<Vec<IndexRow>, Error> {
    store.read(load_rows)
}

fn load_rows(connection: &Connection) -> Result<Vec<IndexRow>, Error> {
    let mut query = connection.prepare(
        "SELECT id, kind, name, description, rel_path, stamp_mtime, stamp_size,
         content_hash, meta_json, problems_json, source_json, indexed_at,
         file_stamp_mtime, file_stamp_size FROM items ORDER BY kind, name",
    )?;
    Ok(query
        .query_map([], |row| {
            let file_mtime: Option<i64> = row.get(12)?;
            Ok(IndexRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                rel_path: row.get(4)?,
                stamp: Stamp {
                    mtime_ns: row.get(5)?,
                    size: row.get(6)?,
                },
                content_hash: row.get(7)?,
                meta_json: row.get(8)?,
                problems_json: row.get(9)?,
                source_json: row.get(10)?,
                indexed_at: row.get(11)?,
                file_stamp: file_mtime
                    .map(|mtime_ns| {
                        Ok::<_, rusqlite::Error>(Stamp {
                            mtime_ns,
                            size: row.get(13)?,
                        })
                    })
                    .transpose()?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

/// None discovers all items; Some limits work to existing item IDs from watcher events.
pub fn catch_up(store: &Store, paths: &Paths, ids: Option<&[i64]>) -> Result<Counts, Error> {
    reindex(store, paths, ids, false)
}

/// Re-parses every item, preserving IDs, lazy hashes and indexed_at for identical rows.
pub fn full_reindex(store: &Store, paths: &Paths) -> Result<Counts, Error> {
    reindex(store, paths, None, true)
}

fn reindex(
    store: &Store,
    paths: &Paths,
    ids: Option<&[i64]>,
    force: bool,
) -> Result<Counts, Error> {
    let library_path = paths.library.clone();
    let ids = ids.map(|ids| ids.iter().copied().collect::<BTreeSet<_>>());
    store.write(move |connection| {
        let transaction = connection.transaction()?;
        let old = load_rows(&transaction)?;
        let candidates = if let Some(ids) = &ids {
            old.iter().filter(|row| ids.contains(&row.id))
                .map(|row| (row.rel_path.clone(), (row.kind.clone(), row.name.clone())))
                .collect()
        } else {
            discover(&library_path)?
        };
        let manifest = library_path.join("library.toml");
        let library = match fs::read_to_string(&manifest) {
            Ok(text) => {
                let registry = Registry::load()?;
                Library::parse(&text, &registry.agents.keys().map(String::as_str).collect::<Vec<_>>())?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Library::default(),
            Err(error) => return Err(error.into()),
        };
        let previous: BTreeMap<_, _> = old.iter().map(|row| (row.rel_path.as_str(), row)).collect();
        let mut seen = BTreeSet::new();
        let mut counts = Counts::default();
        for (rel_path, (kind, name)) in candidates {
            let path = library_path.join(&rel_path);
            let Some(stamp) = stat(&path)? else { continue };
            let file_stamp = match kind.as_str() {
                "skill" => stat(&path.join("SKILL.md"))?,
                "plugin" => stat(&path.join("plugin.json"))?,
                _ => None,
            };
            let source_json = library.skills.get(&name).filter(|_| kind == "skill")
                .map(json_string).transpose()?;
            let before = previous.get(rel_path.as_str()).copied();
            seen.insert(rel_path.clone());
            if !force && ids.is_none() && before.is_some_and(|row| row.stamp == stamp && row.file_stamp == file_stamp && row.source_json == source_json) {
                counts.unchanged += 1;
                continue;
            }
            let mut row = parse_item(&path, &kind, &name)?;
            row.rel_path = rel_path;
            row.stamp = stamp;
            row.file_stamp = file_stamp;
            row.source_json = source_json;
            if let Some(before) = before {
                row.id = before.id;
                row.indexed_at = before.indexed_at;
                if ids.is_none() && row.stamp == before.stamp && row.file_stamp == before.file_stamp && row.meta_json == before.meta_json {
                    row.content_hash = before.content_hash.clone();
                }
                if &row == before {
                    counts.unchanged += 1;
                    continue;
                }
                counts.updated += 1;
            } else {
                counts.inserted += 1;
            }
            row.indexed_at = SystemTime::now().duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?.as_millis() as i64;
            transaction.execute(
                "INSERT INTO items (kind,name,description,rel_path,stamp_mtime,stamp_size,content_hash,meta_json,problems_json,source_json,indexed_at,file_stamp_mtime,file_stamp_size)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                 ON CONFLICT(kind,name) DO UPDATE SET description=excluded.description,rel_path=excluded.rel_path,
                 stamp_mtime=excluded.stamp_mtime,stamp_size=excluded.stamp_size,content_hash=excluded.content_hash,
                 meta_json=excluded.meta_json,problems_json=excluded.problems_json,source_json=excluded.source_json,
                 indexed_at=excluded.indexed_at,file_stamp_mtime=excluded.file_stamp_mtime,file_stamp_size=excluded.file_stamp_size",
                params![row.kind,row.name,row.description,row.rel_path,row.stamp.mtime_ns,row.stamp.size,
                    row.content_hash,row.meta_json,row.problems_json,row.source_json,row.indexed_at,
                    row.file_stamp.map(|stamp| stamp.mtime_ns),row.file_stamp.map(|stamp| stamp.size)],
            )?;
        }
        for row in &old {
            if ids.as_ref().is_none_or(|ids| ids.contains(&row.id)) && !seen.contains(&row.rel_path) {
                transaction.execute("DELETE FROM items WHERE id=?1", [row.id])?;
                counts.removed += 1;
            }
        }
        counts.total = transaction.query_row("SELECT count(*) FROM items", [], |row| row.get::<_, i64>(0))? as usize;
        transaction.commit()?;
        Ok(counts)
    })
}

fn discover(library: &Path) -> Result<BTreeMap<String, (String, String)>, Error> {
    let mut items = BTreeMap::new();
    for (folder, kind, extension) in [
        ("skills", "skill", None),
        ("mcp", "mcp", Some("json")),
        ("hooks", "hook", Some("json")),
        ("subagents", "subagent", Some("md")),
        ("plugins", "plugin", None),
    ] {
        let entries = match fs::read_dir(library.join(folder)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let filename = entry.file_name();
            if filename.to_string_lossy().starts_with('.') {
                continue;
            }
            let file_type = entry.file_type()?;
            if extension.is_some_and(|ext| path.extension().is_none_or(|actual| actual != ext))
                || (extension.is_none() && !file_type.is_dir())
                || (extension.is_some() && !file_type.is_file())
            {
                continue;
            }
            let name = if extension.is_some() {
                path.file_stem()
            } else {
                path.file_name()
            }
            .unwrap()
            .to_string_lossy()
            .into_owned();
            items.insert(
                format!("{folder}/{}", filename.to_string_lossy()),
                (kind.into(), name),
            );
        }
    }
    Ok(items)
}

fn stat(path: &Path) -> Result<Option<Stamp>, Error> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let modified = metadata.modified()?;
    let mtime_ns = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i64,
        Err(error) => -(error.duration().as_nanos() as i64),
    };
    Ok(Some(Stamp {
        mtime_ns,
        size: metadata.len() as i64,
    }))
}

fn parse_item(path: &Path, kind: &str, name: &str) -> Result<IndexRow, Error> {
    let mut problems = Vec::new();
    let meta = match kind {
        "skill" | "subagent" => {
            let (fields, warnings) = if kind == "skill" {
                frontmatter::read_skill(path)?
            } else {
                frontmatter::read_frontmatter(
                    &mut BufReader::with_capacity(1, fs::File::open(path)?),
                    name,
                )?
            };
            problems.extend(warnings.into_iter().map(|warning| warning.to_string()));
            if kind == "skill" {
                inspect_skill(path, &fs::canonicalize(path)?, &mut problems)?;
            }
            json!({"name": fields.name, "description": fields.description, "license": fields.license,
                "compatibility": fields.compatibility, "metadata": fields.metadata, "allowed-tools": fields.allowed_tools})
        }
        _ => {
            let file = if kind == "plugin" {
                path.join("plugin.json")
            } else {
                path.to_owned()
            };
            match fs::read_to_string(&file) {
                Ok(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(value) => value,
                    Err(error) => {
                        problems.push(format!("Invalid JSON: {error}"));
                        json!({})
                    }
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    problems.push(format!(
                        "Missing {}",
                        file.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    json!({})
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    Ok(IndexRow {
        id: 0,
        kind: kind.into(),
        name: name.into(),
        description: meta
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        rel_path: String::new(),
        stamp: Stamp {
            mtime_ns: 0,
            size: 0,
        },
        file_stamp: None,
        content_hash: None,
        meta_json: json_string(&meta)?,
        problems_json: json_string(&problems)?,
        source_json: None,
        indexed_at: 0,
    })
}

fn inspect_skill(path: &Path, root: &Path, problems: &mut Vec<String>) -> Result<(), Error> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_symlink() {
            if fs::canonicalize(&path).is_ok_and(|target| {
                fs::canonicalize(root).is_ok_and(|root| !target.starts_with(root))
            }) {
                problems.push(format!(
                    "Symlink points outside skill: {}",
                    path.strip_prefix(root).unwrap_or(&path).display()
                ));
            }
        } else if metadata.is_dir() {
            if entry.file_name() != ".git" {
                inspect_skill(&path, root, problems)?;
            }
        } else if metadata.len() > 5 * 1024 * 1024 {
            problems.push(format!(
                "File exceeds 5 MB: {}",
                path.strip_prefix(root).unwrap_or(&path).display()
            ));
        }
    }
    problems.sort();
    Ok(())
}

fn json_string(value: &impl Serialize) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|error| Error::ConfigParse(error.to_string()))
}
