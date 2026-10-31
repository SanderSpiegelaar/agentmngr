use clap::Parser;

#[derive(Parser)]
#[command(name = "skm", version)]
struct Cli {}

fn main() {
    Cli::parse();
}
