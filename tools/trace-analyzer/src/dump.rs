//! Sequential event dump (human-readable or JSON).

use crate::reader::TraceReader;
use trace_format::{TraceEvent, TraceEventKind, TraceTier, TraceValue};

pub fn run(file: &str, json: bool, limit: Option<usize>) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    println!("Trace file: {}", reader.header.source_file);
    println!("MeTTaTron version: {}", reader.header.mettatron_version);
    println!("CPUs: {}", reader.header.cpu_count);
    println!();

    let mut count = 0usize;
    for event in reader.events() {
        if let Some(lim) = limit {
            if count >= lim {
                println!("... (truncated at {lim} events)");
                break;
            }
        }

        if json {
            print_event_json(&event);
        } else {
            print_event_human(&event, &reader);
        }

        count += 1;
    }

    println!();
    println!("Total events displayed: {count}");
    Ok(())
}

fn tier_label(tier: &TraceTier) -> &'static str {
    match tier {
        TraceTier::TreeWalker => "TreeWalker",
        TraceTier::BytecodeVM => "BytecodeVM",
        TraceTier::JitStage1 => "JitStage1",
        TraceTier::JitStage2 => "JitStage2",
    }
}

fn format_trace_value(v: &TraceValue) -> String {
    match v {
        TraceValue::Atom(s) => s.clone(),
        TraceValue::Bool(b) => if *b { "True".to_string() } else { "False".to_string() },
        TraceValue::Long(n) => n.to_string(),
        TraceValue::Float(f) => f.to_string(),
        TraceValue::String(s) => format!("\"{}\"", s),
        TraceValue::SExpr(items) => {
            let parts: Vec<String> = items.iter().map(format_trace_value).collect();
            format!("({})", parts.join(" "))
        }
        TraceValue::Unit => "()".to_string(),
        TraceValue::Error(msg, details) => {
            format!("(Error {} {})", msg, format_trace_value(details))
        }
        TraceValue::Type(inner) => format!("Type({})", format_trace_value(inner)),
        TraceValue::Empty => "Empty".to_string(),
        TraceValue::Quoted(inner) => format!("(quote {})", format_trace_value(inner)),
    }
}

fn print_event_human(event: &TraceEvent, reader: &TraceReader) {
    // Header line: [#seq TN DD Tier span file]
    let span_str = if let Some(span) = &event.expr_span {
        let file = reader.resolve_file(span.file_id);
        format!(" {}:{}-{}:{} {}", span.start_row, span.start_col, span.end_row, span.end_col, file)
    } else {
        String::new()
    };

    let ts_ns = event.timestamp_ns;
    let dur_str = match event.duration_ns {
        Some(d) => format!(" dur={}ns", d),
        None => String::new(),
    };
    let span_str2 = match event.span_id {
        Some(id) => format!(" span={}", id),
        None => String::new(),
    };
    println!(
        "[#{} T{} D{} {} {}ns{}{}{}]",
        event.seq, event.thread_id, event.depth, tier_label(&event.tier), ts_ns, dur_str, span_str2, span_str,
    );

    // Input => Outputs
    let input_str = format_trace_value(&event.input);
    if event.outputs.is_empty() {
        println!("  {}", input_str);
    } else {
        let outputs_str: Vec<String> = event.outputs.iter().map(format_trace_value).collect();
        println!("  {} => [{}]", input_str, outputs_str.join(", "));
    }

    // Kind-specific details
    print_kind_details(&event.kind);

    println!();
}

