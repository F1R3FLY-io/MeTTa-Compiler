//! Summary statistics, histograms, and hot expression ranking.

use std::collections::HashMap;

use crate::reader::TraceReader;
use trace_format::{TraceEventKind, TraceTier};

pub fn run(file: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    let mut total_events = 0u64;
    let mut by_tier: HashMap<String, u64> = HashMap::new();
    let mut by_kind: HashMap<String, u64> = HashMap::new();
    let mut depth_histogram: HashMap<u32, u64> = HashMap::new();
    let mut max_depth = 0u32;
    let mut error_count = 0u64;
    let mut bailout_count = 0u64;
    let mut gc_safepoint_count = 0u64;
    let mut workpool_event_count = 0u64;

    for event in reader.events() {
        total_events += 1;

        let tier_name = match event.tier {
            TraceTier::TreeWalker => "TreeWalker",
            TraceTier::BytecodeVM => "BytecodeVM",
            TraceTier::JitStage1 => "JitStage1",
            TraceTier::JitStage2 => "JitStage2",
        };
        *by_tier.entry(tier_name.to_string()).or_default() += 1;

        let kind_name = kind_label(&event.kind);
        *by_kind.entry(kind_name.to_string()).or_default() += 1;

        *depth_histogram.entry(event.depth).or_default() += 1;
        if event.depth > max_depth {
            max_depth = event.depth;
        }

        match &event.kind {
            TraceEventKind::ErrorCreated { .. }
            | TraceEventKind::GroundedOpError { .. }
            | TraceEventKind::ErrorPropagated { .. } => {
                error_count += 1;
            }
            TraceEventKind::JitBailout { .. } | TraceEventKind::BytecodeHalt { .. } => {
                bailout_count += 1;
            }
            TraceEventKind::GcSafepoint { .. } => {
                gc_safepoint_count += 1;
            }
            TraceEventKind::WorkPoolTaskEnqueued { .. }
            | TraceEventKind::WorkPoolTaskDropped { .. }
            | TraceEventKind::WorkPoolTaskCompleted { .. }
            | TraceEventKind::WorkPoolScaleEvent { .. }
            | TraceEventKind::WorkPoolWorkerParked { .. }
            | TraceEventKind::WorkPoolWorkerResumed { .. } => {
                workpool_event_count += 1;
            }
            _ => {}
        }
    }

    println!("=== MeTTaTron Trace Statistics ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Version: {}", reader.header.mettatron_version);
    println!("Total events: {total_events}");
    println!("Max eval depth: {max_depth}");
    println!("Errors: {error_count}");
    println!("Bailouts: {bailout_count}");
    println!("GC safepoints: {gc_safepoint_count}");
    println!("Work pool events: {workpool_event_count}");

    println!();
    println!("--- Events by Tier ---");
    let mut tier_vec: Vec<_> = by_tier.into_iter().collect();
    tier_vec.sort_by(|a, b| b.1.cmp(&a.1));
    for (tier, count) in &tier_vec {
        let pct = (*count as f64 / total_events as f64) * 100.0;
        println!("  {:<14} {:>8}  ({:.1}%)", tier, count, pct);
    }

    println!();
    println!("--- Events by Kind ---");
    let mut kind_vec: Vec<_> = by_kind.into_iter().collect();
    kind_vec.sort_by(|a, b| b.1.cmp(&a.1));
    for (kind, count) in kind_vec.iter().take(20) {
        let pct = (*count as f64 / total_events as f64) * 100.0;
        println!("  {:<30} {:>8}  ({:.1}%)", kind, count, pct);
    }

    println!();
    println!("--- Depth Histogram ---");
    let mut depth_vec: Vec<_> = depth_histogram.into_iter().collect();
    depth_vec.sort_by_key(|&(d, _)| d);
    let max_count = depth_vec.iter().map(|(_, c)| *c).max().unwrap_or(1);
    for (depth, count) in &depth_vec {
        let bar_width = ((*count as f64 / max_count as f64) * 40.0) as usize;
        let bar: String = "\u{2588}".repeat(bar_width);
        println!("  D{:<4} {:>8}  {}", depth, count, bar);
    }

    Ok(())
}

fn kind_label(kind: &TraceEventKind) -> &'static str {
    match kind {
        TraceEventKind::RuleApplication { .. } => "RuleApplication",
        TraceEventKind::GroundedOp { .. } => "GroundedOp",
        TraceEventKind::SpecialForm { .. } => "SpecialForm",
        TraceEventKind::PatternMatch { .. } => "PatternMatch",
        TraceEventKind::RuleMatchSet { .. } => "RuleMatchSet",
        TraceEventKind::TypeOperation { .. } => "TypeOperation",
        TraceEventKind::ApplicativePreEval { .. } => "ApplicativePreEval",
        TraceEventKind::BranchPrune { .. } => "BranchPrune",
        TraceEventKind::ErrorCreated { .. } => "ErrorCreated",
        TraceEventKind::ErrorCaught { .. } => "ErrorCaught",
        TraceEventKind::ErrorPropagated { .. } => "ErrorPropagated",
        TraceEventKind::GroundedOpError { .. } => "GroundedOpError",
        TraceEventKind::NondeterministicFork { .. } => "NondeterministicFork",
        TraceEventKind::BranchStart { .. } => "BranchStart",
        TraceEventKind::BranchEnd { .. } => "BranchEnd",
        TraceEventKind::TierDispatch { .. } => "TierDispatch",
        TraceEventKind::BytecodeCompilation { .. } => "BytecodeCompilation",
        TraceEventKind::JitCompilation { .. } => "JitCompilation",
        TraceEventKind::JitBailout { .. } => "JitBailout",
        TraceEventKind::BytecodeHalt { .. } => "BytecodeHalt",
        TraceEventKind::EvalStart => "EvalStart",
        TraceEventKind::EvalEnd { .. } => "EvalEnd",
        TraceEventKind::GcSafepoint { .. } => "GcSafepoint",
        TraceEventKind::TypeInference { .. } => "TypeInference",
        TraceEventKind::TypeMatch { .. } => "TypeMatch",
        TraceEventKind::RhsTypeComputed { .. } => "RhsTypeComputed",
        TraceEventKind::InferredTypeRegistered { .. } => "InferredTypeRegistered",
        TraceEventKind::WorkPoolTaskEnqueued { .. } => "WorkPoolTaskEnqueued",
        TraceEventKind::WorkPoolTaskDropped { .. } => "WorkPoolTaskDropped",
        TraceEventKind::WorkPoolTaskCompleted { .. } => "WorkPoolTaskCompleted",
        TraceEventKind::WorkPoolScaleEvent { .. } => "WorkPoolScaleEvent",
        TraceEventKind::WorkPoolWorkerParked { .. } => "WorkPoolWorkerParked",
        TraceEventKind::WorkPoolWorkerResumed { .. } => "WorkPoolWorkerResumed",
    }
}
