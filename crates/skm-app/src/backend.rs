use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, ThreadId},
};

use skm_core::{
    Error, engine,
    events::CoreEvent,
    indexer,
    paths::Paths,
    projects,
    store::Store,
    watcher::{BatchIndexer, WatchRoot, Watcher},
};

use crate::model::{ItemChanges, Snapshot};

#[derive(Clone, Copy)]
pub struct CoreThread(ThreadId);

impl CoreThread {
    pub fn new() -> Self {
        Self(thread::current().id())
    }

    pub fn call<T>(&self, call: impl FnOnce() -> T) -> T {
        debug_assert_ne!(
            thread::current().id(),
            self.0,
            "blocking core call on the UI thread"
        );
        call()
    }
}

pub enum Update {
    Loaded(Snapshot),
    Items(ItemChanges),
    Error(String),
}

pub struct Shutdown {
    stopped: Arc<AtomicBool>,
    events: mpsc::Sender<CoreEvent>,
}

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Wake the bridge so it drops the watcher and store on its background thread.
        let _ = self.events.send(CoreEvent::PlanReady);
    }
}

pub fn channel() -> (Shutdown, mpsc::Receiver<CoreEvent>, mpsc::Sender<CoreEvent>) {
    let (events, receiver) = mpsc::channel();
    (
        Shutdown {
            stopped: Arc::default(),
            events: events.clone(),
        },
        receiver,
        events,
    )
}

pub fn run(
    guard: CoreThread,
    stopped: Arc<AtomicBool>,
    events: mpsc::Sender<CoreEvent>,
    receiver: mpsc::Receiver<CoreEvent>,
    updates: mpsc::Sender<Update>,
    manager_home: Option<PathBuf>,
    user_home: Option<PathBuf>,
) {
    let result = guard.call(|| -> Result<(), Error> {
        let mut paths = Paths::resolve(None, manager_home.as_deref(), user_home.as_deref())?;
        paths.ensure_layout()?;
        let store = Arc::new(Store::open(&paths)?);
        let snapshot = load(&store)?;
        let mut roots = vec![
            WatchRoot::Tree(paths.library.clone()),
            WatchRoot::File(paths.home.join(".changed")),
            WatchRoot::File(paths.config_file.clone()),
        ];
        if let Some(home) = &user_home {
            roots.push(WatchRoot::Tree(home.join(".agents/skills")));
        }
        for project in &snapshot.projects {
            roots.push(WatchRoot::Tree(project.path.join(".agents/skills")));
            roots.push(WatchRoot::File(project.path.join("skills-lock.json")));
        }
        let batch = BatchIndexer::new(paths.clone(), store.clone(), vec![], events)?;
        let _watcher = Watcher::start(roots, batch)?;
        if updates.send(Update::Loaded(snapshot)).is_err() {
            return Ok(());
        }
        // Publish cached rows first, then catch up on the same worker.
        let before: BTreeMap<_, _> = indexer::load(&store)?
            .into_iter()
            .map(|row| (row.id, row))
            .collect();
        indexer::catch_up(&store, &paths, None)?;
        let after: BTreeMap<_, _> = indexer::load(&store)?
            .into_iter()
            .map(|row| (row.id, row))
            .collect();
        let ids: BTreeSet<_> = before
            .keys()
            .chain(after.keys())
            .filter(|id| before.get(id) != after.get(id))
            .copied()
            .collect();
        let rows = after
            .into_values()
            .filter(|row| ids.contains(&row.id))
            .map(Into::into)
            .collect();
        if updates
            .send(Update::Items(ItemChanges { ids, rows }))
            .is_err()
        {
            return Ok(());
        }
        while let Ok(event) = receiver.recv() {
            if stopped.load(Ordering::Acquire) {
                break;
            }
            let update = match event {
                CoreEvent::ItemsChanged { ids } => {
                    let ids: BTreeSet<_> =
                        ids.iter().filter_map(|id| id.parse::<i64>().ok()).collect();
                    let rows = indexer::load(&store)?
                        .into_iter()
                        .filter(|row| ids.contains(&row.id))
                        .map(Into::into)
                        .collect();
                    Update::Items(ItemChanges { ids, rows })
                }
                CoreEvent::Error(error) => Update::Error(format!("{}: {error}", error.code())),
                _ => continue,
            };
            if updates.send(update).is_err() {
                break;
            }
        }
        Ok(())
    });
    if let Err(error) = result {
        let _ = updates.send(Update::Error(format!("{}: {error}", error.code())));
    }
}

impl Shutdown {
    pub fn stopped(&self) -> Arc<AtomicBool> {
        self.stopped.clone()
    }
}

