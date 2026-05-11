//! Dedicated WorkPool scaling analysis report.
//!
//! Produces a comprehensive report from a single sweep over the trace file,
//! covering scaling actions, objective decomposition, hill climber behavior,
//! memory pressure correlation, throughput efficiency, blocked worker episodes,
//! and overflow pool usage.

use std::collections::HashMap;

use trace_format::TraceEventKind;

use crate::reader::TraceReader;

// ── Data collection ─────────────────────────────────────────────────────────

struct ScaleRecord {
    timestamp_ns: u64,
    action: String,
    active_workers_after: u32,
    min_workers: u32,
    max_workers: u32,
    queue_depth: u32,
    ema_throughput: f64,
    objective: f64,
    emergency: bool,
    hc_direction: i32,
    hc_cooldown_remaining: u32,
    hc_improvement: f64,
    term_throughput: f64,
    term_queue_depth: f64,
    term_slab_pressure: f64,
    term_rss_pressure: f64,
    bp_level: u32,
    blocked_worker_count: u32,
    overflow_count: u32,
    decision_phase: String,
    delta_evals: u64,
    elapsed_seconds: f64,
}

struct BlockedRecord {
    timestamp_ns: u64,
    blocked_count: u32,
    active_workers: u32,
}

struct CompensatoryRecord {
    timestamp_ns: u64,
    core_unparked: u32,
    overflow_spawned: u32,
    overflow_drained: u32,
    deficit: u32,
    rss_veto: bool,
}

struct WorkpoolData {
    scale_events: Vec<ScaleRecord>,
    blocked_events: Vec<BlockedRecord>,
    compensatory_events: Vec<CompensatoryRecord>,
}

impl WorkpoolData {
    fn new() -> Self {
        Self {
            scale_events: Vec::new(),
            blocked_events: Vec::new(),
            compensatory_events: Vec::new(),
        }
    }

    fn collect(&mut self, event: &trace_format::TraceEvent) {
        match &event.kind {
            TraceEventKind::WorkPoolScaleEvent {
                action,
                active_workers_after,
                min_workers,
                max_workers,
                queue_depth,
                ema_throughput,
                objective,
                emergency,
                hc_direction,
                hc_cooldown_remaining,
                hc_improvement,
                term_throughput,
                term_queue_depth,
                term_slab_pressure,
                term_rss_pressure,
                bp_level,
                blocked_worker_count,
                overflow_count,
                decision_phase,
                delta_evals,
                elapsed_seconds,
                ..
            } => {
                self.scale_events.push(ScaleRecord {
                    timestamp_ns: event.timestamp_ns,
                    action: action.clone(),
                    active_workers_after: *active_workers_after,
                    min_workers: *min_workers,
                    max_workers: *max_workers,
                    queue_depth: *queue_depth,
                    ema_throughput: *ema_throughput,
                    objective: *objective,
                    emergency: *emergency,
                    hc_direction: *hc_direction,
                    hc_cooldown_remaining: *hc_cooldown_remaining,
                    hc_improvement: *hc_improvement,
                    term_throughput: *term_throughput,
                    term_queue_depth: *term_queue_depth,
                    term_slab_pressure: *term_slab_pressure,
                    term_rss_pressure: *term_rss_pressure,
                    bp_level: *bp_level,
                    blocked_worker_count: *blocked_worker_count,
                    overflow_count: *overflow_count,
                    decision_phase: decision_phase.clone(),
                    delta_evals: *delta_evals,
                    elapsed_seconds: *elapsed_seconds,
                });
            }
            TraceEventKind::WorkPoolBlockedWorkersDetected {
                blocked_count,
                active_workers,
                ..
            } => {
                self.blocked_events.push(BlockedRecord {
                    timestamp_ns: event.timestamp_ns,
                    blocked_count: *blocked_count,
                    active_workers: *active_workers,
                });
            }
            TraceEventKind::WorkPoolCompensatoryAction {
                core_unparked,
                overflow_spawned,
                overflow_drained,
                deficit,
                rss_veto,
                ..
            } => {
                self.compensatory_events.push(CompensatoryRecord {
                    timestamp_ns: event.timestamp_ns,
                    core_unparked: *core_unparked,
                    overflow_spawned: *overflow_spawned,
                    overflow_drained: *overflow_drained,
                    deficit: *deficit,
                    rss_veto: *rss_veto,
                });
            }
            _ => {}
        }
    }
}

