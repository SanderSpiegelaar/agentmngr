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
    #[arg(long, global = true)]
    dry_run: bool,
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
    /// Undo a recent operation, or list the last 50 operations.
    Undo {
        #[arg(conflicts_with = "list")]
        op_id: Option<i64>,
        #[arg(long)]
        list: bool,
    },
    /// Enable a skill globally.
    On { skill: String },
    /// Disable a skill globally.
    Off { skill: String },
}

fn main() -> Result<(), skm_core::Error> {
    let cli = Cli::parse();
    let env_home = std::env::var_os("SKILLSMGR_HOME").map(PathBuf::from);
    let user_home = std::env::home_dir();
    let mut paths = Paths::resolve(
        cli.home.as_deref(),
        env_home.as_deref(),
        user_home.as_deref(),
    )?;
    paths.ensure_layout()?;
    let _logging = skm_core::logging::init_logging(&paths.logs)?;
    match cli.command {
        Some(Command::Undo { op_id, list }) => {
            let store = skm_core::store::Store::open(&paths)?;
            if list {
                println!("ID\tAT\tACTOR\tRESULT\tSUMMARY");
                for op in skm_core::engine::list_ops(&store)? {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        op.id, op.at, op.actor, op.result, op.summary
                    );
                }
            } else {
                let result = match skm_core::engine::undo(&paths, &store, op_id, "cli", None) {
                    Ok(result) => result,
                    Err(error) => {
                        eprintln!("{}: {}", error.code(), error);
                        std::process::exit(1);
                    }
                };
                println!("op {}: {} undone", result.op_id, result.done.len());
                for issue in result.issues.iter().chain(&result.failed) {
                    eprintln!(
                        "{}: {}: {}",
                        issue.code,
                        issue.target.display(),
                        issue.message
                    );
                }
                if !result.issues.is_empty() || !result.failed.is_empty() {
                    std::process::exit(3);
                }
            }
        }
        Some(Command::On { ref skill }) | Some(Command::Off { ref skill }) => {
            let on = matches!(cli.command, Some(Command::On { .. }));
            let home = user_home.ok_or_else(|| std::io::Error::other("HOME is unavailable"))?;
            let store = skm_core::store::Store::open(&paths)?;
            let issues = if cli.dry_run {
                let plan =
                    skm_core::global_skills::plan_global_skill(&paths, &store, &home, skill, on)?;
                println!("{}", plan.summary);
                for action in &plan.actions {
                    println!("{:?}", action.action);
                }
                plan.issues
            } else {
                let result =
                    skm_core::global_skills::set_global_skill(&paths, &store, &home, skill, on)?;
                println!(
                    "{} actions, {} issues, {} failed",
                    result.done.len(),
                    result.issues.len(),
                    result.failed.len()
                );
                result.issues.into_iter().chain(result.failed).collect()
            };
            for issue in &issues {
                eprintln!(
                    "{}: {}: {}",
                    issue.code,
                    issue.target.display(),
                    issue.message
                );
            }
            if !issues.is_empty() {
                std::process::exit(3);
            }
        }
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
