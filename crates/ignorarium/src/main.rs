use clap::Parser;

#[derive(Parser)]
#[command(version, about = "Checks experiment plans before they run.")]
struct Cli {}

fn main() {
    Cli::parse();
}
