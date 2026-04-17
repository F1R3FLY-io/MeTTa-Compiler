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

    // Duration statistics (format v2)
    let mut timed_events = 0u64;
    let mut duration_by_kind: HashMap<String, Vec<u64>> = HashMap::new();
    let mut total_gc_pause_ns = 0u64;
    let mut span_count = 0u64;
    let mut total_wall_ns = 0u64; // max timestamp across all events

    for event in reader.events() {
        total_events += 1;

        if event.timestamp_ns + event.duration_ns.unwrap_or(0) > total_wall_ns {
            total_wall_ns = event.timestamp_ns + event.duration_ns.unwrap_or(0);
        }

        let tier_name = match event.tier {
            TraceTier::TreeWalker => "TreeWalker",
            TraceTier::BytecodeVM => "BytecodeVM",
            TraceTier::JitStage1 => "JitStage1",
            TraceTier::JitStage2 => "JitStage2",
        };
        *by_tier.entry(tier_name.to_string()).or_default() += 1;

        let kind_name = kind_label(&event.kind);
        *by_kind.entry(kind_name.to_string()).or_default() += 1;

        // Collect duration data
        if let Some(dur) = event.duration_ns {
            timed_events += 1;
            duration_by_kind.entry(kind_name.to_string())
                .or_default()
                .push(dur);

            if matches!(event.kind, TraceEventKind::GcSafepoint { .. }) {
                total_gc_pause_ns += dur;
            }
        }

        if event.span_id.is_some() {
            span_count += 1;
        }

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
            | TraceEventKind::WorkPoolWorkerResumed { .. }
            | TraceEventKind::WorkPoolBlockedWorkersDetected { .. }
            | TraceEventKind::WorkPoolCompensatoryAction { .. }
            | TraceEventKind::WorkPoolMonitorTick { .. }
            | TraceEventKind::WorkPoolWorkerBlocked { .. }
            | TraceEventKind::WorkPoolWorkerUnblocked { .. } => {
                workpool_event_count += 1;
            }
            _ => {}
        }
    }

    println!("=== MeTTaTron Trace Statistics ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Version: {}", reader.header.mettatron_version);
    println!("Format: v{}", reader.format_version);
    println!("Total events: {total_events}");
    println!("Timed events: {timed_events}");
    println!("Span-correlated events: {span_count}");
    println!("Max eval depth: {max_depth}");
    println!("Errors: {error_count}");
    println!("Bailouts: {bailout_count}");
    println!("GC safepoints: {gc_safepoint_count}");
    if total_gc_pause_ns > 0 {
        println!("GC total pause: {:.3}ms", total_gc_pause_ns as f64 / 1_000_000.0);
    }
    println!("Work pool events: {workpool_event_count}");
    println!("Wall time: {:.3}ms", total_wall_ns as f64 / 1_000_000.0);

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

    // Duration statistics by kind
    if !duration_by_kind.is_empty() {
        println!();
        println!("--- Duration Statistics by Kind ---");
        let mut dur_entries: Vec<_> = duration_by_kind.into_iter().collect();
        // Sort by total time descending
        dur_entries.sort_by(|a, b| {
            let sum_b: u64 = b.1.iter().sum();
            let sum_a: u64 = a.1.iter().sum();
            sum_b.cmp(&sum_a)
        });
        println!("  {:<30} {:>6} {:>12} {:>12} {:>12} {:>12} {:>12}",
                 "Kind", "Count", "Total", "Mean", "Median", "P95", "P99");
        for (kind, mut durations) in dur_entries {
            let n = durations.len();
            let total: u64 = durations.iter().sum();
            let mean = total / n as u64;
            durations.sort_unstable();
            let median = durations[n / 2];
            let p95 = durations[(n as f64 * 0.95) as usize];
            let p99 = durations[(n as f64 * 0.99) as usize];
            println!("  {:<30} {:>6} {:>10}ns {:>10}ns {:>10}ns {:>10}ns {:>10}ns",
                     kind, n, total, mean, median, p95, p99);
        }
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
        TraceEventKind::WorkPoolBlockedWorkersDetected { .. } => "WorkPoolBlockedWorkersDetected",
        TraceEventKind::WorkPoolCompensatoryAction { .. } => "WorkPoolCompensatoryAction",
        TraceEventKind::WorkPoolMonitorTick { .. } => "WorkPoolMonitorTick",
        TraceEventKind::WorkPoolWorkerBlocked { .. } => "WorkPoolWorkerBlocked",
        TraceEventKind::WorkPoolWorkerUnblocked { .. } => "WorkPoolWorkerUnblocked",
        TraceEventKind::LetBindingStep { .. } => "LetBindingStep",
        TraceEventKind::ArgumentPreEvalResult { .. } => "ArgumentPreEvalResult",
        TraceEventKind::TablingDecision { .. } => "TablingDecision",
        TraceEventKind::BindingsApplied { .. } => "BindingsApplied",
        TraceEventKind::RuleSelected { .. } => "RuleSelected",
        TraceEventKind::RuleMatchAttempt { .. } => "RuleMatchAttempt",
        TraceEventKind::RuleLookup { .. } => "RuleLookup",
        TraceEventKind::RuleIndexInsert { .. } => "RuleIndexInsert",
        TraceEventKind::SelfEvaluating { .. } => "SelfEvaluating",
        TraceEventKind::ParallelDispatch { .. } => "ParallelDispatch",
        TraceEventKind::TrampolineStep { .. } => "TrampolineStep",
        TraceEventKind::ContinuationEnter { .. } => "ContinuationEnter",
        TraceEventKind::ContinuationEmit { .. } => "ContinuationEmit",
        TraceEventKind::ContinuationExitNoResume { .. } => "ContinuationExitNoResume",
        TraceEventKind::BindingsDropped { .. } => "BindingsDropped",
    }
}
