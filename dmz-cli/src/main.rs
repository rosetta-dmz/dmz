mod cli;
mod commands;

use clap::Parser;
use cli::Cli;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn main() {
    let cli = Cli::parse();

    // Initialize logging output subscriber
    let log_level = if cli.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    let subscriber = FmtSubscriber::builder().with_max_level(log_level).finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    // Dispatch parsed CLI command
    commands::dispatch(cli.command);
}
