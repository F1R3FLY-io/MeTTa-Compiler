//! End-to-end throughput benchmarks for MeTTaTron
//!
//! Measures programs/second throughput in sequential, parallel, and async modes.
//! Use CLI args to configure test duration and which samples to run.
//!
//! ## Active Benchmarking (Brendan Gregg)
//!
//! This benchmark follows active benchmarking principles:
//! - Configure benchmarks to run for long duration in steady state (use --duration)
//! - While running, analyze performance using other tools to identify true limiters
//! - Confirm the benchmark tests what you intend and understand what that is
//! - Identify bottlenecks: CPU, memory, I/O, network, software resources
//!
//! Common pitfalls to watch for:
//! - Benchmark limited by single-threaded client (not the system under test)
//! - Throttled by resource controls, network, or neighbors
//! - Testing wrong target (e.g., disk I/O instead of filesystem I/O)
//! - Unrealistic workload patterns
//!
//! ## CLI Usage
//!
//! ```bash
//! # Run with defaults (30s, knowledge_graph only)
//! cargo bench --bench e2e_throughput_configurable
//!
//! # Run for longer duration (recommended: hours for production analysis)
//! cargo bench --bench e2e_throughput_configurable -- --duration 300
//!
//! # Run specific samples
//! cargo bench --bench e2e_throughput_configurable -- --samples fib,pattern_matching_stress
//!
//! # Run multiple samples
//! cargo bench --bench e2e_throughput_configurable -- --duration 60 --samples fib,pattern_matching_stress,metta_programming_stress
//!
//! # Run selected modes only
//! cargo bench --bench e2e_throughput_configurable -- --modes sequential,parallel --parallel-workers 1,4,all
//!
//! # Run a profiler-friendly single-mode sample with samply
//! cargo bench --no-run --bench e2e_throughput_configurable
//! samply record target/release/deps/e2e_throughput_configurable-* -- --duration 10 --warmup 0 --samples knowledge_graph --modes sequential
//!
//! # Show help
//! cargo bench --bench e2e_throughput_configurable -- --help
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use mettatron::config::{EvalConfig, configure_eval, get_eval_config};
use mettatron::{MettaState, compile, new_env, run_state};

const SAMPLES: &[(&str, &str)] = &[
    (
        "backward_chaining",
        include_str!("../metta_samples/backward_chaining.metta"),
    ),
    (
        "cartesian_product_stress",
        include_str!("../metta_samples/cartesian_product_stress.metta"),
    ),
    (
        "concurrent_space_operations",
        include_str!("../metta_samples/concurrent_space_operations.metta"),
    ),
    (
        "constraint_search_simple",
        include_str!("../metta_samples/constraint_search_simple.metta"),
    ),
    ("fib", include_str!("../metta_samples/fib.metta")),
    (
        "grounded_tco_stress",
        include_str!("../metta_samples/grounded_tco_stress.metta"),
    ),
    (
        "knowledge_graph",
        include_str!("../metta_samples/knowledge_graph.metta"),
    ),
    (
        "lazy_eager_comparison",
        include_str!("../metta_samples/lazy_eager_comparison.metta"),
    ),
    (
        "metta_programming_stress",
        include_str!("../metta_samples/metta_programming_stress.metta"),
    ),
    (
        "multi_space_reasoning",
        include_str!("../metta_samples/multi_space_reasoning.metta"),
    ),
    (
        "pattern_matching_stress",
        include_str!("../metta_samples/pattern_matching_stress.metta"),
    ),
    (
        "tco_deep_recursion",
        include_str!("../metta_samples/tco_deep_recursion.metta"),
    ),
    (
        "trampoline_stress",
        include_str!("../metta_samples/trampoline_stress.metta"),
    ),
    (
        "type_heavy_program",
        include_str!("../metta_samples/type_heavy_program.metta"),
    ),
];

#[derive(Parser, Debug)]
#[command(name = "e2e_throughput_configurable")]
#[command(about = "MeTTaTron throughput benchmarks", long_about = None)]
#[command(disable_help_flag = false)]
#[command(trailing_var_arg = true)]
#[command(allow_external_subcommands = true)]
struct Args {
    /// Test duration in seconds for each benchmark mode
    #[arg(short, long, default_value_t = 30)]
    duration: u64,

