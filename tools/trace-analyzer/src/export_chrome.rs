//! Chrome Trace Format (JSON) export.
//!
//! Exports trace events as a Chrome Trace Format JSON file that can be
//! visualized in Perfetto UI (<https://ui.perfetto.dev/>) or `chrome://tracing`.
//!
//! Event mapping:
//! - Timed events → Chrome "X" (complete) events with `ts` (us), `dur` (us)
//! - Point events → Chrome "i" (instant) events
//! - Paired span events → Chrome "B"/"E" (begin/end) correlated by span_id

use std::io::Write;
use std::collections::HashMap;

use crate::reader::TraceReader;
use trace_format::TraceEventKind;

pub fn run(file: &str, output: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    if reader.format_version < 2 {
        return Err("The 'export-chrome' subcommand requires format v2 trace files with duration data. \
                    Re-record the trace with the latest MeTTaTron build.".to_string());
    }

    let out_file = std::fs::File::create(output)
        .map_err(|e| format!("Failed to create output file: {e}"))?;
    let mut writer = std::io::BufWriter::new(out_file);

    write!(writer, "{{\"traceEvents\":[")
        .map_err(|e| format!("Write error: {e}"))?;

    let mut first = true;
    let mut span_starts: HashMap<u64, (u64, u32)> = HashMap::new(); // span_id → (start_ns, tid)

    for event in reader.events() {
        let name = event_name(&event.kind);
        let cat = event_category(&event.kind);
        let tid = event.thread_id;
        let ts_us = event.timestamp_ns as f64 / 1000.0;

        // Determine Chrome event type
        if let Some(dur_ns) = event.duration_ns {
            // Complete event (X)
            let dur_us = dur_ns as f64 / 1000.0;
            if !first { write!(writer, ",").map_err(|e| format!("Write error: {e}"))?; }
            first = false;
            write!(writer,
                   "{{\"ph\":\"X\",\"name\":\"{name}\",\"cat\":\"{cat}\",\"ts\":{ts_us:.3},\"dur\":{dur_us:.3},\"tid\":{tid},\"pid\":1,\"args\":{{\"depth\":{},\"seq\":{}}}}}",
                   event.depth, event.seq)
                .map_err(|e| format!("Write error: {e}"))?;
        } else if let Some(span_id) = event.span_id {
            // Begin event (B) if this is the first time we see this span_id
            if !span_starts.contains_key(&span_id) {
                span_starts.insert(span_id, (event.timestamp_ns, tid));
                if !first { write!(writer, ",").map_err(|e| format!("Write error: {e}"))?; }
                first = false;
                write!(writer,
                       "{{\"ph\":\"B\",\"name\":\"{name}\",\"cat\":\"{cat}\",\"ts\":{ts_us:.3},\"tid\":{tid},\"pid\":1,\"args\":{{\"depth\":{},\"span_id\":{}}}}}",
                       event.depth, span_id)
                    .map_err(|e| format!("Write error: {e}"))?;
            } else {
                // End event (E) — pair with the Begin
                if !first { write!(writer, ",").map_err(|e| format!("Write error: {e}"))?; }
                first = false;
                write!(writer,
                       "{{\"ph\":\"E\",\"name\":\"{name}\",\"cat\":\"{cat}\",\"ts\":{ts_us:.3},\"tid\":{tid},\"pid\":1,\"args\":{{\"depth\":{},\"span_id\":{}}}}}",
                       event.depth, span_id)
                    .map_err(|e| format!("Write error: {e}"))?;
                span_starts.remove(&span_id);
            }
        } else {
            // Instant event (i)
            if !first { write!(writer, ",").map_err(|e| format!("Write error: {e}"))?; }
            first = false;
            write!(writer,
                   "{{\"ph\":\"i\",\"name\":\"{name}\",\"cat\":\"{cat}\",\"ts\":{ts_us:.3},\"tid\":{tid},\"pid\":1,\"s\":\"t\",\"args\":{{\"depth\":{},\"seq\":{}}}}}",
                   event.depth, event.seq)
                .map_err(|e| format!("Write error: {e}"))?;
        }
    }

    write!(writer, "]}}")
        .map_err(|e| format!("Write error: {e}"))?;
    writer.flush()
        .map_err(|e| format!("Flush error: {e}"))?;

    println!("Exported Chrome trace to: {output}");
    Ok(())
}

