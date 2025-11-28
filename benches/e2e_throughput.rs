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
//! cargo bench --bench e2e_throughput
//!
//! # Run for longer duration (recommended: hours for production analysis)
//! cargo bench --bench e2e_throughput -- --duration 300
//!
//! # Run specific samples
//! cargo bench --bench e2e_throughput -- --samples fib,pattern_matching_stress
//!
//! # Run multiple samples
//! cargo bench --bench e2e_throughput -- --duration 60 --samples fib,pattern_matching_stress,metta_programming_stress
//!
//! # Run samples for extended period
//! cargo bench --bench e2e_throughput -- --duration 300 --samples fib,knowledge_graph
//!
//! # Run with samply
//! samply record target/release/deps/e2e_throughput-* -- --duration 10 --samples knowledge_graph
//!
//! # Show help
//! cargo bench --bench e2e_throughput -- --help
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use mettatron::config::{configure_eval, get_eval_config, EvalConfig};
use mettatron::{compile, run_state, run_state_async, MettaState};

// TODO -> need more comprehensive set of MeTTa programs
const SAMPLES: &[(&str, &str)] = &[
    ("fib", include_str!("metta_samples/fib.metta")),
    (
        "knowledge_graph",
        include_str!("metta_samples/knowledge_graph.metta"),
    ),
    (
        "pattern_matching_stress",
        include_str!("metta_samples/pattern_matching_stress.metta"),
    ),
    (
        "concurrent_space_operations",
        include_str!("metta_samples/concurrent_space_operations.metta"),
    ),
    (
        "constraint_search_simple",
        include_str!("metta_samples/constraint_search_simple.metta"),
    ),
    (
        "metta_programming_stress",
        include_str!("metta_samples/metta_programming_stress.metta"),
    ),
    (
        "multi_space_reasoning",
        include_str!("metta_samples/multi_space_reasoning.metta"),
    ),
];

// TODO -> should has configurable setup for running with samply
#[derive(Parser, Debug)]
#[command(name = "e2e_throughput")]
#[command(about = "MeTTaTron throughput benchmarks", long_about = None)]
#[command(disable_help_flag = false)]
#[command(trailing_var_arg = true)]
#[command(allow_external_subcommands = true)]
struct Args {
    /// Test duration in seconds for each benchmark mode
    #[arg(short, long, default_value_t = 30)]
    duration: u64,

    /// List of sample names to benchmark
    /// Available: simple_fib, knowledge_graph, pattern_matching, concurrent_space_operations,
    /// constraint_search_simple, metta_programming_stress, multi_space_reasoning
    #[arg(short, long, value_delimiter = ',', default_value = "knowledge_graph")]
    samples: Vec<String>,

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

fn evaluate_full_program(source: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = MettaState::new_empty();
    let program = compile(source)?;
    let _result = run_state(state, program)?;
    Ok(())
}

async fn evaluate_full_program_async(
    source: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = MettaState::new_empty();
    let program = compile(source)?;
    let _result = run_state_async(state, program).await?;
    Ok(())
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

    configure_eval(EvalConfig::cpu_optimized());

    let config = get_eval_config();
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let test_duration = Duration::from_secs(args.duration);

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

        let seq = measure_sequential(name, source, test_duration);
        print_report(&seq);
        reports.push(seq);

        let par4 = measure_parallel(name, source, test_duration, 4);
        print_report(&par4);
        reports.push(par4);

        let par_full = measure_parallel(name, source, test_duration, num_cpus);
        print_report(&par_full);
        reports.push(par_full);

        let async_report = measure_async(name, source, test_duration, num_cpus);
        print_report(&async_report);
        reports.push(async_report);
    }
}
