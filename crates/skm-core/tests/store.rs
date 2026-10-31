use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use skm_core::{Error, paths::Paths, store::Store};

#[test]
fn migrations_from_zero_are_idempotent_and_match_spec() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let paths = Paths::resolve(Some(root.path()), None, None)?;
    drop(rusqlite::Connection::open(&paths.db_file)?);
    let store = Store::open(&paths)?;
    store.read(|connection| {
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            connection.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))?,
            "wal"
        );
        let tables: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(tables, 9);
        assert!(connection.execute("DELETE FROM scopes", []).is_err());
        Ok(())
    })?;
    store.write(|connection| {
        assert_eq!(connection.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))?, 1);
        connection.execute("INSERT INTO scopes (id, kind, name, added_at) VALUES (1, 'global', 'Global', 0)", [])?;
        let defaults: (String, i64) = connection.query_row("SELECT mode, auto_sync FROM scopes", [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        assert_eq!(defaults, ("copy".into(), 1));
        for kind in ["skill", "mcp", "hook", "subagent"] {
            connection.execute("INSERT INTO project_items VALUES (1, ?1, 'same-name', NULL)", [kind])?;
        }
        for how in ["lock", "exclude"] {
            connection.execute("INSERT INTO ledger (scope_id, item_kind, item_name, target, how, written_hash, written_at) VALUES (1, 'skill', 'test', ?1, ?1, '', 0)", [how])?;
        }
        assert!(connection.execute("INSERT INTO project_items VALUES (999, 'skill', 'orphan', NULL)", []).is_err());
        Ok(())
    })?;
    drop(store);
    let reopened = Store::open(&paths)?;
    reopened.read(|connection| {
        assert_eq!(
            connection.query_row("SELECT count(*) FROM project_items", [], |row| row
                .get::<_, i64>(0))?,
            4
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM ledger", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        Ok(())
    })?;
    println!(
        "schema v1: nine tables, copy/auto_sync defaults, multi-kind items, lock/exclude, WAL, foreign keys; reopen preserves rows"
    );
    Ok(())
}

#[test]
fn readers_finish_while_writer_transaction_is_still_open() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let paths = Paths::resolve(Some(root.path()), None, None)?;
    let store = Arc::new(Store::open(&paths)?);
    let (started, writing) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let writer_store = Arc::clone(&store);
    let writer = thread::spawn(move || {
        writer_store.write(move |connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO agents (id, detected, enabled) VALUES ('test', 1, 1)",
                [],
            )?;
            started.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            transaction.commit()?;
            Ok(())
        })
    });
    writing.recv_timeout(Duration::from_secs(10)).unwrap();
    let (finished, results) = mpsc::channel();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            let finished = finished.clone();
            thread::spawn(move || {
                finished
                    .send(store.read(|connection| {
                        Ok(
                            connection.query_row("SELECT count(*) FROM agents", [], |row| {
                                row.get::<_, i64>(0)
                            })?,
                        )
                    }))
                    .unwrap();
            })
        })
        .collect();
    let observations: Vec<_> = (0..4)
        .map(|_| results.recv_timeout(Duration::from_secs(5)))
        .collect();
    release.send(()).unwrap();
    writer.join().unwrap()?;
    for reader in readers {
        reader.join().unwrap();
    }
    for observation in observations {
        assert_eq!(observation.unwrap()?, 0);
    }
    store.read(|connection| {
        assert_eq!(
            connection.query_row("SELECT count(*) FROM agents", [], |row| row
                .get::<_, i64>(0))?,
            1
        );
        Ok(())
    })?;
    println!(
        "four real reader threads completed during an uncommitted write; committed row visible afterward"
    );
    Ok(())
}

#[test]
fn deleted_database_is_recreated_on_next_open() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let paths = Paths::resolve(Some(&root.path().join("new-home")), None, None)?;
    drop(Store::open(&paths)?);
    std::fs::remove_file(&paths.db_file)?;
    assert!(!paths.db_file.exists());
    let store = Store::open(&paths)?;
    assert!(paths.db_file.is_file());
    store.read(|connection| {
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM items", [], |row| row.get::<_, i64>(0))?,
            0
        );
        Ok(())
    })?;
    println!("deleted state.db recreated with schema v1 on next open");
    Ok(())
}
