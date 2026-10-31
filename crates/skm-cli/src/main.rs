mod args;
mod commands;
mod output;
mod project;
mod sets;

use clap::Parser;
use skm_core::{Error, library::Library, paths::Paths, registry::Registry, store::Store};
use std::path::PathBuf;

use args::Cli;

struct Context {
    cli: Cli,
    paths: Paths,
    home: PathBuf,
    store: Store,
    registry: Registry,
    report: std::cell::RefCell<Option<serde_json::Value>>,
}

impl Context {
    fn library(&self) -> Result<Library, Error> {
        match Library::load(
            &self.paths.library.join("library.toml"),
            &self.known_agents(),
        ) {
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Library::default())
            }
            result => result,
        }
    }

    fn known_agents(&self) -> Vec<&str> {
        self.registry.agents.keys().map(String::as_str).collect()
    }
}

fn run(cli: Cli) -> Result<i32, Error> {
    let env_home = std::env::var_os("SKILLSMGR_HOME").map(PathBuf::from);
    let home = std::env::home_dir().ok_or_else(|| std::io::Error::other("HOME is unavailable"))?;
    let mut paths = Paths::resolve(cli.home.as_deref(), env_home.as_deref(), Some(&home))?;
    paths.ensure_layout()?;
    let _logging = skm_core::logging::init_logging(&paths.logs)?;
    let registry = Registry::load()?;
    if let Some(args::Command::Doctor { reindex }) = cli.command {
        return commands::doctor(&cli, &paths, &registry, reindex);
    }
    let store = Store::open(&paths)?;
    let context = Context {
        cli,
        paths,
        home,
        store,
        registry,
        report: Default::default(),
    };
    let result = commands::run(&context);
    if result.is_ok()
        && let Some(value) = context.report.take()
    {
        println!("{value}");
    }
    result
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            if code != 0 && std::env::args().any(|arg| arg == "--json") {
                println!(
                    "{}",
                    serde_json::json!({"ok":false,"code":"E_USAGE","message":error.to_string(),"fix":"Run skm --help."})
                );
            } else {
                let _ = error.print();
            }
            std::process::exit(code);
        }
    };
    let json = cli.json;
    let code = match run(cli) {
        Ok(code) => code,
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"ok":false,"code":error.code(),"message":error.to_string(),"fix":error.hint()})
                );
            } else {
                eprintln!("{}: {}\n{}", error.code(), error, error.hint());
            }
            match error {
                Error::ForeignTarget(_) | Error::NameCollision(_) | Error::ProjectEdited(_) => 3,
                _ => 1,
            }
        }
    };
    std::process::exit(code);
}