fn print_kind_details(kind: &TraceEventKind) {
    match kind {
        TraceEventKind::RuleApplication { rule_lhs, rule_rhs, bindings, rule_span: _ } => {
            println!("  RuleApplication {{");
            println!("    lhs: {}", format_trace_value(rule_lhs));
            println!("    rhs: {}", format_trace_value(rule_rhs));
            if !bindings.is_empty() {
                let b: Vec<String> = bindings.iter()
                    .map(|(k, v)| format!("{} = {}", k, format_trace_value(v)))
                    .collect();
                println!("    bindings: {{ {} }}", b.join(", "));
            }
            println!("  }}");
        }
        TraceEventKind::GroundedOp { op_name, args } => {
            let args_str: Vec<String> = args.iter().map(format_trace_value).collect();
            println!("  GroundedOp {{ op: \"{}\", args: [{}] }}", op_name, args_str.join(", "));
        }
        TraceEventKind::SpecialForm { form_name, phase } => {
            println!("  SpecialForm {{ form: \"{}\", phase: \"{}\" }}", form_name, phase);
        }
        TraceEventKind::PatternMatch { pattern, value, success, bindings } => {
            println!("  PatternMatch {{ pattern: {}, value: {}, success: {}, bindings: {} }}",
                     format_trace_value(pattern), format_trace_value(value), success, bindings.len());
        }
        TraceEventKind::RuleMatchSet { match_count, .. } => {
            println!("  RuleMatchSet {{ match_count: {} }}", match_count);
        }
        TraceEventKind::TypeOperation { op, subject, result_type } => {
            let rt = result_type.as_ref().map(format_trace_value).unwrap_or_else(|| "None".to_string());
            println!("  TypeOperation {{ op: \"{}\", subject: {}, result: {} }}", op, format_trace_value(subject), rt);
        }
        TraceEventKind::ApplicativePreEval { operator, arg_indices, source } => {
            println!("  ApplicativePreEval {{ op: \"{}\", indices: {:?}, source: \"{}\" }}", operator, arg_indices, source);
        }
        TraceEventKind::BranchPrune { expected_type, pruned_count, surviving_count, pruned_types } => {
            println!("  BranchPrune {{ expected: {}, pruned: {}, surviving: {} }}",
                     format_trace_value(expected_type), pruned_count, surviving_count);
            if !pruned_types.is_empty() {
                for (i, pt) in pruned_types.iter().enumerate() {
                    let ty_str = match pt {
                        Some(tv) => format_trace_value(tv),
                        None => "None".to_string(),
                    };
                    println!("    pruned[{}]: rhs_type={}", i, ty_str);
                }
            }
        }
        TraceEventKind::ErrorCreated { message, details } => {
            println!("  ErrorCreated {{ msg: \"{}\", details: {} }}", message, format_trace_value(details));
        }
        TraceEventKind::ErrorCaught { error, handler, default_used } => {
            let def = default_used.as_ref().map(format_trace_value).unwrap_or_else(|| "None".to_string());
            println!("  ErrorCaught {{ error: {}, handler: \"{}\", default: {} }}", format_trace_value(error), handler, def);
        }
        TraceEventKind::ErrorPropagated { error, context } => {
            println!("  ErrorPropagated {{ error: {}, context: \"{}\" }}", format_trace_value(error), context);
        }
        TraceEventKind::GroundedOpError { op_name, error_kind, message, .. } => {
            println!("  GroundedOpError {{ op: \"{}\", kind: \"{}\", msg: \"{}\" }}", op_name, error_kind, message);
        }
        TraceEventKind::NondeterministicFork { branch_count } => {
            println!("  NondeterministicFork {{ branches: {} }}", branch_count);
        }
        TraceEventKind::BranchStart { branch_index, total_branches } => {
            println!("  BranchStart {{ index: {}, total: {} }}", branch_index, total_branches);
        }
        TraceEventKind::BranchEnd { branch_index, result_count } => {
            println!("  BranchEnd {{ index: {}, results: {} }}", branch_index, result_count);
        }
        TraceEventKind::TierDispatch { expression_hash, selected_tier, execution_count } => {
            println!("  TierDispatch {{ hash: 0x{:016x}, tier: {}, exec_count: {} }}",
                     expression_hash, tier_label(selected_tier), execution_count);
        }
        TraceEventKind::BytecodeCompilation { expression_hash, execution_count } => {
            println!("  BytecodeCompilation {{ hash: 0x{:016x}, exec_count: {} }}", expression_hash, execution_count);
        }
        TraceEventKind::JitCompilation { expression_hash, stage, execution_count } => {
            println!("  JitCompilation {{ hash: 0x{:016x}, stage: {}, exec_count: {} }}", expression_hash, stage, execution_count);
        }
        TraceEventKind::JitBailout { bailout_ip, reason, fallback_tier } => {
            println!("  JIT BAILOUT @ ip={}, reason: {}, fallback: {}", bailout_ip, reason, fallback_tier);
        }
        TraceEventKind::BytecodeHalt { ip, reason } => {
            println!("  BytecodeHalt @ ip={}, reason: \"{}\"", ip, reason);
        }
        TraceEventKind::EvalStart => {
            println!("  EvalStart");
        }
        TraceEventKind::EvalEnd { result_count } => {
            println!("  EvalEnd {{ results: {} }}", result_count);
        }
        TraceEventKind::GcSafepoint { root_count, allocation_delta_bytes } => {
            println!("  GcSafepoint {{ roots: {}, alloc_delta: {} bytes }}", root_count, allocation_delta_bytes);
        }
        TraceEventKind::TypeInference { expression, inferred_types, source } => {
            let types_str: Vec<String> = inferred_types.iter().map(format_trace_value).collect();
            println!("  TypeInference {{ expr: {}, types: [{}], source: \"{}\" }}",
                     format_trace_value(expression), types_str.join(", "), source);
        }
        TraceEventKind::TypeMatch { actual, expected, result, reason } => {
            println!("  TypeMatch {{ actual: {}, expected: {}, result: {}, reason: \"{}\" }}",
                     format_trace_value(actual), format_trace_value(expected), result, reason);
        }
        TraceEventKind::RhsTypeComputed { head, arity, lhs, rhs, rhs_type } => {
            let rt = rhs_type.as_ref().map(format_trace_value).unwrap_or_else(|| "None".to_string());
            println!("  RhsTypeComputed {{ head: \"{}\", arity: {}, rhs_type: {} }}", head, arity, rt);
            println!("    lhs: {}", format_trace_value(lhs));
            println!("    rhs: {}", format_trace_value(rhs));
        }
        TraceEventKind::InferredTypeRegistered { function_name, registered_type, source } => {
            println!("  InferredTypeRegistered {{ fn: \"{}\", type: {}, source: \"{}\" }}",
                     function_name, format_trace_value(registered_type), source);
        }
        TraceEventKind::WorkPoolTaskEnqueued { task_kind, priority, queue_depth, active_workers, max_workers } => {
            println!("  WorkPoolTaskEnqueued {{ kind: \"{}\", pri: {}, queue: {}, active: {}/{} }}",
                     task_kind, priority, queue_depth, active_workers, max_workers);
        }
        TraceEventKind::WorkPoolTaskDropped { task_kind, queue_depth, active_workers } => {
            println!("  WorkPoolTaskDropped {{ kind: \"{}\", queue: {}, active: {} }}",
                     task_kind, queue_depth, active_workers);
        }
        TraceEventKind::WorkPoolTaskCompleted { task_kind, runtime_nanos, queue_depth, active_workers } => {
            println!("  WorkPoolTaskCompleted {{ kind: \"{}\", runtime: {}ns, queue: {}, active: {} }}",
                     task_kind, runtime_nanos, queue_depth, active_workers);
        }
        TraceEventKind::WorkPoolScaleEvent {
            action, active_workers_after, min_workers, max_workers,
            queue_depth, ema_throughput, ema_queue_depth,
            ema_slab_pressure, ema_rss_pressure, objective, emergency,
            hc_direction, hc_cooldown_remaining, hc_prev_objective, hc_improvement,
            raw_throughput, raw_slab_pressure, raw_rss_pressure, bp_level,
            slab_amplifier, term_throughput, term_queue_depth, term_slab_pressure, term_rss_pressure,
            blocked_worker_count, overflow_count, decision_phase,
            delta_evals, elapsed_seconds,
        } => {
            println!("  WorkPoolScaleEvent {{ action: \"{}\", active: {}, range: [{}, {}], queue: {}, phase: \"{}\" }}",
                     action, active_workers_after, min_workers, max_workers, queue_depth, decision_phase);
            println!("    ema: tp={:.2}, qd={:.2}, slab={:.3}, rss={:.3}, J={:.4}, emergency={}",
                     ema_throughput, ema_queue_depth, ema_slab_pressure, ema_rss_pressure, objective, emergency);
            println!("    hc: dir={}, cooldown={}, prev_obj={:.4}, improvement={:.4}",
                     hc_direction, hc_cooldown_remaining, hc_prev_objective, hc_improvement);
            println!("    raw: tp={:.2}, slab={:.3}, rss={:.3}, bp={}, amp={:.1}",
                     raw_throughput, raw_slab_pressure, raw_rss_pressure, bp_level, slab_amplifier);
            println!("    terms: tp={:.4}, qd={:.4}, slab={:.4}, rss={:.4}",
                     term_throughput, term_queue_depth, term_slab_pressure, term_rss_pressure);
            println!("    pool: blocked={}, overflow={}, delta_evals={}, elapsed={:.4}s",
                     blocked_worker_count, overflow_count, delta_evals, elapsed_seconds);
        }
        TraceEventKind::WorkPoolWorkerParked { worker_id, queue_depth } => {
            println!("  WorkPoolWorkerParked {{ worker: {}, queue: {} }}", worker_id, queue_depth);
        }
        TraceEventKind::WorkPoolWorkerResumed { worker_id, queue_depth, active_workers } => {
            println!("  WorkPoolWorkerResumed {{ worker: {}, queue: {}, active: {} }}",
                     worker_id, queue_depth, active_workers);
        }
        TraceEventKind::WorkPoolBlockedWorkersDetected { blocked_count, active_workers, blocked_indices } => {
            println!("  WorkPoolBlockedWorkersDetected {{ blocked: {}, active: {}, indices: {:?} }}",
                     blocked_count, active_workers, blocked_indices);
        }
        TraceEventKind::WorkPoolCompensatoryAction {
            core_unparked, overflow_spawned, overflow_drained,
            target, deficit, rss_veto,
        } => {
            println!("  WorkPoolCompensatoryAction {{ core_unparked: {}, overflow_spawned: {}, overflow_drained: {}, target: {}, deficit: {}, rss_veto: {} }}",
                     core_unparked, overflow_spawned, overflow_drained, target, deficit, rss_veto);
        }
        TraceEventKind::WorkPoolMonitorTick {
            current_eval_count, elapsed_ns, queue_len, bp_level, rss_bytes,
        } => {
            println!("  WorkPoolMonitorTick {{ evals: {}, elapsed: {}ns, queue: {}, bp: {}, rss: {} }}",
                     current_eval_count, elapsed_ns, queue_len, bp_level, rss_bytes);
        }
        TraceEventKind::WorkPoolWorkerBlocked { worker_id, cpu_ratio } => {
            println!("  WorkPoolWorkerBlocked {{ worker: {}, cpu_ratio: {:.3} }}", worker_id, cpu_ratio);
        }
        TraceEventKind::WorkPoolWorkerUnblocked { worker_id } => {
            println!("  WorkPoolWorkerUnblocked {{ worker: {} }}", worker_id);
        }
    }
}

fn print_event_json(event: &TraceEvent) {
    match serde_json::to_string(event) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("JSON serialization error: {e}"),
    }
}
