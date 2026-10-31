use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use notify_debouncer_full::DebouncedEvent;

use crate::{Error, engine, events::CoreEvent, indexer, paths::Paths, store::Store};

const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub enum WatchRoot {
    Tree(PathBuf),
    File(PathBuf),
}

#[derive(Debug, Clone)]
pub struct WatchedTarget {
    pub path: PathBuf,
    pub item_id: String,
    /// Config key components, e.g. ["mcpServers", "github"]. None observes the whole target.
    pub entry: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
enum TargetValue {
    Path(engine::TargetState),
    Entry(Option<serde_json::Value>),
}

/// A deterministic batch processor, also used by the native watcher worker.
pub struct BatchIndexer {
    paths: Paths,
    store: Arc<Store>,
    targets: BTreeMap<PathBuf, Vec<(WatchedTarget, TargetValue)>>,
    events: mpsc::Sender<CoreEvent>,
}

impl BatchIndexer {
    pub fn new(
        mut paths: Paths,
        store: Arc<Store>,
        targets: Vec<WatchedTarget>,
        events: mpsc::Sender<CoreEvent>,
    ) -> Result<Self, Error> {
        paths.library = normalize(&paths.library)?;
        paths.home = normalize(&paths.home)?;
        let mut by_path: BTreeMap<PathBuf, Vec<_>> = BTreeMap::new();
        for mut target in targets {
            target.path = normalize(&target.path)?;
            let value = observe(&target, &paths)?;
            by_path
                .entry(target.path.clone())
                .or_default()
                .push((target, value));
        }
        Ok(Self {
            paths,
            store,
            targets: by_path,
            events,
        })
    }

    /// Rescan flags select descendants of the flagged path, never unrelated roots.
    pub fn process(&mut self, batch: &[Event]) -> Result<Vec<String>, Error> {
        let batch = batch
            .iter()
            .map(|event| {
                let mut event = event.clone();
                event.paths = event
                    .paths
                    .iter()
                    .map(|path| normalize(path))
                    .collect::<Result<_, _>>()?;
                Ok(event)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let changed: Vec<_> = batch
            .iter()
            .filter(|event| !matches!(event.kind, EventKind::Access(_)))
            .flat_map(|event| &event.paths)
            .filter(|path| !ignored(path))
            .collect();
        let before = indexer::load(&self.store)?;
        let mut prefixes = BTreeSet::new();
        let all = changed.iter().any(|path| {
            **path == self.paths.home.join(".changed")
                || **path == self.paths.library.join("library.toml")
        });
        for path in &changed {
            if let Ok(relative) = path.strip_prefix(&self.paths.library) {
                prefixes.insert(relative.components().take(2).collect::<PathBuf>());
            } else if self.paths.library.starts_with(path) {
                prefixes.insert(PathBuf::new());
            }
        }
        if all {
            prefixes.clear();
            indexer::catch_up(&self.store, &self.paths, None)?;
        }
        if !prefixes.is_empty() {
            indexer::catch_up_paths(
                &self.store,
                &self.paths,
                &prefixes.into_iter().collect::<Vec<_>>(),
            )?;
        }
        let after = indexer::load(&self.store)?;
        let old: BTreeMap<_, _> = before.iter().map(|row| (row.id, row)).collect();
        let new: BTreeMap<_, _> = after.iter().map(|row| (row.id, row)).collect();
        let mut ids: BTreeSet<String> = old
            .keys()
            .chain(new.keys())
            .filter(|id| old.get(id) != new.get(id))
            .map(ToString::to_string)
            .collect();
        let mut selected = BTreeSet::new();
        if all {
            selected.extend(self.targets.keys().cloned());
        } else {
            for path in changed {
                for ancestor in path.ancestors() {
                    if self.targets.contains_key(ancestor) {
                        selected.insert(ancestor.to_owned());
                    }
                }
                // Directory events, including FSEvents Rescan, select only this subtree.
                selected.extend(
                    self.targets
                        .range(path.clone()..)
                        .take_while(|(target, _)| target.starts_with(path))
                        .map(|(path, _)| path.clone()),
                );
            }
        }
        let mut errors = Vec::new();
        for path in selected {
            for (target, previous) in self.targets.get_mut(&path).unwrap() {
                let current = match observe(target, &self.paths) {
                    Ok(current) => current,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                };
                if *previous != current {
                    *previous = current;
                    ids.insert(target.item_id.clone());
                }
            }
        }
        let ids: Vec<_> = ids.into_iter().collect();
        if !ids.is_empty() {
            self.events
                .send(CoreEvent::ItemsChanged { ids: ids.clone() })
                .map_err(|_| io::Error::other("watcher event receiver disconnected"))?;
        }
        for error in errors {
            self.events
                .send(CoreEvent::Error(error))
                .map_err(|_| io::Error::other("watcher event receiver disconnected"))?;
        }
        Ok(ids)
    }
}

fn observe(target: &WatchedTarget, paths: &Paths) -> Result<TargetValue, Error> {
    let Some(keys) = &target.entry else {
        return Ok(TargetValue::Path(engine::snapshot(&target.path, paths)?));
    };
    let text = match fs::read_to_string(&target.path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(TargetValue::Entry(None));
        }
        Err(error) => return Err(error.into()),
    };
    let value: serde_json::Value = if target.path.extension().is_some_and(|ext| ext == "toml") {
        let parsed: toml::Value =
            toml::from_str(&text).map_err(|error| Error::ConfigParse(error.to_string()))?;
        serde_json::to_value(parsed).map_err(|error| Error::ConfigParse(error.to_string()))?
    } else {
        serde_json::from_str(&text).map_err(|error| Error::ConfigParse(error.to_string()))?
    };
    let mut entry = Some(&value);
    for key in keys {
        entry = entry.and_then(|value| value.get(key));
    }
    Ok(TargetValue::Entry(entry.cloned()))
}

fn ignored(path: &Path) -> bool {
    path.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some(".git" | "node_modules" | "target" | ".venv" | "__pycache__" | ".DS_Store")
        )
    })
}