// ── Report generation ───────────────────────────────────────────────────────

fn print_report(data: &WorkpoolData) {
    let events = &data.scale_events;
    if events.is_empty() {
        println!("No WorkPoolScaleEvent events found in trace.");
        return;
    }

    let total = events.len();

    // 1. Scaling Action Summary
    println!("=== 1. Scaling Action Summary ===");
    println!();
    let mut action_counts: HashMap<&str, usize> = HashMap::new();
    for e in events {
        *action_counts.entry(e.action.as_str()).or_insert(0) += 1;
    }
    for action in &["unpark", "park", "hold"] {
        let count = action_counts.get(action).copied().unwrap_or(0);
        let pct = 100.0 * count as f64 / total as f64;
        println!("  {:<16} {:>6} ({:.1}%)", action, count, pct);
    }
    println!("  {:<16} {:>6}", "total ticks", total);
    println!();

    // 2. Phase Decision Breakdown
    println!("=== 2. Phase Decision Breakdown ===");
    println!();
    let mut phase_counts: HashMap<&str, usize> = HashMap::new();
    for e in events {
        *phase_counts.entry(e.decision_phase.as_str()).or_insert(0) += 1;
    }
    for (phase, count) in &phase_counts {
        let pct = 100.0 * *count as f64 / total as f64;
        println!("  {:<24} {:>6} ({:.1}%)", phase, count, pct);
    }
    let emergency_count = events.iter().filter(|e| e.emergency).count();
    let emergency_pct = 100.0 * emergency_count as f64 / total as f64;
    println!(
        "  Emergency ticks: {} ({:.1}%)",
        emergency_count, emergency_pct
    );
    println!();

    // 3. Active Worker Trajectory
    println!("=== 3. Active Worker Trajectory ===");
    println!();
    let workers: Vec<u32> = events.iter().map(|e| e.active_workers_after).collect();
    let min_w = workers.iter().min().copied().unwrap_or(0);
    let max_w = workers.iter().max().copied().unwrap_or(0);
    let mean_w = workers.iter().map(|w| *w as f64).sum::<f64>() / workers.len() as f64;
    let min_bound = events[0].min_workers;
    let max_bound = events[0].max_workers;

    println!(
        "  Active workers: min={}, max={}, mean={:.1}",
        min_w, max_w, mean_w
    );
    println!("  Bounds: [{}, {}]", min_bound, max_bound);

    let at_floor = workers.iter().filter(|w| **w == min_bound).count();
    let at_ceiling = workers.iter().filter(|w| **w == max_bound).count();
    println!(
        "  Time at floor ({}): {} ticks ({:.1}%)",
        min_bound,
        at_floor,
        100.0 * at_floor as f64 / total as f64
    );
    println!(
        "  Time at ceiling ({}): {} ticks ({:.1}%)",
        max_bound,
        at_ceiling,
        100.0 * at_ceiling as f64 / total as f64
    );

    // ASCII sparkline (50 chars wide)
    let sparkline = make_sparkline(&workers, 50, min_bound, max_bound);
    println!("  Sparkline: {}", sparkline);
    println!();

    // 4. Objective Decomposition
    println!("=== 4. Objective Decomposition ===");
    println!();
    let non_emergency: Vec<&ScaleRecord> = events.iter().filter(|e| !e.emergency).collect();
    if non_emergency.is_empty() {
        println!("  All ticks were emergency overrides — no hill climber objective data.");
    } else {
        let terms = ["throughput", "queue_depth", "slab_pressure", "rss_pressure"];
        let extractors: Vec<Box<dyn Fn(&ScaleRecord) -> f64>> = vec![
            Box::new(|e: &ScaleRecord| e.term_throughput.abs()),
            Box::new(|e: &ScaleRecord| e.term_queue_depth),
            Box::new(|e: &ScaleRecord| e.term_slab_pressure),
            Box::new(|e: &ScaleRecord| e.term_rss_pressure),
        ];

        println!(
            "  {:>16}  {:>10}  {:>10}  {:>10}",
            "term", "mean", "max", "dominates"
        );
        for (i, term) in terms.iter().enumerate() {
            let values: Vec<f64> = non_emergency.iter().map(|e| (extractors[i])(e)).collect();
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let max = values.iter().cloned().fold(0.0_f64, f64::max);

            // Count ticks where this term is the largest contributor
            let dominant_count = non_emergency
                .iter()
                .filter(|e| {
                    let v = (extractors[i])(e);
                    extractors
                        .iter()
                        .enumerate()
                        .all(|(j, f)| j == i || v >= f(e))
                })
                .count();
            let dom_pct = 100.0 * dominant_count as f64 / non_emergency.len() as f64;

            println!(
                "  {:>16}  {:>10.4}  {:>10.4}  {:>6} ({:.0}%)",
                term, mean, max, dominant_count, dom_pct
            );
        }

        let obj_values: Vec<f64> = non_emergency.iter().map(|e| e.objective).collect();
        let obj_mean = obj_values.iter().sum::<f64>() / obj_values.len() as f64;
        let obj_min = obj_values.iter().cloned().fold(f64::INFINITY, f64::min);
        let obj_max = obj_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        println!();
        println!(
            "  Objective J(N): mean={:.4}, min={:.4}, max={:.4}",
            obj_mean, obj_min, obj_max
        );
    }
    println!();

    // 5. Hill Climber State Analysis
    println!("=== 5. Hill Climber State Analysis ===");
    println!();
    let mut direction_reversals = 0;
    for pair in events.windows(2) {
        if pair[0].hc_direction != pair[1].hc_direction {
            direction_reversals += 1;
        }
    }
    println!("  Direction reversals: {}", direction_reversals);

    let cooldown_ticks = events
        .iter()
        .filter(|e| e.hc_cooldown_remaining > 0)
        .count();
    let cooldown_pct = 100.0 * cooldown_ticks as f64 / total as f64;
    println!(
        "  Ticks in cooldown: {} ({:.1}%)",
        cooldown_ticks, cooldown_pct
    );

    // Improvement distribution
    let improvements: Vec<f64> = non_emergency.iter().map(|e| e.hc_improvement).collect();
    if !improvements.is_empty() {
        let threshold = 0.05; // WORK_IMPROVEMENT_THRESHOLD
        let genuine = improvements.iter().filter(|i| **i >= threshold).count();
        let worsening = improvements.iter().filter(|i| **i <= -threshold).count();
        let dead_zone = improvements.iter().filter(|i| i.abs() < threshold).count();
        println!(
            "  Improvement: genuine={}, worsening={}, dead_zone={}",
            genuine, worsening, dead_zone
        );
    }
    println!();

    // 6. Memory Pressure Timeline
    println!("=== 6. Memory Pressure Timeline ===");
    println!();
    let bp_events: Vec<_> = events.iter().filter(|e| e.bp_level > 0).collect();
    if bp_events.is_empty() {
        println!("  No backpressure events (bp_level always 0).");
    } else {
        let first_bp = bp_events[0];
        let first_bp_ms = first_bp.timestamp_ns as f64 / 1_000_000.0;
        println!(
            "  First bp_level > 0 at {:.2}ms (level={})",
            first_bp_ms, first_bp.bp_level
        );

        // Find first park response after first bp
        let first_park_after_bp = events
            .iter()
            .find(|e| e.timestamp_ns >= first_bp.timestamp_ns && e.action == "park");
        if let Some(park) = first_park_after_bp {
            let latency_ms = (park.timestamp_ns - first_bp.timestamp_ns) as f64 / 1_000_000.0;
            println!("  Time to first park response: {:.2}ms", latency_ms);
        }

        let max_bp: u32 = events.iter().map(|e| e.bp_level).max().unwrap_or(0);
        let bp_ticks: usize = events.iter().filter(|e| e.bp_level > 0).count();
        println!("  Max bp_level: {}", max_bp);
        println!(
            "  Ticks with bp > 0: {} ({:.1}%)",
            bp_ticks,
            100.0 * bp_ticks as f64 / total as f64
        );
    }
    println!();

    // 7. Throughput Efficiency
    println!("=== 7. Throughput Efficiency ===");
    println!();
    let efficiency: Vec<(u32, f64)> = events
        .iter()
        .filter(|e| e.active_workers_after > 0 && e.ema_throughput > 0.0)
        .map(|e| {
            (
                e.active_workers_after,
                e.ema_throughput / e.active_workers_after as f64,
            )
        })
        .collect();
    if efficiency.is_empty() {
        println!("  No throughput data available.");
    } else {
        // Group by worker count
        let mut by_workers: HashMap<u32, Vec<f64>> = HashMap::new();
        for (w, eff) in &efficiency {
            by_workers.entry(*w).or_default().push(*eff);
        }
        let mut sorted: Vec<(u32, f64, usize)> = by_workers
            .iter()
            .map(|(w, effs)| (*w, effs.iter().sum::<f64>() / effs.len() as f64, effs.len()))
            .collect();
        sorted.sort_by_key(|(w, _, _)| *w);

        println!("  {:>8}  {:>14}  {:>8}", "workers", "tp/worker", "samples");
        for (w, eff, n) in &sorted {
            println!("  {:>8}  {:>14.2}  {:>8}", w, eff, n);
        }

        // Detect diminishing returns
        if sorted.len() >= 2 {
            let first_eff = sorted[0].1;
            let last_eff = sorted.last().expect("sorted must be non-empty").1;
            if last_eff < first_eff * 0.5 {
                println!();
                println!("  ** Diminishing returns detected: efficiency dropped from {:.2} to {:.2} ({:.0}% decrease)",
                         first_eff, last_eff, 100.0 * (1.0 - last_eff / first_eff));
            }
        }

        // Latency-throughput scatter (Part 8)
        println!();
        println!("  Scaling curve (workers vs total ema_throughput):");
        let mut scatter: HashMap<u32, Vec<f64>> = HashMap::new();
        for e in events.iter().filter(|e| e.active_workers_after > 0) {
            scatter
                .entry(e.active_workers_after)
                .or_default()
                .push(e.ema_throughput);
        }
        let mut scatter_sorted: Vec<(u32, f64)> = scatter
            .iter()
            .map(|(w, tps)| (*w, tps.iter().sum::<f64>() / tps.len() as f64))
            .collect();
        scatter_sorted.sort_by_key(|(w, _)| *w);
        println!("  {:>8}  {:>14}", "workers", "mean_tp");
        for (w, tp) in &scatter_sorted {
            println!("  {:>8}  {:>14.2}", w, tp);
        }
    }
    println!();

    // 8. Blocked Worker Analysis
    println!("=== 8. Blocked Worker Analysis ===");
    println!();
    if data.blocked_events.is_empty() {
        println!("  No blocked worker events detected.");
    } else {
        let total_blocked = data.blocked_events.len();
        let max_blocked: u32 = data
            .blocked_events
            .iter()
            .map(|e| e.blocked_count)
            .max()
            .unwrap_or(0);
        let mean_blocked: f64 = data
            .blocked_events
            .iter()
            .map(|e| e.blocked_count as f64)
            .sum::<f64>()
            / total_blocked as f64;
        println!("  Blocked worker detection events: {}", total_blocked);
        println!("  Max blocked at once: {}", max_blocked);
        println!("  Mean blocked count: {:.1}", mean_blocked);

        // Episode detection: consecutive blocked events
        let mut episodes = 0;
        let mut in_episode = false;
        for e in &data.blocked_events {
            if e.blocked_count > 0 && !in_episode {
                episodes += 1;
                in_episode = true;
            } else if e.blocked_count == 0 {
                in_episode = false;
            }
        }
        println!("  Blocking episodes: {}", episodes);

        // Correlation with compensatory actions
        let comp_after_blocked = data
            .compensatory_events
            .iter()
            .filter(|c| {
                data.blocked_events.iter().any(|b| {
                    c.timestamp_ns >= b.timestamp_ns
                        && c.timestamp_ns - b.timestamp_ns < 500_000_000
                })
            })
            .count();
        println!(
            "  Compensatory actions correlated with blocking: {}",
            comp_after_blocked
        );
    }
    println!();

    // 9. Overflow Pool Usage
    println!("=== 9. Overflow Pool Usage ===");
    println!();
    let total_overflow_spawned: u32 = data
        .compensatory_events
        .iter()
        .map(|c| c.overflow_spawned)
        .sum();
    let total_overflow_drained: u32 = data
        .compensatory_events
        .iter()
        .map(|c| c.overflow_drained)
        .sum();
    let max_overflow: u32 = events.iter().map(|e| e.overflow_count).max().unwrap_or(0);
    let overflow_ticks = events.iter().filter(|e| e.overflow_count > 0).count();
    let rss_veto_count = data
        .compensatory_events
        .iter()
        .filter(|c| c.rss_veto)
        .count();

    println!("  Total overflow spawned: {}", total_overflow_spawned);
    println!("  Total overflow drained: {}", total_overflow_drained);
    println!("  Max concurrent overflow: {}", max_overflow);
    println!(
        "  Ticks with overflow > 0: {} ({:.1}%)",
        overflow_ticks,
        100.0 * overflow_ticks as f64 / total as f64
    );
    println!("  RSS vetoes: {}", rss_veto_count);
    println!();

    // 10. Queue Drain Rate Analysis (Part 8)
    println!("=== 10. Queue Drain Rate Analysis ===");
    println!();
    let mut drain_episodes: Vec<(u32, u32, f64)> = Vec::new(); // (start_depth, workers, drain_rate)
    let mut queue_was_nonzero = false;
    let mut queue_start_depth: u32 = 0;
    let mut queue_start_tick: usize = 0;

    for (i, e) in events.iter().enumerate() {
        if e.queue_depth > 0 && !queue_was_nonzero {
            queue_was_nonzero = true;
            queue_start_depth = e.queue_depth;
            queue_start_tick = i;
        } else if e.queue_depth == 0 && queue_was_nonzero {
            queue_was_nonzero = false;
            let drain_ticks = (i - queue_start_tick) as f64;
            if drain_ticks > 0.0 {
                let avg_workers = events[queue_start_tick..i]
                    .iter()
                    .map(|e| e.active_workers_after as f64)
                    .sum::<f64>()
                    / drain_ticks;
                let drain_rate = queue_start_depth as f64 / drain_ticks;
                let per_worker = if avg_workers > 0.0 {
                    drain_rate / avg_workers
                } else {
                    0.0
                };
                drain_episodes.push((queue_start_depth, avg_workers as u32, per_worker));
            }
        }
    }

    if drain_episodes.is_empty() {
        println!("  No complete queue drain episodes detected.");
    } else {
        println!("  Queue drain episodes: {}", drain_episodes.len());
        println!(
            "  {:>12}  {:>10}  {:>14}",
            "start_depth", "workers", "drain/worker/tick"
        );
        for (depth, workers, rate) in &drain_episodes {
            println!("  {:>12}  {:>10}  {:>14.3}", depth, workers, rate);
        }

        // Check if drain rate improves with more workers
        if drain_episodes.len() >= 2 {
            let mut by_workers: HashMap<u32, Vec<f64>> = HashMap::new();
            for (_, w, rate) in &drain_episodes {
                by_workers.entry(*w).or_default().push(*rate);
            }
            let mut sorted: Vec<(u32, f64)> = by_workers
                .iter()
                .map(|(w, rates)| (*w, rates.iter().sum::<f64>() / rates.len() as f64))
                .collect();
            sorted.sort_by_key(|(w, _)| *w);
            if sorted.len() >= 2 {
                let first = sorted[0].1;
                let last = sorted.last().expect("sorted must be non-empty").1;
                if last <= first {
                    println!();
                    println!("  ** Drain rate does NOT improve with more workers (parallelism bottleneck)");
                }
            }
        }
    }
}

// ── Sparkline helper ────────────────────────────────────────────────────────

fn make_sparkline(values: &[u32], width: usize, min_val: u32, max_val: u32) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }

    let blocks = [
        ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];

    // If all values are the same, show a flat line
    let range = if max_val == min_val {
        1
    } else {
        max_val - min_val
    };

    // Sample values to fit within width
    let step = values.len().max(1) as f64 / width as f64;
    let mut result = String::with_capacity(width * 4);

    for i in 0..width {
        let idx = (i as f64 * step) as usize;
        if idx >= values.len() {
            break;
        }
        let val = values[idx];
        let normalized = ((val - min_val) as f64 / range as f64 * 8.0).round() as usize;
        let block_idx = normalized.min(8);
        result.push(blocks[block_idx]);
    }

    result
}

// ── Entry point ─────────────────────────────────────────────────────────────

pub fn run(file: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    println!("=== WorkPool Scaling Analysis ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Format: v{}", reader.format_version);
    println!();

    let mut data = WorkpoolData::new();

    for event in reader.events() {
        data.collect(&event);
    }

    print_report(&data);

    Ok(())
}
