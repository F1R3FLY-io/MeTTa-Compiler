//! MeTTaTron Trace Analyzer
//!
//! Standalone tool for reading and analyzing binary evaluation trace files
//! produced by `mettatron --trace FILE`.
//!
//! Subcommands:
//! - `dump`          — Sequential event dump (human-readable or JSON)
//! - `stats`         — Summary statistics, histograms, hot expression ranking
//! - `search`        — Pattern-based event filtering
//! - `errors`        — Error/exception event listing with context chain
//! - `bailouts`      — JIT/bytecode bailout summary
//! - `timeline`      — Per-thread activity timeline with Gantt bars (v2)
//! - `parallel`      — Concurrency level analysis over time (v2)
//! - `bottlenecks`   — Serialization bottleneck identification (v2)
//! - `export-chrome` — Chrome Trace Format JSON export for Perfetto UI (v2)
//! - `lint`          — Diagnostic passes for anti-pattern detection (v2)

mod reader;
mod dump;
mod stats;
mod search;
mod errors;
mod bailouts;
mod timeline;
mod parallel;
mod bottlenecks;
mod export_chrome;
mod lint;

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
    /// Per-thread activity timeline with horizontal Gantt bars (requires v2 trace)
    Timeline {
        /// Path to the trace file
        file: String,
    },
    /// Concurrency level analysis over time (requires v2 trace)
    Parallel {
        /// Path to the trace file
        file: String,
        /// Time bucket width in microseconds (default: 100)
        #[arg(long, default_value = None)]
        bucket_us: Option<u64>,
    },
    /// Identify serialization bottlenecks — intervals of low concurrency (requires v2 trace)
    Bottlenecks {
        /// Path to the trace file
        file: String,
        /// Concurrency threshold: flag intervals with fewer than this many active threads (default: 2)
        #[arg(long)]
        concurrency_threshold: Option<usize>,
        /// Minimum duration in microseconds to report (default: 1000 = 1ms)
        #[arg(long)]
        duration_threshold_us: Option<u64>,
        /// Number of top bottlenecks to display (default: 20)
        #[arg(long)]
        top_n: Option<usize>,
    },
    /// Export trace as Chrome Trace Format JSON for Perfetto UI / chrome://tracing (requires v2 trace)
    ExportChrome {
        /// Path to the trace file
        file: String,
        /// Output JSON file path
        #[arg(short, long)]
        output: String,
    },
    /// Run lint-style diagnostic passes detecting anti-patterns and optimization opportunities (requires v2 trace)
    Lint {
        /// Path to the trace file
        file: String,
        /// Minimum severity to report: "info" or "warning"
        #[arg(long, default_value = "info")]
        severity: String,
        /// Run only specific lint(s), comma-separated (e.g., "gc-storm,tier-thrash")
        #[arg(long)]
        lint: Option<String>,
        /// Depth threshold for eval-depth-explosion lint
        #[arg(long, default_value = "100")]
        depth_threshold: u32,
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
        Commands::Timeline { file } => timeline::run(&file),
        Commands::Parallel { file, bucket_us } => parallel::run(&file, bucket_us),
        Commands::Bottlenecks { file, concurrency_threshold, duration_threshold_us, top_n } =>
            bottlenecks::run(&file, concurrency_threshold, duration_threshold_us, top_n),
        Commands::ExportChrome { file, output } => export_chrome::run(&file, &output),
        Commands::Lint { file, severity, lint, depth_threshold } =>
            lint::run(&file, &severity, lint.as_deref(), depth_threshold),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
