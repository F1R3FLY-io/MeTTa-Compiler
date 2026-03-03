//! Pattern-based event filtering.

use crate::reader::TraceReader;
use trace_format::{TraceEventKind, TraceValue};

pub fn run(file: &str, pattern: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    println!("Searching for pattern: \"{}\"", pattern);
    println!();

    let mut match_count = 0u64;
    for event in reader.events() {
        if matches_pattern(&event.input, pattern)
            || event.outputs.iter().any(|o| matches_pattern(o, pattern))
            || kind_matches_pattern(&event.kind, pattern)
        {
            // Reuse the dump formatter for consistent output
            println!(
                "[#{} T{} D{} {:?}]",
                event.seq, event.thread_id, event.depth, event.tier,
            );
            match_count += 1;

            // Print input/output
            let input_str = format_value(&event.input);
            if event.outputs.is_empty() {
                println!("  {}", input_str);
            } else {
                let outputs: Vec<String> = event.outputs.iter().map(format_value).collect();
                println!("  {} => [{}]", input_str, outputs.join(", "));
            }

            println!();
        }
    }

    println!("Found {} matching events", match_count);
    Ok(())
}

fn matches_pattern(value: &TraceValue, pattern: &str) -> bool {
    match value {
        TraceValue::Atom(s) => s.contains(pattern),
        TraceValue::String(s) => s.contains(pattern),
        TraceValue::SExpr(items) => items.iter().any(|item| matches_pattern(item, pattern)),
        TraceValue::Error(msg, details) => msg.contains(pattern) || matches_pattern(details, pattern),
        TraceValue::Type(inner) => matches_pattern(inner, pattern),
        TraceValue::Quoted(inner) => matches_pattern(inner, pattern),
        _ => false,
    }
}

fn kind_matches_pattern(kind: &TraceEventKind, pattern: &str) -> bool {
    match kind {
        TraceEventKind::GroundedOp { op_name, .. } => op_name.contains(pattern),
        TraceEventKind::SpecialForm { form_name, .. } => form_name.contains(pattern),
        TraceEventKind::GroundedOpError { op_name, error_kind, message, .. } => {
            op_name.contains(pattern) || error_kind.contains(pattern) || message.contains(pattern)
        }
        TraceEventKind::ErrorCreated { message, .. } => message.contains(pattern),
        TraceEventKind::JitBailout { reason, .. } => reason.contains(pattern),
        TraceEventKind::BytecodeHalt { reason, .. } => reason.contains(pattern),
        TraceEventKind::WorkPoolTaskEnqueued { task_kind, .. }
        | TraceEventKind::WorkPoolTaskDropped { task_kind, .. }
        | TraceEventKind::WorkPoolTaskCompleted { task_kind, .. } => task_kind.contains(pattern),
        TraceEventKind::WorkPoolScaleEvent { action, .. } => action.contains(pattern),
        TraceEventKind::WorkPoolWorkerParked { .. }
        | TraceEventKind::WorkPoolWorkerResumed { .. }
        | TraceEventKind::WorkPoolBlockedWorkersDetected { .. }
        | TraceEventKind::WorkPoolCompensatoryAction { .. }
        | TraceEventKind::WorkPoolMonitorTick { .. }
        | TraceEventKind::WorkPoolWorkerBlocked { .. }
        | TraceEventKind::WorkPoolWorkerUnblocked { .. } => false,
        _ => false,
    }
}

fn format_value(v: &TraceValue) -> String {
    match v {
        TraceValue::Atom(s) => s.clone(),
        TraceValue::Bool(b) => if *b { "True".to_string() } else { "False".to_string() },
        TraceValue::Long(n) => n.to_string(),
        TraceValue::Float(f) => f.to_string(),
        TraceValue::String(s) => format!("\"{}\"", s),
        TraceValue::SExpr(items) => {
            let parts: Vec<String> = items.iter().map(format_value).collect();
            format!("({})", parts.join(" "))
        }
        TraceValue::Unit => "()".to_string(),
        TraceValue::Error(msg, details) => format!("(Error {} {})", msg, format_value(details)),
        TraceValue::Type(inner) => format!("Type({})", format_value(inner)),
        TraceValue::Empty => "Empty".to_string(),
        TraceValue::Quoted(inner) => format!("(quote {})", format_value(inner)),
    }
}
