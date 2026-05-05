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
//! - `hotpath`       — Expression-level profiling by head symbol
//! - `redundancy`    — Memoization opportunity detection
//! - `fanout`        — Nondeterministic branching deep-dive
//! - `critical-path` — Amdahl's Law analysis via critical path reconstruction
//! - `perf-correlate` — Cross-validate trace with perf CPU profile (folded stacks)
//! - `massif-correlate` — Cross-validate trace with Valgrind massif memory profile
//! - `gdb-correlate` — Cross-correlate GDB coredump backtrace with evaluation trace

mod reader;
mod util;
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
mod workpool;
mod hotpath;
mod redundancy;
mod fanout;
mod critical_path;
mod function_map;
mod perf_parser;
mod massif_parser;
mod perf_correlate;
mod massif_correlate;
mod gdb_parser;
mod gdb_correlate;
mod bindings;
mod closure;

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
    /// Comprehensive WorkPool scaling analysis report
    Workpool {
        /// Path to the trace file
        file: String,
    },
    /// Expression-level profiling grouped by head symbol (like `perf report`)
    Hotpath {
        /// Path to the trace file
        file: String,
        /// Number of top entries to display (default: 30)
        #[arg(long, default_value = "30")]
        top_n: usize,
        /// Sort metric: "self", "inclusive", "count", or "p95"
        #[arg(long, default_value = "self")]
        sort_by: String,
    },
    /// Detect memoization opportunities (repeated identical computations)
    Redundancy {
        /// Path to the trace file
        file: String,
        /// Number of top entries to display (default: 30)
        #[arg(long, default_value = "30")]
        top_n: usize,
        /// Minimum repetition count to report (default: 3)
        #[arg(long, default_value = "3")]
        min_count: u64,
    },
    /// Nondeterministic branching deep-dive (fork/branch analysis)
    Fanout {
        /// Path to the trace file
        file: String,
        /// Number of top entries to display (default: 20)
        #[arg(long, default_value = "20")]
        top_n: usize,
    },
    /// Amdahl's Law analysis via critical path reconstruction
    CriticalPath {
        /// Path to the trace file
        file: String,
        /// Worker counts for speedup prediction (comma-separated, default: "1,2,4,8,16,32")
        #[arg(long, default_value = "1,2,4,8,16,32")]
        workers: String,
    },
    /// Cross-validate MeTTa trace with perf CPU profile (folded stacks)
    PerfCorrelate {
        /// Path to the trace file
        file: String,
        /// Path to folded perf stacks (output of `perf script | stackcollapse-perf.pl`)
        #[arg(long)]
        perf_stacks: String,
        /// Number of top entries to display (default: 20)
        #[arg(long, default_value = "20")]
        top_n: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Cross-validate MeTTa trace with Valgrind massif memory profile
    MassifCorrelate {
        /// Path to the trace file
        file: String,
        /// Path to massif output file (from `valgrind --tool=massif`)
        #[arg(long)]
        massif_out: String,
        /// Number of top entries to display (default: 20)
        #[arg(long, default_value = "20")]
        top_n: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Cross-correlate GDB coredump backtrace with MeTTa evaluation trace.
    ///
    /// Extract backtrace from a coredump:
    ///   coredumpctl debug BINARY --debugger-arguments="-batch -ex 'thread apply all bt'" > bt.txt
    ///
    /// Or from a core file directly:
    ///   gdb -batch -ex 'thread apply all bt' ./target/release/mettatron /path/to/core > bt.txt
    GdbCorrelate {
        /// Path to the trace file (.mtrace)
        file: String,
        /// Path to GDB backtrace file (output of `thread apply all bt`)
        #[arg(long, long_help = "Path to GDB backtrace file.\n\n\
            Generate with coredumpctl:\n  \
            coredumpctl debug BINARY --debugger-arguments=\"-batch -ex 'thread apply all bt'\" > bt.txt\n\n\
            Or from a core file:\n  \
            gdb -batch -ex 'thread apply all bt' ./target/release/mettatron /path/to/core > bt.txt")]
        gdb_bt: String,
        /// Number of tail events to keep per trace thread (default: 50)
        #[arg(long, default_value = "50")]
        tail_n: usize,
        /// Number of top entries to display (default: 20)
        #[arg(long, default_value = "20")]
        top_n: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Per-BoundValue binding-flow analysis (v5 events).
    ///
    /// Reads `ContinuationEnter` / `ContinuationEmit` / `BindingsDropped`
    /// events emitted by the trampoline's dispatcher-level binding-flow
    /// instrumentation (eval-trace feature). Surfaces where per-branch
    /// bindings are preserved, composed, or silently dropped between
    /// continuation handlers.
    Bindings {
        /// Path to the trace file (.mtrace)
        file: String,
        /// Only show BindingsDropped events and a per-(cont_kind, site) summary
        #[arg(long)]
        drops_only: bool,
        /// Comma-separated variable names to filter on (e.g. "$who,$x")
        #[arg(long)]
        var: Option<String>,
        /// Comma-separated continuation kinds to include (e.g. "ProcessEvalEval,ProcessChainExpr")
        #[arg(long)]
        cont: Option<String>,
        /// Maximum number of flows to display
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Built-in dependency closure: distinct head symbols invoked during
    /// evaluation, categorized by dispatch kind (kernel-op / special-form /
    /// pseudo-event / operator), with per-parent call-graph for spec auditing.
    Closure {
        /// Path to the trace file (.mtrace)
        file: String,
        /// Maximum number of entries to display
        #[arg(long, default_value_t = 200)]
        top_n: usize,
        /// Render the per-parent call graph
        #[arg(long)]
        show_graph: bool,
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
        Commands::Workpool { file } => workpool::run(&file),
        Commands::Hotpath { file, top_n, sort_by } =>
            hotpath::run(&file, top_n, &sort_by),
        Commands::Redundancy { file, top_n, min_count } =>
            redundancy::run(&file, top_n, min_count),
        Commands::Fanout { file, top_n } =>
            fanout::run(&file, top_n),
        Commands::CriticalPath { file, workers } => {
            let worker_counts: Vec<usize> = workers
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if worker_counts.is_empty() {
                Err("Invalid --workers: expected comma-separated integers".to_string())
            } else {
                critical_path::run(&file, &worker_counts)
            }
        }
        Commands::PerfCorrelate { file, perf_stacks, top_n, json } =>
            perf_correlate::run(&file, &perf_stacks, top_n, json),
        Commands::MassifCorrelate { file, massif_out, top_n, json } =>
            massif_correlate::run(&file, &massif_out, top_n, json),
        Commands::GdbCorrelate { file, gdb_bt, tail_n, top_n, json } =>
            gdb_correlate::run(&file, &gdb_bt, tail_n, top_n, json),
        Commands::Bindings { file, drops_only, var, cont, limit } => {
            let var_filter: Option<Vec<String>> = var.map(|s| {
                s.split(',').map(|v| v.trim().to_string()).collect()
            });
            let cont_filter: Option<Vec<String>> = cont.map(|s| {
                s.split(',').map(|c| c.trim().to_string()).collect()
            });
            bindings::run(&file, drops_only, var_filter, cont_filter, limit)
        }
        Commands::Closure { file, top_n, show_graph } =>
            closure::run(&file, top_n, show_graph),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
