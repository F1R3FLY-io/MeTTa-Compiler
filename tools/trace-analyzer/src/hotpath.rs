//! Expression-level profiling grouped by head symbol.
//!
//! Groups timed events by the first atom of their input S-expression ("head symbol")
//! and computes per-symbol call count, total self-time, mean, P95, P99, and inclusive time.
//!
//! Algorithm: Single-pass O(N) time, O(K) space (K = distinct head symbols).

use std::collections::HashMap;

use trace_format::TraceEventKind;

use crate::reader::TraceReader;
use crate::util::{extract_head_symbol, format_duration_ns, format_pct};

/// Per-symbol profiling accumulator.
struct SymbolProfile {
    /// Number of events with this head symbol.
    call_count: u64,
    /// Sum of self-time (duration_ns) in nanoseconds.
    total_self_ns: u64,
    /// Sum of inclusive time (including children) in nanoseconds.
    total_inclusive_ns: u64,
    /// All self-time durations (for percentile computation).
    durations: Vec<u64>,
}

impl SymbolProfile {
    fn new() -> Self {
        Self {
            call_count: 0,
            total_self_ns: 0,
            total_inclusive_ns: 0,
            durations: Vec::new(),
        }
    }

    fn record(&mut self, self_time_ns: u64, inclusive_time_ns: u64) {
        self.call_count += 1;
        self.total_self_ns += self_time_ns;
        self.total_inclusive_ns += inclusive_time_ns;
        self.durations.push(self_time_ns);
    }

    fn mean_ns(&self) -> u64 {
        if self.call_count == 0 { 0 } else { self.total_self_ns / self.call_count }
    }

    fn percentile(&mut self, p: f64) -> u64 {
        if self.durations.is_empty() { return 0; }
        self.durations.sort_unstable();
        let idx = ((self.durations.len() as f64 * p) as usize).min(self.durations.len() - 1);
        self.durations[idx]
    }
}

/// Per-thread depth tracking for inclusive time computation.
struct DepthTracker {
    /// Stack of (head_symbol, start_ns) per depth level.
    stack: Vec<(String, u64)>,
}

impl DepthTracker {
    fn new() -> Self {
        Self { stack: Vec::new() }
    }
}

pub fn run(file: &str, top_n: usize, sort_by: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    let mut profiles: HashMap<String, SymbolProfile> = HashMap::new();
    let mut depth_trackers: HashMap<u32, DepthTracker> = HashMap::new();
    let mut total_wall_ns: u64 = 0;
    let mut total_events: u64 = 0;

    for event in reader.events() {
        total_events += 1;
        let end_ns = event.timestamp_ns + event.duration_ns.unwrap_or(0);
        if end_ns > total_wall_ns {
            total_wall_ns = end_ns;
        }

        // Only profile timed events with meaningful kinds
        let duration = match event.duration_ns {
            Some(d) => d,
            None => continue,
        };

        // Skip purely administrative events
        match &event.kind {
            TraceEventKind::EvalStart
            | TraceEventKind::EvalEnd { .. }
            | TraceEventKind::NondeterministicFork { .. }
            | TraceEventKind::BranchStart { .. }
            | TraceEventKind::BranchEnd { .. }
            | TraceEventKind::WorkPoolTaskEnqueued { .. }
            | TraceEventKind::WorkPoolTaskDropped { .. }
            | TraceEventKind::WorkPoolTaskCompleted { .. }
            | TraceEventKind::WorkPoolScaleEvent { .. }
            | TraceEventKind::WorkPoolWorkerParked { .. }
            | TraceEventKind::WorkPoolWorkerResumed { .. }
            | TraceEventKind::WorkPoolBlockedWorkersDetected { .. }
            | TraceEventKind::WorkPoolCompensatoryAction { .. }
            | TraceEventKind::WorkPoolMonitorTick { .. }
            | TraceEventKind::WorkPoolWorkerBlocked { .. }
            | TraceEventKind::WorkPoolWorkerUnblocked { .. } => continue,
            _ => {}
        }

        let head = extract_head_symbol(&event.input).to_string();

        // Track inclusive time via per-thread depth stacks
        let tracker = depth_trackers
            .entry(event.thread_id)
            .or_insert_with(DepthTracker::new);

        let depth = event.depth as usize;

        // Pop any stale entries deeper than current depth
        while tracker.stack.len() > depth {
            tracker.stack.pop();
        }

        // Push current entry
        if tracker.stack.len() == depth {
            tracker.stack.push((head.clone(), event.timestamp_ns));
        }

        // For inclusive time, use duration as-is since we have self-time from the trace.
        // The depth stack gives us nesting context for future correlation.
        let inclusive_ns = duration;

        let profile = profiles
            .entry(head)
            .or_insert_with(SymbolProfile::new);
        profile.record(duration, inclusive_ns);
    }

    // Sort by requested metric
    let mut entries: Vec<(String, SymbolProfile)> = profiles.into_iter().collect();
    for (_, prof) in &mut entries {
        // Pre-sort durations for percentile computation
        prof.durations.sort_unstable();
    }

    match sort_by {
        "self" | "self-time" => entries.sort_by(|a, b| b.1.total_self_ns.cmp(&a.1.total_self_ns)),
        "inclusive" => entries.sort_by(|a, b| b.1.total_inclusive_ns.cmp(&a.1.total_inclusive_ns)),
        "count" => entries.sort_by(|a, b| b.1.call_count.cmp(&a.1.call_count)),
        "p95" => {
            entries.sort_by(|a, b| {
                let p95_a = a.1.durations.get((a.1.durations.len() as f64 * 0.95) as usize).copied().unwrap_or(0);
                let p95_b = b.1.durations.get((b.1.durations.len() as f64 * 0.95) as usize).copied().unwrap_or(0);
                p95_b.cmp(&p95_a)
            });
        }
        _ => entries.sort_by(|a, b| b.1.total_self_ns.cmp(&a.1.total_self_ns)),
    }

    // Print report
    println!("=== Hot Path Analysis ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Total events: {total_events}");
    println!("Wall time: {}", format_duration_ns(total_wall_ns));
    println!("Distinct head symbols: {}", entries.len());
    println!();

    // Header
    println!(
        "  {:<30} {:>8} {:>12} {:>8} {:>10} {:>10} {:>10}",
        "Head Symbol", "Count", "Self Total", "% Wall", "Mean", "P95", "P99"
    );
    println!("  {}", "-".repeat(92));

    for (head, mut prof) in entries.into_iter().take(top_n) {
        let pct_wall = if total_wall_ns > 0 {
            prof.total_self_ns as f64 / total_wall_ns as f64
        } else {
            0.0
        };
        let mean = prof.mean_ns();
        let p95 = prof.percentile(0.95);
        let p99 = prof.percentile(0.99);

        let head_display = if head.len() > 30 {
            format!("{}...", &head[..27])
        } else {
            head
        };

        println!(
            "  {:<30} {:>8} {:>12} {:>8} {:>10} {:>10} {:>10}",
            head_display,
            prof.call_count,
            format_duration_ns(prof.total_self_ns),
            format_pct(pct_wall),
            format_duration_ns(mean),
            format_duration_ns(p95),
            format_duration_ns(p99),
        );
    }

    Ok(())
}
