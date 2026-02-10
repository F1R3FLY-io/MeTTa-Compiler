//! Memory Leak Stress Test for mmverify
//!
//! Runs the mmverify demo in a loop for 60 seconds while monitoring RSS memory
//! via `sysinfo`. Fails early if a sustained memory leak is detected using
//! linear regression on post-warmup samples.
//!
//! Architecture: two threads
//!   - Main thread: compiles once, then loops `new_arena_env()` + `run_state()`
//!   - Monitor thread: polls RSS every 100ms, runs leak analysis every 5s
//!
//! Leak detection requires ALL three conditions:
//!   1. Growth rate > 1 MB/s (linear regression slope)
//!   2. R² > 0.8 (sustained trend, not jemalloc stepped allocation)
//!   3. Final RSS > 2× baseline (absolute significance)
//!
//! Exit codes: 0 = pass, 1 = leak detected, 2 = monitoring failure, 3 = compilation failure
//!
//! Run with:
//!   cargo run --release --example mmverify_memory_stress

use std::hint::black_box;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mettatron::config::{configure_eval, EvalConfig};
use mettatron::{compile_arena, new_arena_env, run_state};
use sysinfo::{Pid, System};

// Include mmverify sources
const MMVERIFY_UTILS: &str = include_str!("../examples/mmverify/mmverify-utils.metta");
const VERIFY_DEMO0_BODY: &str =
    include_str!("../benches/mmverify_samples/verify_demo0_body.metta");

const TOTAL_DURATION: Duration = Duration::from_secs(60);
const WARMUP_DURATION: Duration = Duration::from_secs(5);
const WARMUP_BASELINE_START: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const ANALYSIS_INTERVAL: Duration = Duration::from_secs(5);

// Leak detection thresholds
const GROWTH_RATE_THRESHOLD_BYTES_PER_SEC: f64 = 1_048_576.0; // 1 MB/s
const R_SQUARED_THRESHOLD: f64 = 0.8;
const BASELINE_MULTIPLIER: f64 = 2.0;

#[derive(Clone, Debug)]
struct MemorySample {
    elapsed_secs: f64,
    rss_bytes: u64,
}

struct LeakAnalysis {
    slope_bytes_per_sec: f64,
    r_squared: f64,
    baseline_bytes: u64,
    current_bytes: u64,
    is_leak: bool,
}

/// Compute linear regression slope and R² over memory samples.
///
/// slope = Σ[(tᵢ - t̄)(rssᵢ - r̄)] / Σ[(tᵢ - t̄)²]
/// R² = 1 - SS_res / SS_tot
fn linear_regression(samples: &[MemorySample]) -> (f64, f64) {
    let n = samples.len() as f64;
    if n < 2.0 {
        return (0.0, 0.0);
    }

    let mean_t: f64 = samples.iter().map(|s| s.elapsed_secs).sum::<f64>() / n;
    let mean_rss: f64 = samples.iter().map(|s| s.rss_bytes as f64).sum::<f64>() / n;

    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for s in samples {
        let dt = s.elapsed_secs - mean_t;
        let dr = s.rss_bytes as f64 - mean_rss;
        numerator += dt * dr;
        denominator += dt * dt;
    }

    if denominator.abs() < f64::EPSILON {
        return (0.0, 0.0);
    }

    let slope = numerator / denominator;

    // R² = 1 - SS_res / SS_tot
    let ss_tot: f64 = samples
        .iter()
        .map(|s| {
            let d = s.rss_bytes as f64 - mean_rss;
            d * d
        })
        .sum();

    if ss_tot.abs() < f64::EPSILON {
        return (slope, 1.0); // Perfect fit if no variance
    }

    let ss_res: f64 = samples
        .iter()
        .map(|s| {
            let predicted = mean_rss + slope * (s.elapsed_secs - mean_t);
            let residual = s.rss_bytes as f64 - predicted;
            residual * residual
        })
        .sum();

    let r_squared = 1.0 - ss_res / ss_tot;
    (slope, r_squared)
}