// Native FSEvents reports canonical ancestors (e.g. /private/var rather than /var).
// Do not resolve the final component, which may itself be a managed symlink.
fn normalize(path: &Path) -> Result<PathBuf, Error> {
    let path = std::path::absolute(path)?;
    let mut ancestor = path.parent().unwrap_or(&path);
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| io::Error::other("path has no existing ancestor"))?;
    }
    Ok(fs::canonicalize(ancestor)?.join(path.strip_prefix(ancestor).unwrap()))
}

enum Command {
    Event(notify::Result<Event>),
    Stop,
}

pub struct Watcher {
    native: Option<RecommendedWatcher>,
    commands: mpsc::Sender<Command>,
    worker: Option<JoinHandle<()>>,
    #[cfg(test)]
    wakeups: Arc<std::sync::atomic::AtomicUsize>,
}

impl Watcher {
    /// Roots are explicit; no real home or registered-project discovery happens here.
    pub fn start(roots: Vec<WatchRoot>, mut indexer: BatchIndexer) -> Result<Self, Error> {
        let roots = roots
            .into_iter()
            .map(|root| match root {
                WatchRoot::Tree(path) => Ok(WatchRoot::Tree(normalize(&path)?)),
                WatchRoot::File(path) => Ok(WatchRoot::File(normalize(&path)?)),
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let (commands, receiver) = mpsc::channel();
        let sender = commands.clone();
        let mut native = notify::recommended_watcher(move |event| {
            let _ = sender.send(Command::Event(event));
        })
        .map_err(watch_error)?;
        let mut watches = BTreeMap::new();
        for root in &roots {
            let (requested, recursive) = match root {
                WatchRoot::Tree(path) => (path.as_path(), true),
                WatchRoot::File(path) => (path.parent().unwrap_or(path), false),
            };
            let mut path = requested;
            while !path.is_dir() {
                path = path
                    .parent()
                    .ok_or_else(|| io::Error::other("watch root has no existing ancestor"))?;
            }
            let recursive = recursive || path != requested;
            watches
                .entry(path.to_owned())
                .and_modify(|value| *value |= recursive)
                .or_insert(recursive);
        }
        for (path, recursive) in watches {
            native
                .watch(
                    &path,
                    if recursive {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    },
                )
                .map_err(watch_error)?;
        }
        #[cfg(test)]
        let wakeups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let worker_wakeups = wakeups.clone();
        let worker = thread::Builder::new()
            .name("skm-watcher".into())
            .spawn(move || {
                let mut pending: Vec<DebouncedEvent> = Vec::new();
                loop {
                    // The stock debouncer sleeps on a periodic tick even when idle.
                    // Block indefinitely when empty; arm a deadline only after an event.
                    let command = if pending.is_empty() {
                        receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    } else {
                        receiver.recv_timeout(
                            (pending.last().unwrap().time + DEBOUNCE)
                                .saturating_duration_since(Instant::now()),
                        )
                    };
                    #[cfg(test)]
                    worker_wakeups.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match command {
                        Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Command::Event(Ok(event))) => {
                            if matches!(event.kind, EventKind::Access(_)) {
                                continue;
                            }
                            let mut event = event;
                            event.paths.retain(|path| {
                                roots.iter().any(|root| match root {
                                    WatchRoot::Tree(root) => {
                                        path.starts_with(root) || root.starts_with(&*path)
                                    }
                                    WatchRoot::File(root) => {
                                        path == root || root.starts_with(&*path)
                                    }
                                }) && !ignored(path)
                            });
                            if !event.paths.is_empty() {
                                pending.push(DebouncedEvent::new(event, Instant::now()));
                            }
                        }
                        Ok(Command::Event(Err(error))) => {
                            let _ = indexer.events.send(CoreEvent::Error(watch_error(error)));
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let batch: Vec<_> =
                                pending.drain(..).map(|event| event.event).collect();
                            if let Err(error) = indexer.process(&batch) {
                                let _ = indexer.events.send(CoreEvent::Error(error));
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            native: Some(native),
            commands,
            worker: Some(worker),
            #[cfg(test)]
            wakeups,
        })
    }
}

fn watch_error(error: notify::Error) -> Error {
    io::Error::other(error.to_string()).into()
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.native.take();
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn idle_worker_has_zero_timer_wakeups() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(Some(root.path()), None, None).unwrap();
        let store = Arc::new(Store::open(&paths).unwrap());
        let (sender, _receiver) = crate::events::channel();
        let processor = BatchIndexer::new(paths.clone(), store, vec![], sender).unwrap();
        let watcher =
            Watcher::start(vec![WatchRoot::Tree(paths.library.clone())], processor).unwrap();
        thread::sleep(Duration::from_millis(600));
        assert_eq!(watcher.wakeups.load(Ordering::Relaxed), 0);
        watcher
            .commands
            .send(Command::Event(Ok(Event::new(EventKind::Any).add_path(
                normalize(&paths.library.join("mcp/missing.json")).unwrap(),
            ))))
            .unwrap();
        thread::sleep(Duration::from_millis(600));
        assert_eq!(watcher.wakeups.load(Ordering::Relaxed), 2);
        thread::sleep(Duration::from_millis(600));
        assert_eq!(watcher.wakeups.load(Ordering::Relaxed), 2);
    }
}