fn load(store: &Store) -> Result<Snapshot, Error> {
    Ok(Snapshot {
        items: indexer::load(store)?.into_iter().map(Into::into).collect(),
        projects: projects::list(store)?,
        sets: store.read(|connection| {
            let mut statement = connection
                .prepare("SELECT DISTINCT set_name FROM project_sets ORDER BY set_name")?;
            Ok(statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?)
        })?,
        activity: engine::list_ops(store)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn skill(paths: &Paths, name: &str, description: &str) {
        let directory = paths.library.join("skills").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nBody\n"),
        )
        .unwrap();
    }

    #[test]
    fn sidebar_loads_cached_items_projects_sets_and_activity_from_sqlite() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(Some(root.path()), None, None).unwrap();
        let store = Store::open(&paths).unwrap();
        skill(&paths, "rust", "Rust");
        indexer::catch_up(&store, &paths, None).unwrap();
        store.write(|connection| {
            connection.execute_batch("INSERT INTO scopes(id,kind,path,name,added_at) VALUES (7,'project','/fake/project','Project',0);
                INSERT INTO project_sets(scope_id,set_name) VALUES (7,'work');
                INSERT INTO ops(at,actor,summary,plan_json,result) VALUES (0,'cli','Enabled rust','{}','done');")?;
            Ok(())
        }).unwrap();
        fs::rename(&paths.library, root.path().join("unavailable")).unwrap();
        let guard = CoreThread::new();
        let snapshot = thread::spawn(move || guard.call(|| load(&store).unwrap()))
            .join()
            .unwrap();
        assert_eq!(snapshot.items[0].name, "rust");
        assert_eq!(snapshot.projects[0].name, "Project");
        assert_eq!(snapshot.sets, ["work"]);
        assert_eq!(snapshot.activity[0].summary, "Enabled rust");
        assert!(!paths.library.exists());
    }

    #[test]
    fn watcher_bridge_updates_only_affected_model_rows_and_stops_when_idle() {
        let root = tempfile::tempdir().unwrap();
        let user_home = root.path().join("user");
        fs::create_dir(&user_home).unwrap();
        let paths = Paths::resolve(Some(&root.path().join("manager")), None, None).unwrap();
        let store = Store::open(&paths).unwrap();
        skill(&paths, "rust", "Rust");
        skill(&paths, "git", "Git");
        indexer::catch_up(&store, &paths, None).unwrap();
        drop(store);

        let guard = CoreThread::new();
        let (shutdown, receiver, events) = channel();
        let stopped = shutdown.stopped();
        let (updates, incoming) = mpsc::channel();
        let manager_home = paths.home.clone();
        let worker = thread::spawn(move || {
            run(
                guard,
                stopped,
                events,
                receiver,
                updates,
                Some(manager_home),
                Some(user_home),
            )
        });
        let snapshot = match incoming.recv_timeout(Duration::from_secs(10)).unwrap() {
            Update::Loaded(snapshot) => snapshot,
            _ => panic!("expected cached snapshot"),
        };
        let rust = snapshot
            .items
            .iter()
            .find(|row| row.name == "rust")
            .unwrap()
            .id;
        let git = snapshot
            .items
            .iter()
            .find(|row| row.name == "git")
            .unwrap()
            .id;
        let mut model = Model::default();
        model.apply(ItemChanges {
            ids: snapshot.items.iter().map(|row| row.id).collect(),
            rows: snapshot.items,
        });
        let Update::Items(initial) = incoming.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("expected startup catch-up");
        };
        model.apply(initial);
        let before = model.row_updates.clone();
        skill(&paths, "rust", "Updated Rust");
        let deadline = Instant::now() + Duration::from_secs(10);
        let changed = loop {
            match incoming
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                Update::Items(changes) if changes.ids.contains(&rust) => break changes,
                Update::Error(error) => panic!("{error}"),
                _ => {}
            }
        };
        assert_eq!(model.apply(changed), [rust]);
        assert_eq!(model.row_updates[&rust], before[&rust] + 1);
        assert_eq!(model.row_updates[&git], before[&git]);
        assert_eq!(model.items[&git].name, "git");
        drop(shutdown);
        assert!(matches!(
            incoming.recv_timeout(Duration::from_secs(10)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        worker.join().unwrap();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "blocking core call on the UI thread")]
    fn core_call_guard_rejects_ui_thread() {
        CoreThread::new().call(|| ());
    }

    #[test]
    fn core_call_guard_allows_background_thread() {
        let guard = CoreThread::new();
        assert_eq!(thread::spawn(move || guard.call(|| 42)).join().unwrap(), 42);
    }
}