fn event_name(kind: &TraceEventKind) -> String {
    match kind {
        TraceEventKind::EvalStart => "EvalStart".to_string(),
        TraceEventKind::EvalEnd { .. } => "EvalEnd".to_string(),
        TraceEventKind::GroundedOp { op_name, .. } => format!("GroundedOp:{op_name}"),
        TraceEventKind::SpecialForm { form_name, phase } => format!("{form_name}:{phase}"),
        TraceEventKind::RuleApplication { .. } => "RuleApplication".to_string(),
        TraceEventKind::PatternMatch { .. } => "PatternMatch".to_string(),
        TraceEventKind::RuleMatchSet { match_count, .. } => format!("RuleMatchSet({match_count})"),
        TraceEventKind::TypeOperation { op, .. } => format!("Type:{op}"),
        TraceEventKind::ApplicativePreEval { operator, .. } => format!("ApplicativePreEval:{operator}"),
        TraceEventKind::BranchPrune { .. } => "BranchPrune".to_string(),
        TraceEventKind::ErrorCreated { .. } => "ErrorCreated".to_string(),
        TraceEventKind::ErrorCaught { .. } => "ErrorCaught".to_string(),
        TraceEventKind::ErrorPropagated { .. } => "ErrorPropagated".to_string(),
        TraceEventKind::GroundedOpError { op_name, .. } => format!("GroundedOpError:{op_name}"),
        TraceEventKind::NondeterministicFork { branch_count } => format!("Fork({branch_count})"),
        TraceEventKind::BranchStart { branch_index, .. } => format!("Branch[{branch_index}]"),
        TraceEventKind::BranchEnd { branch_index, .. } => format!("BranchEnd[{branch_index}]"),
        TraceEventKind::TierDispatch { selected_tier, .. } => format!("TierDispatch:{selected_tier}"),
        TraceEventKind::BytecodeCompilation { .. } => "BytecodeCompilation".to_string(),
        TraceEventKind::JitCompilation { stage, .. } => format!("JitCompilation(stage{stage})"),
        TraceEventKind::JitBailout { .. } => "JitBailout".to_string(),
        TraceEventKind::BytecodeHalt { .. } => "BytecodeHalt".to_string(),
        TraceEventKind::GcSafepoint { .. } => "GcSafepoint".to_string(),
        TraceEventKind::TypeInference { .. } => "TypeInference".to_string(),
        TraceEventKind::TypeMatch { .. } => "TypeMatch".to_string(),
        TraceEventKind::RhsTypeComputed { head, .. } => format!("RhsTypeComputed:{head}"),
        TraceEventKind::InferredTypeRegistered { function_name, .. } => format!("InferredType:{function_name}"),
        TraceEventKind::WorkPoolTaskEnqueued { task_kind, .. } => format!("WP:Enqueue:{task_kind}"),
        TraceEventKind::WorkPoolTaskDropped { .. } => "WP:Dropped".to_string(),
        TraceEventKind::WorkPoolTaskCompleted { task_kind, .. } => format!("WP:Complete:{task_kind}"),
        TraceEventKind::WorkPoolScaleEvent { action, .. } => format!("WP:Scale:{action}"),
        TraceEventKind::WorkPoolWorkerParked { worker_id, .. } => format!("WP:Park[{worker_id}]"),
        TraceEventKind::WorkPoolWorkerResumed { worker_id, .. } => format!("WP:Resume[{worker_id}]"),
        TraceEventKind::WorkPoolBlockedWorkersDetected { blocked_count, .. } => format!("WP:Blocked:{blocked_count}"),
        TraceEventKind::WorkPoolCompensatoryAction { .. } => "WP:Compensate".to_string(),
        TraceEventKind::WorkPoolMonitorTick { .. } => "WP:Tick".to_string(),
        TraceEventKind::WorkPoolWorkerBlocked { worker_id, .. } => format!("WP:WorkerBlocked[{worker_id}]"),
        TraceEventKind::WorkPoolWorkerUnblocked { worker_id, .. } => format!("WP:WorkerUnblocked[{worker_id}]"),
        TraceEventKind::LetBindingStep { form, .. } => format!("LetBindingStep:{form}"),
        TraceEventKind::ArgumentPreEvalResult { arg_index, .. } => format!("ArgPreEval[{arg_index}]"),
        TraceEventKind::TablingDecision { decision, .. } => format!("Tabling:{decision}"),
        TraceEventKind::BindingsApplied { .. } => "BindingsApplied".to_string(),
        TraceEventKind::RuleSelected { selected_index, total_matches, .. } => format!("RuleSelected:{selected_index}/{total_matches}"),
        TraceEventKind::RuleMatchAttempt { call_head, matcher, .. } => format!("RuleMatchAttempt:{matcher}:{call_head}"),
        TraceEventKind::RuleLookup { head, .. } => format!("RuleLookup:{head}"),
        TraceEventKind::RuleIndexInsert { head, .. } => format!("RuleIndexInsert:{}", head.as_deref().unwrap_or("*")),
        TraceEventKind::SelfEvaluating { reason, .. } => format!("SelfEvaluating:{reason}"),
        TraceEventKind::ParallelDispatch { branch_count, phase, .. } => format!("ParallelDispatch({branch_count}):{phase}"),
        TraceEventKind::TrampolineStep { work_kind, iteration, .. } => format!("TrampolineStep#{iteration}:{work_kind}"),
        TraceEventKind::ContinuationEnter { cont_kind, flow_id, .. } => format!("ContEnter#{flow_id}:{cont_kind}"),
        TraceEventKind::ContinuationEmit { cont_kind, flow_id, .. } => format!("ContEmit#{flow_id}:{cont_kind}"),
        TraceEventKind::ContinuationExitNoResume { cont_kind, flow_id, exit_kind } => format!("ContExit#{flow_id}:{cont_kind}:{exit_kind}"),
        TraceEventKind::BindingsDropped { cont_kind, flow_id, dropped_keys, .. } => format!("BindingsDropped#{flow_id}:{cont_kind}:{}", dropped_keys.join(",")),
        TraceEventKind::BindingsExtracted { source, head, arity, .. } => format!("BindingsExtracted:{source}:{head}/{arity}"),
        TraceEventKind::BindingsFreshened { source, epoch, .. } => format!("BindingsFreshened:{source}:epoch={epoch}"),
        TraceEventKind::VariableLookupFailed { context, var_name, .. } => format!("VariableLookupFailed:{context}:{var_name}"),
    }
}

