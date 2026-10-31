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
    if let Some(Command::Doctor) = cli.command {
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
