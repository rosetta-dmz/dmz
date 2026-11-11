//! # DMZ CLI
//!
//! Command-line interface and dispatch orchestrator

pub mod cli;
pub mod commands;

pub use cli::{Cli, Commands};
pub use commands::dispatch;
