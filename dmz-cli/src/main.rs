use clap::Parser;
use dmz_cli::cli::Cli;
use dmz_cli::commands;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Initialize logging subscriber respecting RUST_LOG with fallback based on --verbose
    let default_level = if cli.verbose { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    // Dispatch parsed CLI command and translate Result into safe POSIX ExitCode
    match commands::dispatch(cli.command) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => {
            // Guard against truncation wrapping (e.g. 256 as u8 == 0) and negative values
            let exit_u8 = u8::try_from(code).unwrap_or(1);
            ExitCode::from(if exit_u8 == 0 { 1 } else { exit_u8 })
        }
        Err(err) => {
            eprintln!("\x1b[31merror:\x1b[0m {err:#}");
            ExitCode::FAILURE
        }
    }
}
