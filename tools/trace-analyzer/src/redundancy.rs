//! Memoization opportunity detection.
//!
//! Hashes `(input, outputs)` for each computation event (RuleApplication, GroundedOp).
//! Detects when identical inputs produce identical outputs repeatedly.
//!
//! Algorithm: Single-pass. `HashMap<input_hash, (output_hash, count, total_ns)>` with
//! LRU eviction at 50K entries.

use std::collections::HashMap;

use trace_format::TraceEventKind;

use crate::reader::TraceReader;
use crate::util::{format_duration_ns, hash_trace_value, hash_trace_values};

/// Maximum number of distinct input hashes tracked before LRU eviction.
const MAX_ENTRIES: usize = 50_000;

/// A tracked computation pattern.
struct ComputationEntry {
    /// Hash of the output(s).
    output_hash: u64,
    /// Number of times this exact (input_hash, output_hash) was seen.
    count: u64,
    /// Total duration in nanoseconds.
    total_ns: u64,
    /// Head symbol for display.
    head_symbol: String,
    /// Representative input display string (first occurrence).
    input_display: String,
    /// Last access order (for LRU eviction).
    last_access: u64,
}

pub fn run(file: &str, top_n: usize, min_count: u64) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    let mut entries: HashMap<u64, ComputationEntry> = HashMap::with_capacity(MAX_ENTRIES);
    let mut access_counter: u64 = 0;
    let mut total_events: u64 = 0;
    let mut total_wall_ns: u64 = 0;

    for event in reader.events() {
        total_events += 1;
        let end_ns = event.timestamp_ns + event.duration_ns.unwrap_or(0);
        if end_ns > total_wall_ns {
            total_wall_ns = end_ns;
        }

        // Only track RuleApplication and GroundedOp events
        match &event.kind {
            TraceEventKind::RuleApplication { .. }
            | TraceEventKind::GroundedOp { .. } => {}
            _ => continue,
        }

        let duration = event.duration_ns.unwrap_or(0);
        let input_hash = hash_trace_value(&event.input);
        let output_hash = hash_trace_values(&event.outputs);
        access_counter += 1;

        if let Some(entry) = entries.get_mut(&input_hash) {
            if entry.output_hash == output_hash {
                // Same input, same output — redundant computation
                entry.count += 1;
                entry.total_ns += duration;
                entry.last_access = access_counter;
            }
            // Different output for same input hash: skip (not memoizable)
        } else {
            // LRU eviction if at capacity
            if entries.len() >= MAX_ENTRIES {
                // Find the entry with lowest last_access
                let evict_key = entries
                    .iter()
                    .min_by_key(|(_, e)| e.last_access)
                    .map(|(k, _)| *k);
                if let Some(key) = evict_key {
                    entries.remove(&key);
                }
            }

            let head_symbol = crate::util::extract_head_symbol(&event.input).to_string();
            let input_display = format!("{}", event.input);
            let input_display = if input_display.len() > 60 {
                format!("{}...", &input_display[..57])
            } else {
                input_display
            };

            entries.insert(input_hash, ComputationEntry {
                output_hash,
                count: 1,
                total_ns: duration,
                head_symbol,
                input_display,
                last_access: access_counter,
            });
        }
    }

    // Filter by min_count and compute potential savings
    let mut candidates: Vec<_> = entries
        .into_values()
        .filter(|e| e.count >= min_count)
        .collect();

    // Sort by potential savings: (count - 1) * mean_duration
    candidates.sort_by(|a, b| {
        let savings_a = if a.count > 1 {
            (a.count - 1) * (a.total_ns / a.count)
        } else {
            0
        };
        let savings_b = if b.count > 1 {
            (b.count - 1) * (b.total_ns / b.count)
        } else {
            0
        };
        savings_b.cmp(&savings_a)
    });

    // Print report
    println!("=== Redundancy / Memoization Opportunities ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Total events: {total_events}");
    println!("Wall time: {}", format_duration_ns(total_wall_ns));
    println!("Tracked computations: {}", candidates.len());
    println!("Min count filter: {min_count}");
    println!();

    if candidates.is_empty() {
        println!("  No redundant computations found (min_count={min_count}).");
        return Ok(());
    }

    println!(
        "  {:<30} {:>8} {:>12} {:>10} {:>14}",
        "Head Symbol", "Count", "Total Time", "Mean", "Savings"
    );
    println!("  {}", "-".repeat(78));

    for entry in candidates.iter().take(top_n) {
        let mean_ns = if entry.count > 0 { entry.total_ns / entry.count } else { 0 };
        let savings_ns = if entry.count > 1 {
            (entry.count - 1) * mean_ns
        } else {
            0
        };

        let head_display = if entry.head_symbol.len() > 30 {
            format!("{}...", &entry.head_symbol[..27])
        } else {
            entry.head_symbol.clone()
        };

        println!(
            "  {:<30} {:>8} {:>12} {:>10} {:>14}",
            head_display,
            entry.count,
            format_duration_ns(entry.total_ns),
            format_duration_ns(mean_ns),
            format_duration_ns(savings_ns),
        );
    }

    // Summary
    let total_savings: u64 = candidates.iter().map(|e| {
        let mean_ns = if e.count > 0 { e.total_ns / e.count } else { 0 };
        if e.count > 1 { (e.count - 1) * mean_ns } else { 0 }
    }).sum();

    println!();
    println!("Total potential savings: {}", format_duration_ns(total_savings));
    if total_wall_ns > 0 {
        println!(
            "Savings as % of wall time: {:.1}%",
            total_savings as f64 / total_wall_ns as f64 * 100.0
        );
    }

    Ok(())
}
