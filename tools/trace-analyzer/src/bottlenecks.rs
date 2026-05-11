//! Serialization bottleneck identification.
//!
//! Identifies intervals where concurrency drops below a threshold for
//! longer than a minimum duration, then classifies the cause.

use std::collections::HashSet;

use crate::reader::TraceReader;
use trace_format::TraceEventKind;

/// A detected low-concurrency interval.
struct Bottleneck {
    start_ns: u64,
    end_ns: u64,
    max_concurrency: usize,
    cause: String,
}

struct BoundaryPt {
    time_ns: u64,
    thread_id: u32,
    is_start: bool,
}

struct TimedEvent {
    start_ns: u64,
    end_ns: u64,
    #[allow(dead_code)]
    thread_id: u32,
    kind_label: String,
}

pub fn run(
    file: &str,
    concurrency_threshold: Option<usize>,
    duration_threshold_us: Option<u64>,
    top_n: Option<usize>,
) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    if reader.format_version < 2 {
        return Err(
            "The 'bottlenecks' subcommand requires format v2 trace files with duration data. \
                    Re-record the trace with the latest MeTTaTron build."
                .to_string(),
        );
    }

    let conc_threshold = concurrency_threshold.unwrap_or(2);
    let dur_threshold_ns = duration_threshold_us.unwrap_or(1000) * 1_000; // default 1ms
    let top_n = top_n.unwrap_or(20);

    let mut boundaries: Vec<BoundaryPt> = Vec::new();
    let mut timed_events: Vec<TimedEvent> = Vec::new();

    for event in reader.events() {
        if let Some(dur) = event.duration_ns {
            if dur == 0 {
                continue;
            }
            boundaries.push(BoundaryPt {
                time_ns: event.timestamp_ns,
                thread_id: event.thread_id,
                is_start: true,
            });
            boundaries.push(BoundaryPt {
                time_ns: event.timestamp_ns + dur,
                thread_id: event.thread_id,
                is_start: false,
            });
            timed_events.push(TimedEvent {
                start_ns: event.timestamp_ns,
                end_ns: event.timestamp_ns + dur,
                thread_id: event.thread_id,
                kind_label: classify_event_kind(&event.kind),
            });
        }
    }

    if boundaries.is_empty() {
        println!("No timed events found in trace file.");
        return Ok(());
    }

    // Sort boundaries
    boundaries.sort_by(|a, b| {
        a.time_ns
            .cmp(&b.time_ns)
            .then_with(|| a.is_start.cmp(&b.is_start))
    });

    // Sweep and detect low-concurrency intervals
    let mut active: HashSet<u32> = HashSet::new();
    let mut bottlenecks: Vec<Bottleneck> = Vec::new();
    let mut low_start: Option<u64> = None;
    let mut low_max_conc: usize = 0;

    for b in &boundaries {
        if b.is_start {
            active.insert(b.thread_id);
        } else {
            active.remove(&b.thread_id);
        }

        let level = active.len();

        if level < conc_threshold {
            if low_start.is_none() {
                low_start = Some(b.time_ns);
                low_max_conc = level;
            } else if level > low_max_conc {
                low_max_conc = level;
            }
        } else if let Some(start) = low_start {
            let duration = b.time_ns.saturating_sub(start);
            if duration >= dur_threshold_ns {
                // Classify cause by examining active events during this interval
                let cause = classify_bottleneck(&timed_events, start, b.time_ns);
                bottlenecks.push(Bottleneck {
                    start_ns: start,
                    end_ns: b.time_ns,
                    max_concurrency: low_max_conc,
                    cause,
                });
            }
            low_start = None;
        }
    }

    // Handle trailing low-concurrency interval
    if let Some(start) = low_start {
        let end = boundaries.last().map(|b| b.time_ns).unwrap_or(start);
        let duration = end.saturating_sub(start);
        if duration >= dur_threshold_ns {
            let cause = classify_bottleneck(&timed_events, start, end);
            bottlenecks.push(Bottleneck {
                start_ns: start,
                end_ns: end,
                max_concurrency: low_max_conc,
                cause,
            });
        }
    }

    // Sort by duration descending
    bottlenecks.sort_by(|a, b| {
        let dur_b = b.end_ns - b.start_ns;
        let dur_a = a.end_ns - a.start_ns;
        dur_b.cmp(&dur_a)
    });

    let total_wall_ns: u64 = boundaries.last().map(|b| b.time_ns).unwrap_or(0)
        - boundaries.first().map(|b| b.time_ns).unwrap_or(0);

    println!("=== Serialization Bottleneck Analysis ===");
    println!();
    println!("Concurrency threshold: < {} threads", conc_threshold);
    println!(
        "Duration threshold: >= {:.1}ms",
        dur_threshold_ns as f64 / 1e6
    );
    println!("Total bottlenecks found: {}", bottlenecks.len());
    println!();

    if bottlenecks.is_empty() {
        println!("No bottlenecks detected.");
        return Ok(());
    }

    let total_bottleneck_ns: u64 = bottlenecks.iter().map(|b| b.end_ns - b.start_ns).sum();
    println!(
        "Total bottleneck time: {:.3}ms ({:.1}% of wall time)",
        total_bottleneck_ns as f64 / 1e6,
        if total_wall_ns > 0 {
            total_bottleneck_ns as f64 / total_wall_ns as f64 * 100.0
        } else {
            0.0
        }
    );
    println!();

    println!("--- Top {} Bottlenecks by Duration ---", top_n);
    println!(
        "  {:<12} {:<12} {:<12} {:<6} {}",
        "Start (ms)", "End (ms)", "Duration", "MaxC", "Cause"
    );
    for b in bottlenecks.iter().take(top_n) {
        let dur = b.end_ns - b.start_ns;
        println!(
            "  {:>10.3}  {:>10.3}  {:>8.3}ms  {:>4}  {}",
            b.start_ns as f64 / 1e6,
            b.end_ns as f64 / 1e6,
            dur as f64 / 1e6,
            b.max_concurrency,
            b.cause
        );
    }

    // Aggregate causes
    println!();
    println!("--- Bottleneck Causes Summary ---");
    let mut cause_totals: Vec<(String, u64, usize)> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for b in &bottlenecks {
        let idx = seen.entry(b.cause.clone()).or_insert_with(|| {
            cause_totals.push((b.cause.clone(), 0, 0));
            cause_totals.len() - 1
        });
        cause_totals[*idx].1 += b.end_ns - b.start_ns;
        cause_totals[*idx].2 += 1;
    }
    cause_totals.sort_by(|a, b| b.1.cmp(&a.1));
    for (cause, total_ns, count) in &cause_totals {
        println!(
            "  {:<30} {:>6} occurrences  {:>10.3}ms",
            cause,
            count,
            *total_ns as f64 / 1e6
        );
    }

    Ok(())
}