    /// Warm-up duration in seconds before each measurement
    #[arg(short, long, default_value_t = 5)]
    warmup: u64,

    /// List of sample names to benchmark
    /// Use --list-samples to print the current catalog.
    #[arg(short, long, value_delimiter = ',', default_value = "knowledge_graph")]
    samples: Vec<String>,

    /// Benchmark modes to run: sequential, parallel, async
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "sequential,parallel,async"
    )]
    modes: Vec<String>,

    /// Worker counts for parallel mode. Use "all" for available CPU count.
    #[arg(long, value_delimiter = ',', default_value = "4,all")]
    parallel_workers: Vec<String>,

    /// Async task concurrency. Defaults to available CPU count when omitted or 0.
    #[arg(long, default_value_t = 0)]
    async_concurrency: usize,

    /// List available samples and exit.
    #[arg(long)]
    list_samples: bool,

    /// Ignored trailing args from cargo bench
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    #[arg(hide = true)]
    trailing: Vec<String>,
}

#[derive(Debug)]
struct ThroughputReport {
    sample_name: String,
    mode: String,
    programs_per_second: f64,
    total_programs: u64,
    total_errors: u64,
    error_rate_percent: f64,
    average_latency_ms: f64,
    test_duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchmarkMode {
    Sequential,
    Parallel,
    Async,
}

fn parse_modes(names: &[String]) -> Result<Vec<BenchmarkMode>, String> {
    let mut modes = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mode = match name {
            "sequential" => BenchmarkMode::Sequential,
            "parallel" => BenchmarkMode::Parallel,
            "async" => BenchmarkMode::Async,
            other => {
                return Err(format!(
                    "unknown mode '{other}'. Available modes: sequential, parallel, async"
                ));
            }
        };
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    }
    if modes.is_empty() {
        return Err("at least one benchmark mode is required".to_string());
    }
    Ok(modes)
}

fn parse_parallel_workers(specs: &[String], num_cpus: usize) -> Result<Vec<usize>, String> {
    let mut workers = Vec::new();
    for spec in specs {
        let spec = spec.trim();
        if spec.is_empty() {
            continue;
        }
        let count = if spec == "all" {
            num_cpus
        } else {
            spec.parse::<usize>()
                .map_err(|_| format!("invalid parallel worker count '{spec}'"))?
        };
        if count == 0 {
            return Err("parallel worker count must be greater than zero".to_string());
        }
        if !workers.contains(&count) {
            workers.push(count);
        }
    }
    if workers.is_empty() {
        return Err("at least one parallel worker count is required".to_string());
    }
    Ok(workers)
}

fn evaluate_full_program(source: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let program = compile(source)?;
    let env = new_env();
    let _result = run_state(MettaState::from_env(env), &program)?;
    Ok(())
}

async fn evaluate_full_program_async(
    source: &'static str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tokio::task::spawn_blocking(move || evaluate_full_program(source))
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
}

// ============================================================================
// Warm-up Functions
// ============================================================================
// Warm-up is critical for accurate benchmarking because:
// 1. CPU frequency scaling: CPUs start at base frequency and turbo boost under load
// 2. Cache warming: Instruction cache, data cache, and TLB need to be populated
// 3. Branch predictor training: CPU needs time to learn branch patterns
// 4. Thread pool initialization: Worker threads need to be spawned and ready

fn warmup_sequential(source: &str, duration: Duration) {
    println!("  Warming up for {}s...", duration.as_secs());
    let start = Instant::now();
    while start.elapsed() < duration {
        let _ = evaluate_full_program(source);
    }
}

fn warmup_parallel(source: &str, duration: Duration, num_workers: usize) {
    println!(
        "  Warming up parallel-{} for {}s...",
        num_workers,
        duration.as_secs()
    );
    let start = Instant::now();
    thread::scope(|s| {
        for _ in 0..num_workers {
            s.spawn(|| {
                while start.elapsed() < duration {
                    let _ = evaluate_full_program(source);
                }
            });
        }
    });
}

