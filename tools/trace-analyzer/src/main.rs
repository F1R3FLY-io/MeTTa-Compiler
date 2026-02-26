//! MeTTaTron Trace Analyzer
//!
//! Standalone tool for reading and analyzing binary evaluation trace files
//! produced by `mettatron --trace FILE`.
//!
//! Subcommands:
//! - `dump`     — Sequential event dump (human-readable or JSON)
//! - `stats`    — Summary statistics, histograms, hot expression ranking
//! - `search`   — Pattern-based event filtering
//! - `errors`   — Error/exception event listing with context chain
//! - `bailouts` — JIT/bytecode bailout summary

mod reader;
mod dump;
mod stats;
mod search;
mod errors;
mod bailouts;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "trace-analyzer")]
#[command(about = "Analysis tool for MeTTaTron evaluation trace files")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sequential dump of all events
    Dump {
        /// Path to the trace file
        file: String,
        /// Output as JSON instead of human-readable
        #[arg(long)]
        json: bool,
        /// Maximum number of events to display
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Summary statistics
    Stats {
        /// Path to the trace file
        file: String,
    },
    /// Search for events matching a pattern
    Search {
        /// Path to the trace file
        file: String,
        /// Pattern to search for (atom name, rule LHS, error kind, etc.)
        pattern: String,
    },
    /// List all error/exception events
    Errors {
        /// Path to the trace file
        file: String,
    },
    /// List all JIT/bytecode bailout events
    Bailouts {
        /// Path to the trace file
        file: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Dump { file, json, limit } => dump::run(&file, json, limit),
        Commands::Stats { file } => stats::run(&file),
        Commands::Search { file, pattern } => search::run(&file, &pattern),
        Commands::Errors { file } => errors::run(&file),
        Commands::Bailouts { file } => bailouts::run(&file),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