fn classify_event_kind(kind: &TraceEventKind) -> String {
    match kind {
        TraceEventKind::GcSafepoint { .. } => "gc-pause".to_string(),
        TraceEventKind::EvalStart | TraceEventKind::EvalEnd { .. } => "eval".to_string(),
        TraceEventKind::GroundedOp { .. } => "grounded-op".to_string(),
        TraceEventKind::BranchStart { .. } | TraceEventKind::BranchEnd { .. } => {
            "branching".to_string()
        }
        TraceEventKind::WorkPoolTaskCompleted { .. } => "workpool-task".to_string(),
        _ => "other".to_string(),
    }
}

fn classify_bottleneck(events: &[TimedEvent], start: u64, end: u64) -> String {
    // Find events that overlap with the bottleneck interval
    let mut cause_durations: std::collections::HashMap<&str, u64> =
        std::collections::HashMap::new();

    for ev in events {
        // Check for overlap: event overlaps [start, end] if ev.start < end && ev.end > start
        if ev.start_ns < end && ev.end_ns > start {
            let overlap_start = ev.start_ns.max(start);
            let overlap_end = ev.end_ns.min(end);
            let overlap = overlap_end.saturating_sub(overlap_start);
            *cause_durations.entry(&ev.kind_label).or_default() += overlap;
        }
    }

    if cause_durations.is_empty() {
        return "idle".to_string();
    }

    // Return the cause with the longest overlap
    cause_durations
        .into_iter()
        .max_by_key(|&(_, d)| d)
        .map(|(cause, _)| cause.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
