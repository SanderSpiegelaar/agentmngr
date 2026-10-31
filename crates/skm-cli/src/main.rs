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
    Doctor,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    if let Some(Command::Doctor) = cli.command {
        let env_home = std::env::var_os("SKILLSMGR_HOME").map(PathBuf::from);
        let user_home = std::env::home_dir();
        let paths = Paths::resolve(
            cli.home.as_deref(),
            env_home.as_deref(),
            user_home.as_deref(),
        )?;
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
    Ok(())
}