/// Compute the baseline RSS as mean over the warmup window [3s, 5s].
fn compute_baseline(samples: &[MemorySample]) -> u64 {
    let warmup_start = WARMUP_BASELINE_START.as_secs_f64();
    let warmup_end = WARMUP_DURATION.as_secs_f64();

    let baseline_samples: Vec<_> = samples
        .iter()
        .filter(|s| s.elapsed_secs >= warmup_start && s.elapsed_secs <= warmup_end)
        .collect();

    if baseline_samples.is_empty() {
        // Fall back to last warmup sample
        return samples
            .iter()
            .filter(|s| s.elapsed_secs <= warmup_end)
            .last()
            .map(|s| s.rss_bytes)
            .unwrap_or(0);
    }

    let sum: u64 = baseline_samples.iter().map(|s| s.rss_bytes).sum();
    sum / baseline_samples.len() as u64
}

/// Analyze post-warmup samples for memory leak.
fn analyze_leak(all_samples: &[MemorySample], baseline_bytes: u64) -> LeakAnalysis {
    let warmup_end = WARMUP_DURATION.as_secs_f64();
    let post_warmup: Vec<_> = all_samples
        .iter()
        .filter(|s| s.elapsed_secs > warmup_end)
        .cloned()
        .collect();

    if post_warmup.len() < 10 {
        return LeakAnalysis {
            slope_bytes_per_sec: 0.0,
            r_squared: 0.0,
            baseline_bytes,
            current_bytes: all_samples.last().map(|s| s.rss_bytes).unwrap_or(0),
            is_leak: false,
        };
    }

    let (slope, r_squared) = linear_regression(&post_warmup);
    let current_bytes = post_warmup.last().map(|s| s.rss_bytes).unwrap_or(0);

    let is_leak = slope > GROWTH_RATE_THRESHOLD_BYTES_PER_SEC
        && r_squared > R_SQUARED_THRESHOLD
        && current_bytes > (baseline_bytes as f64 * BASELINE_MULTIPLIER) as u64;

    LeakAnalysis {
        slope_bytes_per_sec: slope,
        r_squared,
        baseline_bytes,
        current_bytes,
        is_leak,
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.2} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn main() {
    println!("=== mmverify Memory Leak Stress Test ===");
    println!("Duration: {}s (warmup: {}s)", TOTAL_DURATION.as_secs(), WARMUP_DURATION.as_secs());
    println!("Leak thresholds: growth > 1 MB/s, R² > {}, final > {}× baseline",
        R_SQUARED_THRESHOLD, BASELINE_MULTIPLIER);
    println!();

    // Configure evaluator
    configure_eval(EvalConfig::cpu_optimized());

    // Compile the mmverify program once
    let program = format!("{}\n\n{}", MMVERIFY_UTILS, VERIFY_DEMO0_BODY);
    println!("Compiling mmverify program...");
    let compiled_state = match compile_arena(&program) {
        Ok(state) => state,
        Err(e) => {
            eprintln!("FATAL: Failed to compile mmverify program: {}", e);
            process::exit(3);
        }
    };
    println!("Compilation successful.");
    println!();

    // Shared state
    let stop_signal = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(Mutex::new(Vec::<MemorySample>::with_capacity(1024)));

    let pid = Pid::from_u32(process::id());
    let start_time = Instant::now();

    // Spawn monitor thread
    let monitor_stop = Arc::clone(&stop_signal);
    let monitor_samples = Arc::clone(&samples);
    let monitor_handle = thread::spawn(move || {
        let mut sys = System::new();
        let mut last_analysis = Instant::now();
        let mut baseline: Option<u64> = None;

        loop {
            let elapsed = start_time.elapsed();
            if elapsed >= TOTAL_DURATION || monitor_stop.load(Ordering::Relaxed) {
                break;
            }

            // Refresh process info and sample RSS
            sys.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_memory(),
            );

            if let Some(proc_info) = sys.process(pid) {
                let rss_bytes = proc_info.memory();
                let sample = MemorySample {
                    elapsed_secs: elapsed.as_secs_f64(),
                    rss_bytes,
                };

                let mut locked = monitor_samples.lock().expect("samples lock poisoned");
                locked.push(sample);

                // Compute baseline after warmup
                if baseline.is_none() && elapsed >= WARMUP_DURATION {
                    baseline = Some(compute_baseline(&locked));
                    println!(
                        "[{:6.1}s] Warmup complete. Baseline RSS: {}",
                        elapsed.as_secs_f64(),
                        format_bytes(baseline.expect("baseline just set"))
                    );
                }

                // Periodic leak analysis (every 5s after warmup)
                if baseline.is_some() && last_analysis.elapsed() >= ANALYSIS_INTERVAL {
                    last_analysis = Instant::now();
                    let analysis = analyze_leak(&locked, baseline.expect("baseline is Some"));

                    println!(
                        "[{:6.1}s] RSS: {} | growth: {:.2} KB/s | R²: {:.4}",
                        elapsed.as_secs_f64(),
                        format_bytes(analysis.current_bytes),
                        analysis.slope_bytes_per_sec / 1024.0,
                        analysis.r_squared,
                    );

                    if analysis.is_leak {
                        eprintln!();
                        eprintln!("LEAK DETECTED!");
                        eprintln!(
                            "  Growth rate: {:.2} MB/s (threshold: 1.00 MB/s)",
                            analysis.slope_bytes_per_sec / 1_048_576.0
                        );
                        eprintln!("  R²: {:.4} (threshold: {:.1})", analysis.r_squared, R_SQUARED_THRESHOLD);
                        eprintln!(
                            "  Current RSS: {} (baseline: {}, ratio: {:.2}×)",
                            format_bytes(analysis.current_bytes),
                            format_bytes(analysis.baseline_bytes),
                            analysis.current_bytes as f64 / analysis.baseline_bytes as f64,
                        );
                        monitor_stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            } else {
                eprintln!("WARNING: Could not read process info for pid {}", pid.as_u32());
            }

            thread::sleep(POLL_INTERVAL);
        }
    });

    // Main evaluation loop
    println!("Starting evaluation loop...");
    let mut iteration_count: u64 = 0;
    let eval_start = Instant::now();

    loop {
        if stop_signal.load(Ordering::Relaxed) {
            break;
        }

        let elapsed = start_time.elapsed();
        if elapsed >= TOTAL_DURATION {
            break;
        }

        let env = new_arena_env();
        match run_state(env, &compiled_state) {
            Ok(result) => {
                black_box(result);
            }
            Err(e) => {
                eprintln!("WARNING: Iteration {} failed: {}", iteration_count, e);
            }
        }

        iteration_count += 1;
    }

    let eval_elapsed = eval_start.elapsed();

    // Wait for monitor thread
    monitor_handle.join().expect("monitor thread panicked");

    // Final report
    println!();
    println!("=== Final Report ===");
    println!("Iterations: {}", iteration_count);
    println!(
        "Throughput: {:.2} iter/s",
        iteration_count as f64 / eval_elapsed.as_secs_f64()
    );
    println!("Wall time:  {:.1}s", eval_elapsed.as_secs_f64());

    let locked = samples.lock().expect("samples lock poisoned");

    if locked.len() < 2 {
        eprintln!("ERROR: Insufficient memory samples collected ({})", locked.len());
        process::exit(2);
    }

    let baseline = compute_baseline(&locked);
    let analysis = analyze_leak(&locked, baseline);

    let peak_rss = locked.iter().map(|s| s.rss_bytes).max().unwrap_or(0);
    let min_rss = locked.iter().map(|s| s.rss_bytes).min().unwrap_or(0);

    println!();
    println!("Memory Statistics:");
    println!("  Baseline RSS:  {}", format_bytes(baseline));
    println!("  Final RSS:     {}", format_bytes(analysis.current_bytes));
    println!("  Peak RSS:      {}", format_bytes(peak_rss));
    println!("  Min RSS:       {}", format_bytes(min_rss));
    println!("  Growth rate:   {:.2} KB/s", analysis.slope_bytes_per_sec / 1024.0);
    println!("  R²:            {:.4}", analysis.r_squared);
    println!(
        "  Final/Base:    {:.2}×",
        if baseline > 0 {
            analysis.current_bytes as f64 / baseline as f64
        } else {
            0.0
        }
    );

    println!();
    if analysis.is_leak {
        println!("RESULT: FAIL - Memory leak detected");
        process::exit(1);
    } else {
        println!("RESULT: PASS - No memory leak detected");
        process::exit(0);
    }
}
