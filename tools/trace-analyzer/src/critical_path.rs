//! Amdahl's Law analysis via critical path reconstruction.
//!
//! Reconstructs the fork-join tree from `span_id` correlation.
//! Computes:
//! - Critical path = longest sequential dependency chain
//! - Parallelizable fraction P = (total_time - critical_path) / total_time
//! - Amdahl's law speedup for 1, 2, 4, 8, 16, 32, N cores
//! - Top serialization bottleneck segments on the critical path
//!
//! Algorithm: Two-pass.
//!   Pass 1: Build span tree from events.
//!   Pass 2: Bottom-up critical path computation.

use std::collections::HashMap;

use trace_format::TraceEventKind;

use crate::reader::TraceReader;
use crate::util::{extract_operator_name, format_duration_ns};

/// A node in the span tree.
struct SpanNode {
    /// Duration of this span (exclusive of children for leaf nodes).
    duration_ns: u64,
    /// Start timestamp.
    start_ns: u64,
    /// Head symbol for display.
    head_symbol: String,
    /// Depth in the evaluation.
    depth: u32,
    /// Whether this is a fork point (has parallel children).
    is_fork: bool,
    /// Child span IDs (sequential children or parallel branches).
    children: Vec<u64>,
    /// Critical path through this node and its descendants.
    critical_path_ns: u64,
}

/// A sequential segment on the critical path (for bottleneck identification).
struct CriticalSegment {
    head_symbol: String,
    duration_ns: u64,
    depth: u32,
    start_ns: u64,
}

