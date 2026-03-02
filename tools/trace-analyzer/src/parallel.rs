//! Concurrency level analysis over time.
//!
//! Computes how many threads are simultaneously active at each point in time,
//! producing a concurrency histogram and summary statistics.

use std::collections::HashSet;

use crate::reader::TraceReader;

/// Boundary event for the sweep-line algorithm.
#[derive(Clone)]
struct Boundary {
    time_ns: u64,
    thread_id: u32,
    /// true = start of activity, false = end of activity
    is_start: bool,
}

pub fn run(file: &str, bucket_us: Option<u64>) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    if reader.format_version < 2 {
        return Err("The 'parallel' subcommand requires format v2 trace files with duration data. \
                    Re-record the trace with the latest MeTTaTron build.".to_string());
    }

    let bucket_ns = bucket_us.unwrap_or(100) * 1_000; // default 100us buckets

    // Collect boundary points from timed events
    let mut boundaries: Vec<Boundary> = Vec::new();

    for event in reader.events() {
        if let Some(dur) = event.duration_ns {
            if dur == 0 {
                continue;
            }
            boundaries.push(Boundary {
                time_ns: event.timestamp_ns,
                thread_id: event.thread_id,
                is_start: true,
            });
            boundaries.push(Boundary {
                time_ns: event.timestamp_ns + dur,
                thread_id: event.thread_id,
                is_start: false,
            });
        }
    }

    if boundaries.is_empty() {
        println!("No timed events found in trace file.");
        return Ok(());
    }

    // Sort: by time, then End before Start to avoid momentary overcounting
    boundaries.sort_by(|a, b| {
        a.time_ns.cmp(&b.time_ns)
            .then_with(|| a.is_start.cmp(&b.is_start)) // false (end) < true (start)
    });

    let min_ns = boundaries.first().map(|b| b.time_ns).unwrap_or(0);
    let max_ns = boundaries.last().map(|b| b.time_ns).unwrap_or(0);
    let range_ns = max_ns - min_ns;

    // Sweep and record concurrency levels
    let mut active: HashSet<u32> = HashSet::new();
    let mut concurrency_samples: Vec<(u64, usize)> = Vec::new(); // (time, level)

    for b in &boundaries {
        if b.is_start {
            active.insert(b.thread_id);
        } else {
            active.remove(&b.thread_id);
        }
        concurrency_samples.push((b.time_ns, active.len()));
    }

    // Compute weighted concurrency (time-weighted average)
    let mut weighted_sum: f64 = 0.0;
    let mut prev_time = min_ns;
    let mut prev_level: usize = 0;
    for &(time, level) in &concurrency_samples {
        if time > prev_time {
            weighted_sum += prev_level as f64 * (time - prev_time) as f64;
        }
        prev_time = time;
        prev_level = level;
    }
    let mean_concurrency = if range_ns > 0 {
        weighted_sum / range_ns as f64
    } else {
        0.0
    };

    // Max concurrency
    let max_concurrency = concurrency_samples.iter().map(|&(_, l)| l).max().unwrap_or(0);

    // Collect all distinct concurrency levels for percentile calculation
    let mut level_durations: Vec<(usize, u64)> = Vec::new(); // (level, duration_ns)
    let mut prev_time = min_ns;
    let mut prev_level: usize = 0;
    for &(time, level) in &concurrency_samples {
        if time > prev_time && prev_level > 0 {
            level_durations.push((prev_level, time - prev_time));
        }
        prev_time = time;
        prev_level = level;
    }
    level_durations.sort_by_key(|&(l, _)| l);

    let total_active_ns: u64 = level_durations.iter().map(|&(_, d)| d).sum();
    let p95_level = percentile_level(&level_durations, total_active_ns, 0.95);
    let p99_level = percentile_level(&level_durations, total_active_ns, 0.99);
    let median_level = percentile_level(&level_durations, total_active_ns, 0.50);

    // Thread-time = sum of all individual thread active durations
    let thread_time_ns: u64 = level_durations.iter().map(|&(l, d)| l as u64 * d).sum();
    let cpu_count = reader.header.cpu_count.max(1);
    let parallelism_efficiency = if range_ns > 0 {
        thread_time_ns as f64 / (range_ns as f64 * cpu_count as f64)
    } else {
        0.0
    };

    println!("=== Concurrency Analysis ===");
    println!();
    println!("Time range: {:.3}ms", range_ns as f64 / 1e6);
    println!("CPU count: {}", cpu_count);
    println!();
    println!("Mean concurrency: {:.2}", mean_concurrency);
    println!("Median concurrency: {}", median_level);
    println!("P95 concurrency: {}", p95_level);
    println!("P99 concurrency: {}", p99_level);
    println!("Max concurrency: {}", max_concurrency);
    println!("Parallelism efficiency: {:.1}% (thread-time / wall-time x CPUs)",
             parallelism_efficiency * 100.0);

    // Bucket into time slices for histogram
    if range_ns > 0 && bucket_ns > 0 {
        let num_buckets = ((range_ns + bucket_ns - 1) / bucket_ns) as usize;
        let num_buckets = num_buckets.min(200); // cap for display
        let actual_bucket_ns = range_ns / num_buckets as u64;

        let mut bucket_max: Vec<usize> = vec![0; num_buckets];
        for &(time, level) in &concurrency_samples {
            let bucket = ((time - min_ns) / actual_bucket_ns.max(1)) as usize;
            let bucket = bucket.min(num_buckets - 1);
            if level > bucket_max[bucket] {
                bucket_max[bucket] = level;
            }
        }

        println!();
        println!("--- Concurrency Over Time (max per {:.1}us bucket) ---", actual_bucket_ns as f64 / 1000.0);
        let chart_max = max_concurrency.max(1);
        let chart_width: usize = 60;
        for (i, &level) in bucket_max.iter().enumerate() {
            let bar_len = (level as f64 / chart_max as f64 * chart_width as f64) as usize;
            let bar: String = "\u{2588}".repeat(bar_len);
            let time_ms = (min_ns + i as u64 * actual_bucket_ns) as f64 / 1e6;
            println!("  {:>8.3}ms {:>2} {}", time_ms, level, bar);
        }
    }

    Ok(())
}

/// Compute weighted percentile from sorted (level, duration) pairs.
fn percentile_level(level_durations: &[(usize, u64)], total_ns: u64, pct: f64) -> usize {
    if level_durations.is_empty() || total_ns == 0 {
        return 0;
    }
    let target = (total_ns as f64 * pct) as u64;
    let mut cumulative = 0u64;
    for &(level, dur) in level_durations {
        cumulative += dur;
        if cumulative >= target {
            return level;
        }
    }
    level_durations.last().map(|&(l, _)| l).unwrap_or(0)
}
