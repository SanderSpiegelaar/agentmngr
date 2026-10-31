use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use skm_core::paths::Paths;

#[derive(Parser)]
#[command(name = "skm", version)]
struct Cli {
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print resolved paths.
    Doctor {
        #[arg(long)]
        reindex: bool,
    },
    /// List registered agents and their detection and management status.
    Agents,
}

fn main() -> Result<(), skm_core::Error> {
    let cli = Cli::parse();
    let env_home = std::env::var_os("SKILLSMGR_HOME").map(PathBuf::from);
    let user_home = std::env::home_dir();
    let paths = Paths::resolve(
        cli.home.as_deref(),
        env_home.as_deref(),
        user_home.as_deref(),
    )?;
    paths.ensure_layout()?;
    let _logging = skm_core::logging::init_logging(&paths.logs)?;
    match cli.command {
        Some(Command::Agents) => {
            let home = user_home.ok_or_else(|| std::io::Error::other("HOME is unavailable"))?;
            let config = skm_core::config::Config::load(&paths.config_file)?;
            let registry = skm_core::registry::Registry::load()?.resolve(
                &home,
                &std::env::vars_os()
                    .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
                    .collect(),
                &config,
            )?;
            let path = std::env::var_os("PATH").unwrap_or_default();
            let store = skm_core::store::Store::open(&paths)?;
            let enabled = store.read(|connection| {
                let mut query = connection.prepare("SELECT id FROM agents WHERE enabled = 1")?;
                Ok(query
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<BTreeSet<String>, _>>()?)
            })?;
            // Detection does not opt an agent into management.
            let statuses = registry.detect(&path, &enabled)?;
            let statuses = store.write(move |connection| {
                let transaction = connection.transaction()?;
                let detected_at = SystemTime::now().duration_since(UNIX_EPOCH)
                    .map_err(std::io::Error::other)?.as_secs() as i64;
                for status in &statuses {
                    transaction.execute(
                        "INSERT INTO agents (id, detected, enabled, detected_at) VALUES (?1, ?2, 0, ?3)
                         ON CONFLICT(id) DO UPDATE SET detected=excluded.detected, detected_at=excluded.detected_at",
                        (&status.id, status.detected, detected_at),
                    )?;
                }
                transaction.commit()?;
                Ok(statuses)
            })?;
            println!("{:<14} {:<20} {:<10} ENABLED", "ID", "AGENT", "DETECTED");
            for status in statuses {
                println!(
                    "{:<14} {:<20} {:<10} {}",
                    status.id, status.name, status.detected, status.enabled
                );
            }
        }
        Some(Command::Doctor { reindex }) => {
            let registry = skm_core::registry::Registry::load()?;
            let known_agents: Vec<_> = registry.agents.keys().map(String::as_str).collect();
            match skm_core::library::Library::load(
                &paths.library.join("library.toml"),
                &known_agents,
            ) {
                Ok(_) => {}
                Err(skm_core::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                }
                Err(error) => return Err(error),
            }
            if reindex {
                let store = skm_core::store::Store::open(&paths)?;
                let counts = skm_core::indexer::full_reindex(&store, &paths)?;
                println!(
                    "reindex: {} total, {} inserted, {} updated, {} removed, {} unchanged",
                    counts.total, counts.inserted, counts.updated, counts.removed, counts.unchanged
                );
            }
            for (name, path) in [
                ("home", &paths.home),
                ("library", &paths.library),
                ("cache", &paths.cache),
                ("trash", &paths.trash),
                ("logs", &paths.logs),
                ("config", &paths.config_file),
                ("db", &paths.db_file),
                ("lock", &paths.lock_file),
            ] {
                println!("{name}: {}", path.display());
            }
        }
        None => {}
    }
    Ok(())
}
