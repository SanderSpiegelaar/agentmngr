use std::path::PathBuf;

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
            // Detection does not opt an agent into management.
            let statuses = registry.detect(&path, &Default::default())?;
            println!("{:<14} {:<20} {:<10} ENABLED", "ID", "AGENT", "DETECTED");
            for status in statuses {
                println!(
                    "{:<14} {:<20} {:<10} {}",
                    status.id, status.name, status.detected, status.enabled
                );
            }
        }
        Some(Command::Doctor { reindex }) => {
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