fn event_category(kind: &TraceEventKind) -> &'static str {
    match kind {
        TraceEventKind::EvalStart | TraceEventKind::EvalEnd { .. } => "eval",
        TraceEventKind::GroundedOp { .. } | TraceEventKind::GroundedOpError { .. } => "grounded",
        TraceEventKind::SpecialForm { .. } => "eval",
        TraceEventKind::RuleApplication { .. } | TraceEventKind::PatternMatch { .. }
        | TraceEventKind::RuleMatchSet { .. } => "eval",
        TraceEventKind::TypeOperation { .. } | TraceEventKind::TypeInference { .. }
        | TraceEventKind::TypeMatch { .. } | TraceEventKind::RhsTypeComputed { .. }
        | TraceEventKind::InferredTypeRegistered { .. } | TraceEventKind::ApplicativePreEval { .. }
        | TraceEventKind::BranchPrune { .. } => "type",
        TraceEventKind::ErrorCreated { .. } | TraceEventKind::ErrorCaught { .. }
        | TraceEventKind::ErrorPropagated { .. } => "error",
        TraceEventKind::NondeterministicFork { .. } | TraceEventKind::BranchStart { .. }
        | TraceEventKind::BranchEnd { .. } => "nondeterminism",
        TraceEventKind::TierDispatch { .. } | TraceEventKind::BytecodeCompilation { .. }
        | TraceEventKind::JitCompilation { .. } | TraceEventKind::JitBailout { .. }
        | TraceEventKind::BytecodeHalt { .. } => "tier",
        TraceEventKind::GcSafepoint { .. } => "gc",
        TraceEventKind::WorkPoolTaskEnqueued { .. } | TraceEventKind::WorkPoolTaskDropped { .. }
        | TraceEventKind::WorkPoolTaskCompleted { .. } | TraceEventKind::WorkPoolScaleEvent { .. }
        | TraceEventKind::WorkPoolWorkerParked { .. } | TraceEventKind::WorkPoolWorkerResumed { .. }
        | TraceEventKind::WorkPoolBlockedWorkersDetected { .. }
        | TraceEventKind::WorkPoolCompensatoryAction { .. }
        | TraceEventKind::WorkPoolMonitorTick { .. }
        | TraceEventKind::WorkPoolWorkerBlocked { .. }
        | TraceEventKind::WorkPoolWorkerUnblocked { .. } => "workpool",
        TraceEventKind::LetBindingStep { .. } | TraceEventKind::BindingsApplied { .. } => "binding",
        TraceEventKind::ArgumentPreEvalResult { .. } => "preeval",
        TraceEventKind::TablingDecision { .. } => "tabling",
        TraceEventKind::RuleSelected { .. } => "rule",
        TraceEventKind::RuleMatchAttempt { .. } => "rule",
        TraceEventKind::RuleLookup { .. } => "rule",
        TraceEventKind::RuleIndexInsert { .. } => "rule",
        TraceEventKind::SelfEvaluating { .. } => "eval",
        TraceEventKind::ParallelDispatch { .. } => "nondeterminism",
        TraceEventKind::TrampolineStep { .. } => "eval",
        TraceEventKind::ContinuationEnter { .. } | TraceEventKind::ContinuationEmit { .. }
        | TraceEventKind::ContinuationExitNoResume { .. }
        | TraceEventKind::BindingsDropped { .. }
        | TraceEventKind::BindingsExtracted { .. }
        | TraceEventKind::BindingsFreshened { .. }
        | TraceEventKind::VariableLookupFailed { .. } => "binding-flow",
    }
}
