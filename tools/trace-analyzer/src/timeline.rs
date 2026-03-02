//! Per-thread activity timeline with horizontal Gantt bars.
//!
//! Extracts timed events (those with `duration_ns`) from a trace file and
//! visualizes them as a per-thread timeline using Unicode block characters.

use std::collections::HashMap;

use crate::reader::TraceReader;
use trace_format::TraceEventKind;

/// A time-bounded activity span on a single thread.
struct Activity {
    thread_id: u32,
    start_ns: u64,
    end_ns: u64,
    label: String,
    #[allow(dead_code)]
    depth: u32,
}

fn kind_label(kind: &TraceEventKind) -> String {
    match kind {
        TraceEventKind::EvalStart => "EvalStart".to_string(),
        TraceEventKind::EvalEnd { .. } => "EvalEnd".to_string(),
        TraceEventKind::GroundedOp { op_name, .. } => format!("GroundedOp:{op_name}"),
        TraceEventKind::SpecialForm { form_name, phase } => format!("SpecialForm:{form_name}:{phase}"),
        TraceEventKind::GcSafepoint { .. } => "GcSafepoint".to_string(),
        TraceEventKind::BranchStart { branch_index, .. } => format!("Branch[{branch_index}]"),
        TraceEventKind::BranchEnd { branch_index, .. } => format!("BranchEnd[{branch_index}]"),
        TraceEventKind::RuleApplication { .. } => "RuleApplication".to_string(),
        TraceEventKind::WorkPoolTaskCompleted { task_kind, .. } => format!("WorkPool:{task_kind}"),
        _ => "Other".to_string(),
    }
}

pub fn run(file: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    if reader.format_version < 2 {
        eprintln!("Warning: trace file is format v1 — duration data may be absent.");
    }

    // Collect activities from timed events
    let mut activities: Vec<Activity> = Vec::new();
    let mut thread_ids: Vec<u32> = Vec::new();

    for event in reader.events() {
        if let Some(dur) = event.duration_ns {
            if dur == 0 {
                continue;
            }
            activities.push(Activity {
                thread_id: event.thread_id,
                start_ns: event.timestamp_ns,
                end_ns: event.timestamp_ns + dur,
                label: kind_label(&event.kind),
                depth: event.depth,
            });
            if !thread_ids.contains(&event.thread_id) {
                thread_ids.push(event.thread_id);
            }
        }
    }

    if activities.is_empty() {
        println!("No timed events found in trace file.");
        return Ok(());
    }

    thread_ids.sort_unstable();

    // Determine time range
    let min_ns = activities.iter().map(|a| a.start_ns).min().unwrap_or(0);
    let max_ns = activities.iter().map(|a| a.end_ns).max().unwrap_or(0);
    let range_ns = max_ns - min_ns;

    if range_ns == 0 {
        println!("All events have zero duration range.");
        return Ok(());
    }

    println!("=== Per-Thread Activity Timeline ===");
    println!();
    println!("Time range: {:.3}ms - {:.3}ms ({:.3}ms total)",
             min_ns as f64 / 1e6, max_ns as f64 / 1e6, range_ns as f64 / 1e6);
    println!();

    // Terminal width for Gantt bar
    let bar_width: usize = 80;

    // Group activities by thread
    let mut by_thread: HashMap<u32, Vec<&Activity>> = HashMap::new();
    for a in &activities {
        by_thread.entry(a.thread_id).or_default().push(a);
    }

    for &tid in &thread_ids {
        let thread_acts = by_thread.get(&tid).map(|v| v.as_slice()).unwrap_or(&[]);

        // Build a coverage bitmap for this thread
        let mut bar = vec![' '; bar_width];
        for a in thread_acts {
            let start_pos = ((a.start_ns - min_ns) as f64 / range_ns as f64 * bar_width as f64) as usize;
            let end_pos = ((a.end_ns - min_ns) as f64 / range_ns as f64 * bar_width as f64) as usize;
            let start_pos = start_pos.min(bar_width - 1);
            let end_pos = end_pos.min(bar_width).max(start_pos + 1);
            for i in start_pos..end_pos {
                bar[i] = '\u{2588}'; // Full block
            }
        }

        let bar_str: String = bar.into_iter().collect();
        println!("  T{:<3} [{} events] |{}|",
                 tid, thread_acts.len(), bar_str);
    }

    // Summary: top activities by total duration
    println!();
    println!("--- Top Activities by Total Duration ---");
    let mut dur_by_label: HashMap<String, (u64, u64)> = HashMap::new(); // (total_ns, count)
    for a in &activities {
        let entry = dur_by_label.entry(a.label.clone()).or_default();
        entry.0 += a.end_ns - a.start_ns;
        entry.1 += 1;
    }
    let mut dur_vec: Vec<_> = dur_by_label.into_iter().collect();
    dur_vec.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (label, (total, count)) in dur_vec.iter().take(15) {
        println!("  {:<40} {:>6} events  {:>10.3}ms total",
                 label, count, *total as f64 / 1e6);
    }

    let _ = (min_ns, max_ns); // suppress warnings
    Ok(())
}