fn warmup_async(source: &'static str, duration: Duration, concurrency: usize) {
    let config = get_eval_config();
    println!(
        "  Warming up async-{} for {}s...",
        concurrency,
        duration.as_secs()
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(config.max_blocking_threads)
        .enable_all()
        .build()
        .unwrap();

    let start = Instant::now();
    rt.block_on(async {
        let mut handles = Vec::with_capacity(concurrency);
        for _ in 0..concurrency {
            let handle = tokio::spawn(async move {
                while start.elapsed() < duration {
                    let _ = evaluate_full_program_async(source).await;
                }
            });
            handles.push(handle);
        }
        for handle in handles {
            let _ = handle.await;
        }
    });
}

fn measure_sequential(sample_name: &str, source: &str, duration: Duration) -> ThroughputReport {
    println!(
        "  [sequential] Starting throughput test for '{}' ({}s)",
        sample_name,
        duration.as_secs()
    );

    let start = Instant::now();
    let mut completed = 0u64;
    let mut errors = 0u64;
    let mut total_latency = Duration::ZERO;

    while start.elapsed() < duration {
        let iter_start = Instant::now();
        match evaluate_full_program(source) {
            Ok(_) => {
                completed += 1;
                total_latency += iter_start.elapsed();
            }
            Err(e) => {
                errors += 1;
                eprintln!("Evaluation error: {}", e);
            }
        }
    }

    let actual_duration = start.elapsed();
    build_report(
        sample_name,
        "sequential",
        completed,
        errors,
        Some(total_latency),
        actual_duration,
    )
}

fn measure_parallel(
    sample_name: &str,
    source: &str,
    duration: Duration,
    num_workers: usize,
) -> ThroughputReport {
    println!(
        "  [parallel-{}] Starting throughput test for '{}' ({}s)",
        num_workers,
        sample_name,
        duration.as_secs()
    );

    let completed = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    thread::scope(|s| {
        for worker_id in 0..num_workers {
            let completed = Arc::clone(&completed);
            let errors = Arc::clone(&errors);

            s.spawn(move || {
                let mut local_completed = 0u64;
                let mut local_errors = 0u64;

                while start.elapsed() < duration {
                    match evaluate_full_program(source) {
                        Ok(_) => local_completed += 1,
                        Err(e) => {
                            local_errors += 1;
                            if local_errors <= 3 {
                                eprintln!("Worker {} error: {}", worker_id, e);
                            }
                        }
                    }
                }

                completed.fetch_add(local_completed, Ordering::Relaxed);
                errors.fetch_add(local_errors, Ordering::Relaxed);
            });
        }
    });

    let actual_duration = start.elapsed();
    build_report(
        sample_name,
        &format!("parallel-{}", num_workers),
        completed.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed),
        None,
        actual_duration,
    )
}

fn measure_async(
    sample_name: &str,
    source: &'static str,
    duration: Duration,
    concurrency: usize,
) -> ThroughputReport {
    let config = get_eval_config();

    println!(
        "  [async-{}] Starting throughput test for '{}' ({}s, max_blocking={})",
        concurrency,
        sample_name,
        duration.as_secs(),
        config.max_blocking_threads
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(config.max_blocking_threads)
        .enable_all()
        .build()
        .unwrap();

    let completed = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));

    let start = Instant::now();

    rt.block_on(async {
        let mut handles = Vec::with_capacity(concurrency);

        for task_id in 0..concurrency {
            let completed = Arc::clone(&completed);
            let errors = Arc::clone(&errors);

            let handle = tokio::spawn(async move {
                let mut local_completed = 0u64;
                let mut local_errors = 0u64;

                while start.elapsed() < duration {
                    match evaluate_full_program_async(source).await {
                        Ok(_) => local_completed += 1,
                        Err(e) => {
                            local_errors += 1;
                            if local_errors <= 3 {
                                eprintln!("Task {} error: {}", task_id, e);
                            }
                        }
                    }
                    tokio::task::yield_now().await;
                }

                completed.fetch_add(local_completed, Ordering::Relaxed);
                errors.fetch_add(local_errors, Ordering::Relaxed);
            });

            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }
    });

    let actual_duration = start.elapsed();
    build_report(
        sample_name,
        &format!("async-{}", concurrency),
        completed.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed),
        None,
        actual_duration,
    )
}

