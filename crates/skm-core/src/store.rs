use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use rusqlite::{Connection, OpenFlags};

use crate::{Error, paths::Paths};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_item_file_stamps.sql"),
    include_str!("../migrations/003_project_sync.sql"),
];
type WriteCommand = Box<dyn FnOnce(&mut Connection) + Send>;

/// A single writer thread and independent, per-call read-only connections.
pub struct Store {
    db_file: PathBuf,
    writer: Option<mpsc::Sender<WriteCommand>>,
    thread: Option<JoinHandle<()>>,
}

impl Store {
    /// Creates the home layout and database, applying pending migrations atomically.
    pub fn open(paths: &Paths) -> Result<Self, Error> {
        let mut paths = paths.clone();
        paths.ensure_layout()?;
        let mut connection = Connection::open(&paths.db_file)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let transaction = connection.transaction()?;
        let version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 0 || version > MIGRATIONS.len() as i64 {
            return Err(Error::Database(format!(
                "unsupported schema version {version}"
            )));
        }
        for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            transaction.execute_batch(migration)?;
            transaction.pragma_update(None, "user_version", (index + 1) as i64)?;
        }
        transaction.commit()?;

        let (writer, commands) = mpsc::channel::<WriteCommand>();
        let thread = thread::Builder::new()
            .name("skm-store".into())
            .spawn(move || {
                for command in commands {
                    command(&mut connection);
                }
            })?;
        Ok(Self {
            db_file: paths.db_file.clone(),
            writer: Some(writer),
            thread: Some(thread),
        })
    }

    /// Runs on the writer thread and waits for its result. Use a transaction for atomic writes.
    /// Do not call `write` recursively from a writer command.
    pub fn write<T, F>(&self, operation: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, Error> + Send + 'static,
    {
        let (reply, result) = mpsc::sync_channel(1);
        self.writer
            .as_ref()
            .expect("writer exists until drop")
            .send(Box::new(move |connection| {
                let _ = reply.send(operation(connection));
            }))
            .map_err(|_| Error::Database("store writer stopped".into()))?;
        result
            .recv()
            .map_err(|_| Error::Database("store writer stopped before replying".into()))?
    }

    /// Opens a read-only connection per call, avoiding shared locks or pool lifecycle state.
    pub fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let connection =
            Connection::open_with_flags(&self.db_file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        operation(&connection)
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.writer.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