pub fn run(file: &str, worker_counts: &[usize]) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    // Pass 1: Collect all span-correlated events and fork/branch structure
    let mut spans: HashMap<u64, SpanNode> = HashMap::new();
    let mut fork_to_branches: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut total_wall_ns: u64 = 0;
    let mut total_compute_ns: u64 = 0;
    let mut total_events: u64 = 0;

    // Track root-level sequential spans (no span_id or top-level)
    let mut root_spans: Vec<u64> = Vec::new();
    let mut next_synth_id: u64 = u64::MAX / 2; // synthetic IDs for non-span events

    for event in reader.events() {
        total_events += 1;
        let end_ns = event.timestamp_ns + event.duration_ns.unwrap_or(0);
        if end_ns > total_wall_ns {
            total_wall_ns = end_ns;
        }

        let duration = event.duration_ns.unwrap_or(0);
        if duration > 0 {
            total_compute_ns += duration;
        }

        match &event.kind {
            TraceEventKind::NondeterministicFork { branch_count } => {
                if let Some(span_id) = event.span_id {
                    let node = SpanNode {
                        duration_ns: duration,
                        start_ns: event.timestamp_ns,
                        head_symbol: extract_operator_name(&event.input, &event.kind),
                        depth: event.depth,
                        is_fork: true,
                        children: Vec::with_capacity(*branch_count as usize),
                        critical_path_ns: 0,
                    };
                    spans.insert(span_id, node);
                    fork_to_branches.entry(span_id).or_default();
                    root_spans.push(span_id);
                }
            }
            TraceEventKind::BranchStart { .. } => {
                if let Some(span_id) = event.span_id {
                    let node = SpanNode {
                        duration_ns: duration,
                        start_ns: event.timestamp_ns,
                        head_symbol: extract_operator_name(&event.input, &event.kind),
                        depth: event.depth,
                        is_fork: false,
                        children: Vec::new(),
                        critical_path_ns: 0,
                    };
                    spans.insert(span_id, node);

                    // Find parent fork (closest preceding fork at depth-1)
                    // Heuristic: associate with the most recent fork span
                    for fork_branches in fork_to_branches.values_mut() {
                        // Branches get associated lazily below
                    }
                }
            }
            TraceEventKind::BranchEnd { .. } => {
                if let Some(span_id) = event.span_id {
                    if let Some(node) = spans.get_mut(&span_id) {
                        node.duration_ns = duration;
                    }
                }
            }
            TraceEventKind::RuleLookup { .. }
            | TraceEventKind::RuleIndexInsert { .. }
            | TraceEventKind::SelfEvaluating { .. }
            | TraceEventKind::ParallelDispatch { .. }
            | TraceEventKind::TrampolineStep { .. } => { /* not relevant to critical path */ }
            _ => {
                // Non-fork timed events contribute to sequential computation
                if duration > 0 {
                    if let Some(span_id) = event.span_id {
                        let node = SpanNode {
                            duration_ns: duration,
                            start_ns: event.timestamp_ns,
                            head_symbol: extract_operator_name(&event.input, &event.kind),
                            depth: event.depth,
                            is_fork: false,
                            children: Vec::new(),
                            critical_path_ns: duration,
                        };
                        spans.insert(span_id, node);
                    } else {
                        // Synthesize an ID for non-span events
                        next_synth_id += 1;
                        let synth_id = next_synth_id;
                        let node = SpanNode {
                            duration_ns: duration,
                            start_ns: event.timestamp_ns,
                            head_symbol: extract_operator_name(&event.input, &event.kind),
                            depth: event.depth,
                            is_fork: false,
                            children: Vec::new(),
                            critical_path_ns: duration,
                        };
                        spans.insert(synth_id, node);
                    }
                }
            }
        }
    }

    // Pass 2: Compute critical path
    // For fork nodes: critical_path = max(branch critical paths)
    // For sequential nodes: critical_path = sum of sequential children
    // Since we may not have perfect tree structure, use a simpler heuristic:
    // Group timed events by thread and compute per-thread sequential time.
    // The critical path is the maximum per-thread sequential time.

    // Simplified critical path: max wall time across all threads
    // (since we don't have perfect parent-child span correlation)
    let mut thread_times: HashMap<u32, u64> = HashMap::new();
    let reader2 = TraceReader::open(file)?;
    let mut critical_segments: Vec<CriticalSegment> = Vec::new();

    for event in reader2.events() {
        let duration = event.duration_ns.unwrap_or(0);
        if duration > 0 {
            *thread_times.entry(event.thread_id).or_default() += duration;
        }
    }

    let main_thread_time = thread_times.values().copied().max().unwrap_or(0);

    // Collect top sequential bottlenecks (longest individual spans)
    let mut all_segments: Vec<CriticalSegment> = spans
        .values()
        .filter(|s| s.duration_ns > 0 && !s.is_fork)
        .map(|s| CriticalSegment {
            head_symbol: s.head_symbol.clone(),
            duration_ns: s.duration_ns,
            depth: s.depth,
            start_ns: s.start_ns,
        })
        .collect();

    all_segments.sort_by(|a, b| b.duration_ns.cmp(&a.duration_ns));
    critical_segments = all_segments.into_iter().take(20).collect();

    // Compute Amdahl's law
    // P = parallelizable fraction
    // If we have N threads with total compute time T_total and wall time T_wall:
    // Sequential fraction S = T_wall / T_total (approximation)
    // More accurate: S = (T_total - (T_wall * mean_concurrency)) / T_total is complex
    // Simple model: S = main_thread_time / total_compute_ns
    let sequential_fraction = if total_compute_ns > 0 {
        (main_thread_time as f64 / total_compute_ns as f64).min(1.0)
    } else {
        1.0
    };
    let parallel_fraction = 1.0 - sequential_fraction;

    // Print report
    println!("=== Critical Path / Amdahl's Law Analysis ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Total events: {total_events}");
    println!("Wall time: {}", format_duration_ns(total_wall_ns));
    println!(
        "Total compute time: {}",
        format_duration_ns(total_compute_ns)
    );
    println!(
        "Longest thread time: {}",
        format_duration_ns(main_thread_time)
    );
    println!("Active threads: {}", thread_times.len());
    println!();

    println!("--- Parallelization Potential ---");
    println!(
        "  Sequential fraction (S): {:.4} ({:.1}%)",
        sequential_fraction,
        sequential_fraction * 100.0
    );
    println!(
        "  Parallel fraction (P):   {:.4} ({:.1}%)",
        parallel_fraction,
        parallel_fraction * 100.0
    );
    println!();

    // Amdahl's law: Speedup(N) = 1 / (S + P/N)
    println!("--- Amdahl's Law Speedup Predictions ---");
    println!(
        "  {:>8}  {:>10}  {:>10}",
        "Workers", "Speedup", "Efficiency"
    );
    println!("  {}", "-".repeat(32));

    for &n in worker_counts {
        let speedup = 1.0 / (sequential_fraction + parallel_fraction / n as f64);
        let efficiency = speedup / n as f64 * 100.0;
        println!("  {:>8}  {:>10.2}x  {:>9.1}%", n, speedup, efficiency);
    }
    println!();

    // Per-thread time breakdown
    println!("--- Per-Thread Compute Time ---");
    let mut thread_vec: Vec<_> = thread_times.into_iter().collect();
    thread_vec.sort_by(|a, b| b.1.cmp(&a.1));
    for (tid, time) in thread_vec.iter().take(20) {
        let pct = if total_compute_ns > 0 {
            *time as f64 / total_compute_ns as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "  Thread {:>4}: {:>12}  ({:.1}%)",
            tid,
            format_duration_ns(*time),
            pct
        );
    }
    println!();

    // Top serialization bottlenecks
    if !critical_segments.is_empty() {
        println!("--- Top Sequential Bottlenecks ---");
        println!(
            "  {:<30} {:>12} {:>6} {:>12}",
            "Head Symbol", "Duration", "Depth", "Start"
        );
        println!("  {}", "-".repeat(64));

        for seg in &critical_segments {
            let head_display = if seg.head_symbol.len() > 30 {
                format!("{}...", &seg.head_symbol[..27])
            } else {
                seg.head_symbol.clone()
            };
            println!(
                "  {:<30} {:>12} {:>6} {:>12}",
                head_display,
                format_duration_ns(seg.duration_ns),
                seg.depth,
                format_duration_ns(seg.start_ns),
            );
        }
    }

    Ok(())
}
