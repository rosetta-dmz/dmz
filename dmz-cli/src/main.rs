use clap::Parser;
use dmz_cli::cli::Cli;
use dmz_cli::commands;
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
    let _ = tracing::subscriber::set_global_default(subscriber);

    // Dispatch parsed CLI command
    commands::dispatch(cli.command);
}
