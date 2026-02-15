//! End-to-end throughput benchmarks for MeTTaTron
//!
//! Simple, fixed-configuration benchmark that measures programs/second throughput.
//! No CLI args, no configuration - just run and get results.
//!
//! ## Usage
//!
//! ```bash
//! cargo bench --bench e2e_throughput
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mettatron::config::{configure_eval, EvalConfig};
use mettatron::{compile, run_state, run_state_async, MettaState};

const SAMPLES: &[(&str, &str)] = &[
    ("fib", include_str!("../metta_samples/fib.metta")),
    (
        "knowledge_graph",
        include_str!("../metta_samples/knowledge_graph.metta"),
    ),
    (
        "pattern_matching_stress",
        include_str!("../metta_samples/pattern_matching_stress.metta"),
    ),
    (
        "concurrent_space_operations",
        include_str!("../metta_samples/concurrent_space_operations.metta"),
    ),
    (
        "constraint_search",
        include_str!("../metta_samples/constraint_search_simple.metta"),
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
        "backward_chaining",
        include_str!("../metta_samples/backward_chaining.metta"),
    ),
];

const WARMUP_DURATION: Duration = Duration::from_secs(5);
const TEST_DURATION: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
struct BenchResult {
    name: String,
    throughput: f64,
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

fn warmup_sequential(source: &str) {
    let start = Instant::now();
    while start.elapsed() < WARMUP_DURATION {
        let _ = evaluate_full_program(source);
    }
}

fn warmup_parallel(source: &str, num_workers: usize) {
    let start = Instant::now();
    thread::scope(|s| {
        for _ in 0..num_workers {
            s.spawn(|| {
                while start.elapsed() < WARMUP_DURATION {
                    let _ = evaluate_full_program(source);
                }
            });
        }
    });
}

fn warmup_async(source: &'static str, concurrency: usize, max_blocking: usize) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(max_blocking)
        .enable_all()
        .build()
        .unwrap();

    let start = Instant::now();
    rt.block_on(async {
        let mut handles = Vec::with_capacity(concurrency);
        for _ in 0..concurrency {
            let handle = tokio::spawn(async move {
                while start.elapsed() < WARMUP_DURATION {
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

fn measure_sequential(source: &str) -> f64 {
    let start = Instant::now();
    let mut completed = 0u64;

    while start.elapsed() < TEST_DURATION {
        if evaluate_full_program(source).is_ok() {
            completed += 1;
        }
    }

    completed as f64 / start.elapsed().as_secs_f64()
}

fn measure_parallel(source: &str, num_workers: usize) -> f64 {
    let completed = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    thread::scope(|s| {
        for _ in 0..num_workers {
            let completed = Arc::clone(&completed);
            s.spawn(move || {
                let mut local_completed = 0u64;
                while start.elapsed() < TEST_DURATION {
                    if evaluate_full_program(source).is_ok() {
                        local_completed += 1;
                    }
                }
                completed.fetch_add(local_completed, Ordering::Relaxed);
            });
        }
    });

    let elapsed = start.elapsed().as_secs_f64();
    completed.load(Ordering::Relaxed) as f64 / elapsed
}

fn measure_async(source: &'static str, concurrency: usize, max_blocking: usize) -> f64 {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(max_blocking)
        .enable_all()
        .build()
        .unwrap();

    let completed = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    rt.block_on(async {
        let mut handles = Vec::with_capacity(concurrency);

        for _ in 0..concurrency {
            let completed = Arc::clone(&completed);
            let handle = tokio::spawn(async move {
                let mut local_completed = 0u64;
                while start.elapsed() < TEST_DURATION {
                    if evaluate_full_program_async(source).await.is_ok() {
                        local_completed += 1;
                    }
                    tokio::task::yield_now().await;
                }
                completed.fetch_add(local_completed, Ordering::Relaxed);
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }
    });

    let elapsed = start.elapsed().as_secs_f64();
    completed.load(Ordering::Relaxed) as f64 / elapsed
}

fn print_results_table(results: &[BenchResult]) {
    println!("\n{:<37} | {}", "e2e", "throughput (programs/sec)");
    println!("{:<37} | {}", "─".repeat(37), "─".repeat(24));

    for result in results {
        println!("{:<37} | {:.2}", result.name, result.throughput);
    }
}

fn main() {
    configure_eval(EvalConfig::cpu_optimized());

    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let mut all_results = Vec::new();

    for (name, source) in SAMPLES {
        // Sequential
        println!("Benchmarking {} [sequential]", name);
        warmup_sequential(source);
        let seq_throughput = measure_sequential(source);
        all_results.push(BenchResult {
            name: format!("{}_sequential", name),
            throughput: seq_throughput,
        });

        // Parallel-4
        println!("Benchmarking {} [parallel-4]", name);
        warmup_parallel(source, 4);
        let par4_throughput = measure_parallel(source, 4);
        all_results.push(BenchResult {
            name: format!("{}_parallel_4", name),
            throughput: par4_throughput,
        });

        // Parallel-N (all CPUs)
        println!("Benchmarking {} [parallel-{}]", name, num_cpus);
        warmup_parallel(source, num_cpus);
        let par_full_throughput = measure_parallel(source, num_cpus);
        all_results.push(BenchResult {
            name: format!("{}_parallel_{}", name, num_cpus),
            throughput: par_full_throughput,
        });

        // Async
        println!("Benchmarking {} [async-{}]", name, num_cpus);
        warmup_async(source, num_cpus, 24);
        let async_throughput = measure_async(source, num_cpus, 24);
        all_results.push(BenchResult {
            name: format!("{}_async_{}", name, num_cpus),
            throughput: async_throughput,
        });
    }

    print_results_table(&all_results);
}
