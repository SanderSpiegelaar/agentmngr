use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "skm", version)]
pub struct Cli {
    #[arg(long, global = true)]
    pub home: Option<PathBuf>,
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true)]
    pub dry_run: bool,
    #[arg(long, global = true)]
    pub yes: bool,
    #[arg(short = 'q', global = true, conflicts_with = "verbose")]
    pub quiet: bool,
    #[arg(short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Kind {
    Skill,
    Mcp,
    Hook,
    Subagent,
    Plugin,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Mcp => "mcp",
            Self::Hook => "hook",
            Self::Subagent => "subagent",
            Self::Plugin => "plugin",
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    Status,
    List {
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        foreign: bool,
    },
    Show {
        item: String,
    },
    Add {
        source: String,
        #[arg(long)]
        skill: Vec<String>,
        #[arg(long, value_enum, default_value = "skill")]
        kind: Kind,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        on: Option<String>,
        #[arg(long = "set")]
        set: Option<String>,
    },
    Search {
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Remove {
        item: String,
        #[arg(long)]
        off: bool,
    },
    On {
        item: String,
        #[arg(long, default_value = "global")]
        scope: String,
        #[arg(long)]
        agent: Vec<String>,
    },
    Off {
        item: String,
        #[arg(long, default_value = "global")]
        scope: String,
        #[arg(long)]
        agent: Vec<String>,
    },
    Foreign {
        #[arg(long)]
        project: Option<PathBuf>,
    },
    Adopt {
        #[arg(required_unless_present = "all", num_args = 1.., conflicts_with = "all")]
        names: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, conflicts_with = "scope")]
        project: Option<PathBuf>,
    },
    Set {
        #[command(subcommand)]
        command: SetCommand,
    },
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    Agents {
        #[arg(value_parser = ["detect"])]
        action: Option<String>,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    Undo {
        #[arg(conflicts_with = "list")]
        op_id: Option<i64>,
        #[arg(long)]
        list: bool,
    },
    Doctor {
        #[arg(long)]
        bundle: Option<PathBuf>,
        #[arg(long)]
        reindex: bool,
    },
}

#[derive(Subcommand)]
pub enum AgentCommand {
    Enable { id: String },
    Disable { id: String },
}

#[derive(Subcommand)]
pub enum SetCommand {
    List,
    Show {
        name: String,
    },
    Create {
        name: String,
    },
    Delete {
        name: String,
    },
    Add {
        name: String,
        #[arg(required = true, num_args = 1..)]
        items: Vec<String>,
        #[arg(long)]
        agent: Vec<String>,
    },
    Rm {
        name: String,
        #[arg(required = true, num_args = 1..)]
        items: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum ProjectCommand {
    List,
    Add {
        path: PathBuf,
    },
    Remove {
        path: PathBuf,
        #[arg(long)]
        clean: bool,
    },
    Use {
        path: PathBuf,
        #[arg(required = true, num_args = 1..)]
        sets: Vec<String>,
    },
    Unuse {
        path: PathBuf,
        #[arg(required = true, num_args = 1..)]
        sets: Vec<String>,
    },
    Mode {
        path: PathBuf,
        #[arg(value_parser = ["copy", "link"])]
        mode: String,
    },
    Pause {
        path: PathBuf,
    },
    Resume {
        path: PathBuf,
    },
}