fn build_report(
    sample_name: &str,
    mode: &str,
    completed: u64,
    errors: u64,
    total_latency: Option<Duration>,
    actual_duration: Duration,
) -> ThroughputReport {
    let programs_per_second = completed as f64 / actual_duration.as_secs_f64();
    let error_rate = if completed + errors > 0 {
        errors as f64 / (completed + errors) as f64 * 100.0
    } else {
        0.0
    };
    let avg_latency = match total_latency {
        Some(lat) if completed > 0 => lat.as_secs_f64() / completed as f64 * 1000.0,
        _ => 0.0,
    };

    ThroughputReport {
        sample_name: sample_name.to_string(),
        mode: mode.to_string(),
        programs_per_second,
        total_programs: completed,
        total_errors: errors,
        error_rate_percent: error_rate,
        average_latency_ms: avg_latency,
        test_duration: actual_duration,
    }
}

fn print_report(report: &ThroughputReport) {
    println!("\n--- {} [{}] ---", report.sample_name, report.mode);
    println!("Duration: {:.1}s", report.test_duration.as_secs_f64());
    println!("Programs completed: {}", report.total_programs);
    println!("Throughput: {:.2} programs/sec", report.programs_per_second);
    if report.average_latency_ms > 0.0 {
        println!("Average latency: {:.2}ms", report.average_latency_ms);
    }
    println!(
        "Errors: {} ({:.1}%)",
        report.total_errors, report.error_rate_percent
    );
}

fn main() {
    let args = Args::parse();

    if args.list_samples {
        for (name, _) in SAMPLES {
            println!("{}", name);
        }
        return;
    }

    configure_eval(EvalConfig::cpu_optimized());

    let config = get_eval_config();
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let test_duration = Duration::from_secs(args.duration);
    let warmup_duration = Duration::from_secs(args.warmup);
    let modes = parse_modes(&args.modes).unwrap_or_else(|err| {
        eprintln!("Error: {err}");
        std::process::exit(2);
    });
    let parallel_workers =
        parse_parallel_workers(&args.parallel_workers, num_cpus).unwrap_or_else(|err| {
            eprintln!("Error: {err}");
            std::process::exit(2);
        });
    let async_concurrency = if args.async_concurrency == 0 {
        num_cpus
    } else {
        args.async_concurrency
    };

    // Filter samples based on CLI args
    let samples_to_run: Vec<_> = SAMPLES
        .iter()
        .filter(|(name, _)| args.samples.contains(&name.to_string()))
        .collect();

    if samples_to_run.is_empty() {
        eprintln!("Error: No valid samples selected. Available samples:");
        for (name, _) in SAMPLES {
            eprintln!("  - {}", name);
        }
        std::process::exit(1);
    }

    println!("=== MeTTaTron Throughput Benchmarks ===");
    println!("CPUs: {}", num_cpus);
    println!(
        "EvalConfig: max_blocking_threads={}, batch_size_hint={}",
        config.max_blocking_threads, config.batch_size_hint
    );
    println!("Test duration per mode: {}s", test_duration.as_secs());
    println!("Warm-up duration per mode: {}s", warmup_duration.as_secs());
    println!(
        "Modes: {}",
        modes
            .iter()
            .map(|mode| match mode {
                BenchmarkMode::Sequential => "sequential",
                BenchmarkMode::Parallel => "parallel",
                BenchmarkMode::Async => "async",
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
    if modes.contains(&BenchmarkMode::Parallel) {
        println!(
            "Parallel workers: {}",
            parallel_workers
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if modes.contains(&BenchmarkMode::Async) {
        println!("Async concurrency: {}", async_concurrency);
    }
    println!(
        "Samples: {}",
        samples_to_run
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut reports = Vec::new();

    for (name, source) in samples_to_run {
        println!("\n==== ==== ==== Benchmarking: {} ==== ==== ====", name);

        // Sequential mode
        if modes.contains(&BenchmarkMode::Sequential) {
            warmup_sequential(source, warmup_duration);
            let seq = measure_sequential(name, source, test_duration);
            print_report(&seq);
            reports.push(seq);
        }

        if modes.contains(&BenchmarkMode::Parallel) {
            for workers in &parallel_workers {
                warmup_parallel(source, warmup_duration, *workers);
                let report = measure_parallel(name, source, test_duration, *workers);
                print_report(&report);
                reports.push(report);
            }
        }

        if modes.contains(&BenchmarkMode::Async) {
            warmup_async(source, warmup_duration, async_concurrency);
            let async_report = measure_async(name, source, test_duration, async_concurrency);
            print_report(&async_report);
            reports.push(async_report);
        }
    }
}
