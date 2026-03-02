//! Generic Trampoline Engine
//!
//! This module provides a truly generic trampoline evaluation engine that works with
//! any value type implementing `MettaValueTrait`. This enables the same evaluation
//! logic to work with both heap-allocated (`MettaValue`) and arena-allocated
//! (`MettaValue`) values.
//!
//! ## Design
//!
//! The generic engine:
//! - Uses `GenericWorkItem<V, F>` and `GenericContinuation<V, F>` for work tracking
//! - Calls `eval_step_generic` for single-step evaluation
//! - Uses `GenericEnvironment<V, F>` for all environment operations
//!
//! ## Entry Points
//!
//! - `eval_trampoline_generic`: Generic evaluation for any `EvalContext`
//!
//! The generic engine is parameterized by the `EvalContext` trait, which determines
//! the value type and factory. The production implementation uses `StaticEvalContext`
//! with arena-allocated `MettaValue` values.

use std::collections::VecDeque;
use std::sync::OnceLock;

use tracing::trace;

use super::context::{ContextEnv, EvalContext};
use super::generic_engine::{
    apply_bindings_generic, eval_switch_generic, is_boolean_check_pattern, pattern_match_generic,
    try_match_all_rules_generic, GenericSwitchResult,
};
use super::generic_types::{GenericContinuation, GenericEvalResult, GenericWorkItem};
use super::super::list_ops::substitute_variable_generic;
use super::super::processing::{
    process_collected_sexpr_generic, GenericProcessedSExpr,
};
use super::super::step::{eval_step_generic, GenericEvalStep};

use crate::backend::eval::types_generic::{
    extract_type_constraint, get_ground_type, is_pattern_type_compatible,
    infer_type_generic, types_match_generic, types_match_with_subtypes,
};
use crate::backend::grounded::{execute_generic_grounded_op, ExecError, GenericGroundedWork};
use crate::backend::models::{GenericMultiplicityMatch, MettaValueFactory, MettaValueInner, MettaValueTrait};
use crate::backend::models::metta_value::is_variable_str;

/// Cached check for the `METTA_DEBUG_EVAL` environment variable.
/// Uses `OnceLock` so the syscall happens at most once per process.
static METTA_DEBUG_EVAL_CACHED: OnceLock<bool> = OnceLock::new();

fn is_debug_eval() -> bool {
    *METTA_DEBUG_EVAL_CACHED.get_or_init(|| std::env::var("METTA_DEBUG_EVAL").is_ok())
}

// Evaluation memoization and type-driven dispatch helpers extracted to `dispatch_hints`
// module for icache locality. Re-import the functions used in this file.
use super::dispatch_hints::{
    is_memoized_normal_form, memoize_normal_form,
    derive_arg_expected_type,
};

/// Generic trampoline evaluation entry point.
///
/// This function provides a unified evaluation engine that works with any value type
/// implementing `MettaValueTrait`. It uses:
/// - `GenericWorkItem<V, F>` for pending evaluation work
/// - `GenericContinuation<V, F>` for continuation handling
/// - `eval_step_generic` for single-step evaluation
///
/// All environment operations use `GenericEnvironment<V, F>` inherent methods.
///
/// # Type Parameters
///
/// - `C`: The evaluation context (e.g., `StaticEvalContext` or `SessionContext`)
///
/// # Arguments
///
/// - `value`: The value to evaluate
/// - `env`: The evaluation environment (`GenericEnvironment<C::Value, C::Factory>`)
/// - `ctx`: The evaluation context providing the factory
///
/// # Returns
///
/// A tuple of (results, final_environment)
pub fn eval_trampoline_generic<C: EvalContext>(
    value: C::Value,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalResult<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    // Debug tracing controlled by environment variable (cached — one syscall per process)
    let debug_eval = is_debug_eval();
    let mut eval_count: u64 = 0;

    // Trace: EvalStart with span correlation + start timestamp.
    // These variables carry the start timestamp and span ID to the EvalEnd site.
    #[cfg(feature = "eval-trace")]
    let (_eval_start_ns, _eval_span_id) = {
        if let Some(tc) = ctx.trace_collector() {
            let span_id = tc.next_span_id();
            let start_ns = tc.elapsed_ns();
            tc.emit_timed(
                trace_format::TraceTier::TreeWalker,
                0,
                crate::backend::trace::trace_value_generic(&value),
                vec![],
                None,
                trace_format::TraceEventKind::EvalStart,
                start_ns,
                None, // duration not known yet — EvalEnd carries it
                Some(span_id),
            );
            (start_ns, span_id)
        } else {
            (0u64, 0u64)
        }
    };

    // Set thread-local trace collector so that type inference (infer_types_generic,
    // types_match_generic) and rule management (add_rule) can emit trace events
    // without requiring an EvalContext parameter.
    #[cfg(feature = "eval-trace")]
    {
        if let Some(tc) = ctx.trace_collector() {
            // The trace collector is behind a shared reference with a 'static-like
            // lifetime (Arc in SessionContext). We store a raw pointer in the
            // thread-local; the trampoline outlives all type inference calls.
            crate::backend::trace::thread_local_sink::set_thread_trace_collector_ref(tc);
        }
    }

    // Initialize work stack with the initial evaluation
    let mut work_stack: Vec<GenericWorkItem<C::Value, ContextEnv<C>>> = vec![GenericWorkItem::Eval {
        value,
        env: env.clone(),
        depth: 0,
        is_tail_call: false,
        expected_type: None,
    }];

    // Continuation storage - index 0 is always Done
    let mut continuations: Vec<GenericContinuation<C::Value, ContextEnv<C>>> = vec![GenericContinuation::Done];

    // Final result storage
    let mut final_result: Option<GenericEvalResult<C::Value, ContextEnv<C>>> = None;

    // GC safepoint counter: wrapping u16 overflows every 4096 iterations (mask 0xFFF).
    // Increased from u8 (256) to reduce maybe_process_gc_response overhead (4.9% → ~1%).
    let mut gc_counter: u16 = 0;

    // Main trampoline loop
    while let Some(work) = work_stack.pop() {
        // Periodic GC safepoint check (every 4096 trampoline iterations)
        gc_counter = gc_counter.wrapping_add(1);
        if gc_counter & 0xFFF == 0 && ctx.should_safepoint() {
            // Collect all live values from trampoline state as GC roots.
            // This ensures values in the work stack and continuations survive
            // the mark-sweep cycle that runs during the safepoint pause.
            let mut roots = Vec::new();
            // Root from the work item we just popped (it's not on the stack)
            work.collect_values(&mut roots);
            for w in &work_stack {
                w.collect_values(&mut roots);
            }
            for c in &continuations {
                c.collect_values(&mut roots);
            }
            // Collect roots from all caller frames in the thread-local chain.
            // This protects values held by callers of nested trampolines
            // (e.g., compiled expressions in eval_include_generic).
            // Only applies when C::Value is MettaValue (GC-managed). After
            // monomorphization, the TypeId check becomes a compile-time constant.
            if std::any::TypeId::of::<C::Value>() == std::any::TypeId::of::<crate::backend::models::MettaValue>() {
                // SAFETY: C::Value is MettaValue, so Vec<C::Value> and Vec<MettaValue>
                // have identical layout. We transmute the reference temporarily.
                let concrete_roots: &mut Vec<crate::backend::models::MettaValue> =
                    unsafe { &mut *(&mut roots as *mut Vec<C::Value> as *mut Vec<crate::backend::models::MettaValue>) };
                crate::backend::eval::frame_chain::collect_frame_chain_roots(concrete_roots);
            }
            #[cfg(feature = "eval-trace")]
            let _root_count = roots.len() as u32;
            #[cfg(feature = "eval-trace")]
            let _safepoint_start = {
                ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0)
            };
            ctx.perform_safepoint(roots);
            // Trace: GcSafepoint with measured pause duration
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let end_ns = tc.elapsed_ns();
                    let duration = end_ns.saturating_sub(_safepoint_start);
                    tc.emit_timed(
                        trace_format::TraceTier::TreeWalker,
                        0,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::GcSafepoint {
                            root_count: _root_count,
                            allocation_delta_bytes: 0,
                        },
                        _safepoint_start,
                        Some(duration),
                        None, // no span correlation needed for safepoints
                    );
                }
            }
        }
        match work {
            GenericWorkItem::Eval {
                value,
                env,
                depth,
                is_tail_call,
                expected_type,
            } => {
                trace!(target: "mettatron::backend::eval::eval_trampoline_generic", ?value, depth, "eval work item");

                // Debug trace (zero-conversion: uses Debug trait)
                if debug_eval {
                    eval_count += 1;
                    if eval_count % 1000 == 0 || eval_count < 100 {
                        eprintln!(
                            "[EVAL#{}] depth={} work_stack={} conts={} value={:?}",
                            eval_count,
                            depth,
                            work_stack.len(),
                            continuations.len(),
                            value
                        );
                    }
                }

                // Phase 9.5: Normal-form memoization check.
                // If this S-expression has been previously evaluated and reached
                // fixpoint (evaluated to itself), skip evaluation entirely.
                let is_sexpr = value.as_sexpr().is_some();
                if is_sexpr && is_memoized_normal_form(&value) {
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![value], env),
                    });
                    continue;
                }

                // Sub-expression tiered dispatch: increment per-slot execution counter
                // and attempt dispatch to compiled bytecode/JIT.
                //
                // For compilable S-expressions (~3% of all in PLN), this:
                // 1. Increments the per-slot atomic counter (~10-15 cycles)
                // 2. Reads the cached compilation hash from the slot (~3 cycles)
                // 3. If hash is non-zero (cron has flushed): DashMap lookup (~15 cycles)
                // 4. If compiled code is ready: dispatch to highest tier (JIT2 > JIT1 > Bytecode)
                // 5. On dispatch success: push Resume and skip eval_step_generic
                //
                // Net overhead for cold expressions (no compiled code): ~25-30 cycles
                // Net benefit for hot expressions: tree-walker step replaced by bytecode/JIT
                //
                // Only active for MettaValue (GC-managed) — after monomorphization
                // the TypeId check becomes a compile-time constant, and the else
                // branch is eliminated entirely for non-MettaValue instantiations.
                if is_sexpr
                    && std::any::TypeId::of::<C::Value>()
                        == std::any::TypeId::of::<crate::backend::models::MettaValue>()
                {
                    let has_compilable_head = if let Some(items) = value.as_sexpr() {
                        if let Some(head) = items.first() {
                            if let Some(name) = head.as_atom() {
                                matches!(name,
                                    "!" | "eval"
                                    | "+" | "-" | "*" | "/" | "%" | "abs" | "pow"
                                    | "<" | "<=" | ">" | ">=" | "==" | "!="
                                    | "and" | "or" | "not" | "xor"
                                    | "if" | "case" | "chain"
                                    | "let" | "let*"
                                    | "superpose"
                                    | "quote" | "unquote"
                                    | "car-atom" | "cdr-atom" | "cons-atom" | "size-atom"
                                    | "decons-atom" | "empty"
                                    | "map-atom" | "filter-atom" | "foldl-atom"
                                    | "get-type" | "get-metatype"
                                    | "error" | "is-error" | "catch"
                                    | "repr"
                                )
                            } else { false }
                        } else { false }
                    } else { false };
                    if has_compilable_head {
                        // Merged: increment per-slot exec counter AND read cached compilation hash
                        // in a single thread-local + generation check (vs 2× for separate calls).
                        let compilation_hash = crate::backend::bytecode::tiered_cache::increment_and_get_hash(value.inner_ptr());

                        // Try dispatching to compiled bytecode/JIT.
                        // hash != 0 guard short-circuits before any trait dispatch / DashMap lookup for cold code.
                        if compilation_hash != 0 {
                        if let Some((results, new_env)) = ctx.try_compiled_dispatch(&value, &env, compilation_hash) {
                            #[cfg(feature = "eval-trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    let output_tvs: Vec<trace_format::TraceValue> = results.iter()
                                        .map(|v| crate::backend::trace::trace_value_generic(v))
                                        .collect();
                                    tc.emit_converted(
                                        trace_format::TraceTier::BytecodeVM,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&value),
                                        output_tvs,
                                        None,
                                        trace_format::TraceEventKind::TierDispatch {
                                            expression_hash: 0,
                                            selected_tier: trace_format::TraceTier::BytecodeVM,
                                            execution_count: 0,
                                        },
                                    );
                                }
                            }
                            work_stack.push(GenericWorkItem::Resume {
                                result: (results, new_env),
                            });
                            continue; // Skip eval_step_generic — compiled code handled it
                        }
                        // Dispatch returned None — fall through to tree-walker
                        }
                    }
                }

                // Save input pointer for fixpoint detection (Phase 9.5)
                let input_ptr = if is_sexpr { value.inner_ptr() } else { std::ptr::null() };

                // Perform one step of evaluation using generic step function
                let step_result = eval_step_generic(value, env.clone(), depth, ctx);
                let _ = is_tail_call; // Used to determine depth in push sites
                trace!(target: "mettatron::backend::eval::eval_trampoline_generic", ?step_result);

                // Process the step result
                match step_result {
                    // Direct result - resume continuation
                    GenericEvalStep::Done(result) => {
                        // Phase 9.5: Fixpoint detection — if eval returned
                        // the same S-expression (by pointer), memoize it
                        if !input_ptr.is_null()
                            && result.0.len() == 1
                            && result.0[0].inner_ptr() == input_ptr
                        {
                            memoize_normal_form(&result.0[0]);
                        }
                        work_stack.push(GenericWorkItem::Resume { result });
                    }

                    // Need to evaluate S-expression sub-items
                    GenericEvalStep::EvalSExpr { items, env, depth } => {
                        if items.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![ctx.factory().sexpr(vec![])], env),
                            });
                        } else {
                            let mut items_deque: VecDeque<C::Value> = items.into_iter().collect();
                            let first = items_deque.pop_front().expect("items is non-empty");

                            continuations.push(GenericContinuation::CollectSExpr {
                                remaining: items_deque,
                                collected: Vec::new(),
                                original_env: env.clone(),
                                depth,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    }

                    // Start a TCO grounded operation
                    // Uses static dispatch - works with any V: MettaValueTrait (NO conversion)
                    GenericEvalStep::StartGroundedOp { state, env, depth } => {
                        let mut state = state;
                        // Use static dispatch - monomorphized for each value type
                        // Clone op_name to avoid borrow conflict with mutable state
                        let op_name = state.op_name.clone();
                        #[cfg(feature = "eval-trace")]
                        let _grounded_start_ns = {
                            ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0)
                        };
                        if let Some(work) = execute_generic_grounded_op(&op_name, &mut state, ctx.factory()) {
                            match work {
                                GenericGroundedWork::Done(results) => {
                                    // Results are already in correct type V - NO conversion
                                    let values: Vec<C::Value> = results
                                        .into_iter()
                                        .map(|(v, _)| v)
                                        .collect();
                                    // Trace: GroundedOp success with duration
                                    #[cfg(feature = "eval-trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            let end_ns = tc.elapsed_ns();
                                            let duration = end_ns.saturating_sub(_grounded_start_ns);
                                            let input = crate::backend::trace::trace_value_generic(
                                                &ctx.factory().sexpr({
                                                    let mut parts = Vec::with_capacity(1 + state.args.len());
                                                    parts.push(ctx.factory().atom(&op_name));
                                                    for arg in state.args.iter() { parts.push(arg.clone()); }
                                                    parts
                                                }),
                                            );
                                            tc.emit_timed(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                input,
                                                values.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                                None,
                                                trace_format::TraceEventKind::GroundedOp {
                                                    op_name: op_name.clone(),
                                                    args: state.args.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                                },
                                                _grounded_start_ns,
                                                Some(duration),
                                                None,
                                            );
                                        }
                                    }
                                    work_stack.push(GenericWorkItem::Resume {
                                        result: (values, env),
                                    });
                                }
                                GenericGroundedWork::EvalArg { arg_idx, state: new_state } => {
                                    continuations.push(GenericContinuation::ProcessGroundedOp {
                                        state: new_state.clone(),
                                        pending_arg_idx: arg_idx,
                                        env: env.clone(),
                                        depth,
                                    });

                                    // Arg already in correct type V - NO conversion
                                    let arg_to_eval = new_state.args[arg_idx].clone();
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: arg_to_eval,
                                        env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                    });
                                }
                                GenericGroundedWork::Error(e) => {
                                    // Trace: GroundedOpError
                                    #[cfg(feature = "eval-trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            let (error_kind, message) = match &e {
                                                ExecError::NoReduce => ("NoReduce", String::new()),
                                                ExecError::Runtime(msg) => ("Runtime", msg.clone()),
                                                ExecError::Arithmetic(msg) => ("Arithmetic", msg.clone()),
                                                ExecError::IncorrectArgument(msg) => ("IncorrectArgument", msg.clone()),
                                            };
                                            let input = crate::backend::trace::trace_value_generic(
                                                &ctx.factory().sexpr({
                                                    let mut parts = Vec::with_capacity(1 + state.args.len());
                                                    parts.push(ctx.factory().atom(&op_name));
                                                    for arg in state.args.iter() { parts.push(arg.clone()); }
                                                    parts
                                                }),
                                            );
                                            tc.emit_converted(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                input,
                                                vec![],
                                                None,
                                                trace_format::TraceEventKind::GroundedOpError {
                                                    op_name: op_name.clone(),
                                                    error_kind: error_kind.to_string(),
                                                    message,
                                                    args: state.args.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                                },
                                            );
                                        }
                                    }
                                    match e {
                                        ExecError::NoReduce => {
                                            // MeTTa HE semantics: return the original expression unreduced
                                            let mut expr_parts = Vec::with_capacity(1 + state.args.len());
                                            expr_parts.push(ctx.factory().atom(&state.op_name));
                                            for arg in state.args.iter() {
                                                expr_parts.push(arg.clone());
                                            }
                                            let unreduced = ctx.factory().sexpr(expr_parts);
                                            work_stack.push(GenericWorkItem::Resume {
                                                result: (vec![unreduced], env),
                                            });
                                        }
                                        _ => {
                                            let error_value = match e {
                                                ExecError::Runtime(msg) => ctx.factory().error(&msg, ctx.factory().atom("TypeError")),
                                                ExecError::Arithmetic(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArithmeticError")),
                                                ExecError::IncorrectArgument(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArityError")),
                                                ExecError::NoReduce => unreachable!(),
                                            };
                                            work_stack.push(GenericWorkItem::Resume {
                                                result: (vec![error_value], env),
                                            });
                                        }
                                    }
                                }
                            }
                        } else {
                            // Operation not in generic registry - report error
                            // All 14 standard TCO operations (arithmetic, comparison, logical) are in the
                            // generic registry. Custom operations should be added there, not to the
                            // legacy registry.
                            let error_value = ctx.factory().error(
                                &format!("Grounded operation '{}' not found in generic registry", op_name),
                                ctx.factory().atom("OperationNotFoundError"),
                            );
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![error_value], env),
                            });
                        }
                    }

                    // Start let binding
                    GenericEvalStep::StartLetBinding { pattern, value_expr, body, env, depth } => {
                        continuations.push(GenericContinuation::ProcessLet {
                            pending_values: None,
                            pattern,
                            body,
                            results: Vec::new(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: value_expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Evaluate if branch (TCO)
                    GenericEvalStep::EvalIfBranch { branch, env, depth } => {
                        work_stack.push(GenericWorkItem::Eval {
                            value: branch,
                            env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                        });
                    }

                    // Evaluate rule matches with unevaluated arguments (lazy evaluation)
                    // Note: matches are now in generic type (V, GenericBindings<V>, Option<V>)
                    // Phase 8.7: Prune matches whose rhs_type is incompatible with expected_type
                    GenericEvalStep::EvalRuleMatchesLazy { mut matches, env, depth } => {
                        // 8.7: Branch pruning — filter out matches whose rhs_type
                        // is known to be incompatible with the expected_type
                        if let Some(ref expected) = expected_type {
                            let before_count = matches.len();

                            #[cfg(feature = "eval-trace")]
                            let mut pruned_types: Vec<Option<trace_format::TraceValue>> = Vec::new();

                            matches.retain(|(_rhs, _bindings, rhs_type)| {
                                let keep = match rhs_type {
                                    Some(rt) => types_match_generic(rt, expected),
                                    None => true, // Unknown type — don't prune (conservative)
                                };
                                #[cfg(feature = "eval-trace")]
                                if !keep {
                                    pruned_types.push(
                                        rhs_type.as_ref().map(crate::backend::trace::trace_value_generic)
                                    );
                                }
                                keep
                            });

                            // Emit BranchPrune trace event when pruning occurred
                            #[cfg(feature = "eval-trace")]
                            {
                                if before_count != matches.len() {
                                    if let Some(tc) = ctx.trace_collector() {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            crate::backend::trace::trace_value_generic(expected),
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::BranchPrune {
                                                expected_type: crate::backend::trace::trace_value_generic(expected),
                                                pruned_count: (before_count - matches.len()) as u32,
                                                surviving_count: matches.len() as u32,
                                                pruned_types,
                                            },
                                        );
                                    }
                                }
                            }

                            let _ = before_count; // suppress unused warning when eval-trace is off
                        }

                        if matches.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], env),
                            });
                        } else {
                            // Strip rhs_type → 2-tuples for ProcessRuleMatches
                            let mut matches_deque: VecDeque<_> = matches.into_iter()
                                .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                                .collect();
                            let total_branches = (matches_deque.len() + 1) as u32; // +1 for the one we pop
                            let (rhs, bindings) = matches_deque.pop_front().expect("matches is non-empty");

                            // Trace: NondeterministicFork + BranchStart for first branch
                            #[cfg(feature = "eval-trace")]
                            let _branch_span_id = {
                                if let Some(tc) = ctx.trace_collector() {
                                    // Emit fork event
                                    if total_branches > 1 {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            trace_format::TraceValue::Unit,
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::NondeterministicFork {
                                                branch_count: total_branches,
                                            },
                                        );
                                    }
                                    // Emit BranchStart for first branch with span ID
                                    let span_id = tc.next_span_id();
                                    let start_ns = tc.elapsed_ns();
                                    tc.emit_timed(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        trace_format::TraceValue::Unit,
                                        vec![],
                                        None,
                                        trace_format::TraceEventKind::BranchStart {
                                            branch_index: 0,
                                            total_branches,
                                        },
                                        start_ns,
                                        None, // duration filled at BranchEnd
                                        Some(span_id),
                                    );
                                    span_id
                                } else {
                                    0u64
                                }
                            };

                            continuations.push(GenericContinuation::ProcessRuleMatches {
                                remaining_matches: matches_deque,
                                results: vec![],
                                env: env.clone(),
                                depth,
                                #[cfg(feature = "eval-trace")]
                                branch_span_id: _branch_span_id,
                                #[cfg(feature = "eval-trace")]
                                branch_start_ns: {
                                    ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0)
                                },
                                #[cfg(feature = "eval-trace")]
                                branch_index: 0,
                                #[cfg(feature = "eval-trace")]
                                total_branches,
                            });

                            // Apply generic bindings to RHS - NO CONVERSION needed!
                            // Both rhs and bindings are already in generic type V
                            let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                            // Trace: RuleApplication (tree-walker, first match)
                            #[cfg(feature = "eval-trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    let bindings_tv: Vec<(String, trace_format::TraceValue)> = bindings
                                        .iter()
                                        .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                                        .collect();
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&rhs),
                                        vec![crate::backend::trace::trace_value_generic(&instantiated_rhs)],
                                        None,
                                        trace_format::TraceEventKind::RuleApplication {
                                            rule_lhs: crate::backend::trace::trace_value_generic(&rhs),
                                            rule_rhs: crate::backend::trace::trace_value_generic(&instantiated_rhs),
                                            bindings: bindings_tv,
                                            rule_span: None,
                                        },
                                    );
                                }
                            }

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        }
                    }

                    // Evaluate grounded arguments
                    GenericEvalStep::EvalGroundedArgs { items, grounded_indices, env, depth } => {
                        if grounded_indices.is_empty() {
                            work_stack.push(GenericWorkItem::Eval {
                                value: ctx.factory().sexpr(items),
                                env,
                                depth,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        } else {
                            let first_idx = grounded_indices[0];
                            let arg_to_eval = items[first_idx].clone();

                            // Phase 9.2: Derive expected_type from parent op's
                            // builtin signature for branch pruning (Phase 8.7).
                            let arg_expected_type = derive_arg_expected_type::<C>(
                                &items, first_idx, &env, ctx.factory(),
                            );

                            continuations.push(GenericContinuation::CollectGroundedArg {
                                items,
                                grounded_indices,
                                current_idx: 0,
                                evaluated_results: Vec::new(),
                                env: env.clone(),
                                depth,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: arg_to_eval,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: arg_expected_type,
                            });
                        }
                    }

                    // Start map-atom
                    GenericEvalStep::StartMapAtom { elements, var_name, template, env, depth } => {
                        if elements.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![ctx.factory().sexpr(vec![])], env),
                            });
                        } else {
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            continuations.push(GenericContinuation::ProcessMapAtom {
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                template: template.clone(),
                                collected_results: vec![],
                                env: env.clone(),
                                depth,
                            });

                            // Substitute variable and evaluate - NO CONVERSION NEEDED
                            let instantiated = substitute_variable_generic(
                                &template, &var_name, &first, ctx.factory(),
                            );

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    }

                    // Start filter-atom
                    GenericEvalStep::StartFilterAtom { elements, var_name, predicate, env, depth } => {
                        if elements.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![ctx.factory().sexpr(vec![])], env),
                            });
                        } else {
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            continuations.push(GenericContinuation::ProcessFilterAtom {
                                current_element: Some(first.clone()),
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                predicate: predicate.clone(),
                                filtered_results: vec![],
                                env: env.clone(),
                                depth,
                            });

                            // NO CONVERSION NEEDED - use generic substitute
                            let instantiated = substitute_variable_generic(
                                &predicate, &var_name, &first, ctx.factory(),
                            );

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                // Phase 9.2d: filter-atom predicate should return Bool
                                expected_type: Some(ctx.factory().atom("Bool")),
                            });
                        }
                    }

                    // Start foldl-atom
                    GenericEvalStep::StartFoldlAtom { elements, init, acc_var_name, item_var_name, operation, env, depth } => {
                        if elements.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![init], env),
                            });
                        } else {
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            continuations.push(GenericContinuation::ProcessFoldlAtom {
                                remaining_elements: remaining,
                                acc_var_name: acc_var_name.clone(),
                                item_var_name: item_var_name.clone(),
                                operation: operation.clone(),
                                env: env.clone(),
                                depth,
                            });

                            // NO CONVERSION NEEDED - use generic substitute for both variables
                            let instantiated = substitute_variable_generic(
                                &operation, &acc_var_name, &init, ctx.factory(),
                            );
                            let instantiated = substitute_variable_generic(
                                &instantiated, &item_var_name, &first, ctx.factory(),
                            );

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    }

                    // Start sort-tuple (insertion sort via trampoline)
                    GenericEvalStep::StartSortTuple { elements, var1_name, var2_name, comparator, env, depth } => {
                        if elements.len() <= 1 {
                            // 0 or 1 elements — already sorted
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![ctx.factory().sexpr(elements)], env),
                            });
                        } else {
                            // Start insertion sort: first element is trivially sorted,
                            // take the second element as 'current' to insert.
                            let mut unsorted_iter = elements.into_iter();
                            let first = unsorted_iter.next().expect("at least 2 elements");
                            let current = unsorted_iter.next().expect("at least 2 elements");
                            let unsorted: Vec<_> = unsorted_iter.collect();

                            // Compare current vs sorted[0] (= first)
                            let instantiated = substitute_variable_generic(
                                &comparator, &var1_name, &current, ctx.factory(),
                            );
                            let instantiated = substitute_variable_generic(
                                &instantiated, &var2_name, &first, ctx.factory(),
                            );

                            continuations.push(GenericContinuation::ProcessSortTuple {
                                sorted: vec![first],
                                unsorted,
                                current,
                                insert_pos: 0,
                                var1_name,
                                var2_name,
                                comparator,
                                env: env.clone(),
                                depth,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    }

                    // Start best-candidate (linear scan via trampoline)
                    GenericEvalStep::StartBestCandidate { elements, var_name, rank_fn, env, depth } => {
                        if elements.is_empty() {
                            // Empty tuple — return Unit
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![ctx.factory().unit()], env),
                            });
                        } else {
                            let mut elem_iter = elements.into_iter();
                            let first = elem_iter.next().expect("non-empty");
                            let remaining: Vec<_> = elem_iter.collect();

                            // Evaluate rank function for first element
                            let instantiated = substitute_variable_generic(
                                &rank_fn, &var_name, &first, ctx.factory(),
                            );

                            continuations.push(GenericContinuation::ProcessBestCandidate {
                                best: None,
                                best_rank: None,
                                remaining,
                                current: first,
                                var_name,
                                rank_fn,
                                env: env.clone(),
                                depth,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    }

                    // Evaluate if condition
                    GenericEvalStep::EvalIfCondition { condition, then_branch, else_branch, env, depth } => {
                        continuations.push(GenericContinuation::ProcessIfCondition {
                            then_branch,
                            else_branch,
                            env: env.clone(),
                            depth,
                        });

                        // 8.7: if-condition always expects Bool — prune non-Bool branches
                        work_stack.push(GenericWorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: Some(ctx.factory().atom("Bool")),
                        });
                    }

                    // Evaluate case atom
                    GenericEvalStep::EvalCaseAtom { atom, cases, env, depth } => {
                        continuations.push(GenericContinuation::ProcessCaseAtom {
                            cases,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Switch: pattern match WITHOUT evaluating atom
                    GenericEvalStep::SwitchAtom { atom, cases, env, depth } => {
                        // Switch does NOT evaluate atom - pattern match directly
                        match eval_switch_generic(&atom, &cases, ctx.factory()) {
                            GenericSwitchResult::Match(template, _bindings) => {
                                // Template needs evaluation
                                work_stack.push(GenericWorkItem::Eval {
                                    value: template,
                                    env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                });
                            }
                            GenericSwitchResult::Error(err) => {
                                work_stack.push(GenericWorkItem::Resume {
                                    result: (vec![err], env),
                                });
                            }
                            GenericSwitchResult::NoMatch => {
                                // No case matched - prune branch (MeTTa HE returns Empty)
                                work_stack.push(GenericWorkItem::Resume {
                                    result: (vec![], env),
                                });
                            }
                        }
                    }

                    // Evaluate eval
                    GenericEvalStep::EvalEval { arg, env, depth } => {
                        continuations.push(GenericContinuation::ProcessEvalEval {
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Evaluate return
                    GenericEvalStep::EvalReturn { value, env, depth } => {
                        continuations.push(GenericContinuation::ProcessReturn {
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start chain
                    GenericEvalStep::StartChain { expr, var, body, env, depth } => {
                        continuations.push(GenericContinuation::ProcessChainExpr {
                            var,
                            body,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start function
                    GenericEvalStep::StartFunction { expr, env, depth } => {
                        continuations.push(GenericContinuation::ProcessFunction {
                            iteration_count: 1,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Evaluate is-error
                    GenericEvalStep::EvalIsError { expr, env, depth } => {
                        continuations.push(GenericContinuation::ProcessIsError {
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start catch
                    GenericEvalStep::StartCatch { expr, default, env, depth } => {
                        continuations.push(GenericContinuation::ProcessCatch {
                            default,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start conjunction
                    GenericEvalStep::StartConjunction { goals, env, depth } => {
                        if goals.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![ctx.factory().unit()], env),
                            });
                        } else if goals.len() == 1 {
                            work_stack.push(GenericWorkItem::Eval {
                                value: goals.into_iter().next().expect("goals.len() == 1"),
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        } else {
                            let mut remaining = VecDeque::from(goals);
                            let first_goal = remaining.pop_front().expect("non-empty");

                            continuations.push(GenericContinuation::ProcessConjunction {
                                remaining_goals: remaining,
                                accumulated_results: Vec::new(),
                                env: env.clone(),
                                depth,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: first_goal,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    }

                    // Start unify
                    GenericEvalStep::StartUnify { pattern1, pattern2, success_body, failure_body, env, depth } => {
                        continuations.push(GenericContinuation::ProcessUnifyPattern1 {
                            pattern2,
                            success_body,
                            failure_body,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: pattern1,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start collapse
                    GenericEvalStep::StartCollapse { expr, env, depth } => {
                        continuations.push(GenericContinuation::ProcessCollapse {
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start collapse-bind
                    GenericEvalStep::StartCollapseBind { expr, env, depth } => {
                        continuations.push(GenericContinuation::ProcessCollapseBind {
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start amb
                    GenericEvalStep::StartAmb { alternatives, env, depth } => {
                        if alternatives.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], env),
                            });
                        } else {
                            let mut alts_deque: VecDeque<_> = alternatives.into_iter().collect();
                            let first = alts_deque.pop_front().expect("alternatives is non-empty");

                            continuations.push(GenericContinuation::ProcessAmb {
                                remaining_alts: alts_deque,
                                results: Vec::new(),
                                env: env.clone(),
                                depth,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    }

                    // Start guard
                    GenericEvalStep::StartGuard { condition, env, depth } => {
                        continuations.push(GenericContinuation::ProcessGuard {
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start get-atoms
                    GenericEvalStep::StartGetAtoms { space_ref, env, depth } => {
                        continuations.push(GenericContinuation::ProcessGetAtoms {
                            space_ref: space_ref.clone(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start memo
                    GenericEvalStep::StartMemo { memo_ref, expr, first_only, env, depth } => {
                        continuations.push(GenericContinuation::ProcessMemoTable {
                            memo_ref: memo_ref.clone(),
                            expr,
                            first_only,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start new-memo
                    GenericEvalStep::StartNewMemo { name_arg, size_arg, env, depth } => {
                        continuations.push(GenericContinuation::ProcessNewMemoName {
                            name_arg: name_arg.clone(),
                            size_arg,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: name_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start memo operation
                    GenericEvalStep::StartMemoOp { memo_ref, op_type, env, depth } => {
                        let is_clear = matches!(op_type, super::super::step::MemoOpType::Clear);
                        continuations.push(GenericContinuation::ProcessMemoOp {
                            memo_ref: memo_ref.clone(),
                            is_clear,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start match
                    GenericEvalStep::StartMatch { space_arg, pattern, template, env, depth } => {
                        continuations.push(GenericContinuation::ProcessMatchSpace {
                            space_arg: space_arg.clone(),
                            pattern,
                            template,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start add-atom
                    GenericEvalStep::StartAddAtom { space_ref, atom, env, depth } => {
                        continuations.push(GenericContinuation::ProcessAddAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start remove-atom
                    GenericEvalStep::StartRemoveAtom { space_ref, atom, env, depth } => {
                        continuations.push(GenericContinuation::ProcessRemoveAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start new-state
                    GenericEvalStep::StartNewState { initial_value, env, depth } => {
                        continuations.push(GenericContinuation::ProcessNewState {
                            initial_value: initial_value.clone(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: initial_value,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start get-state
                    GenericEvalStep::StartGetState { state_ref, env, depth } => {
                        continuations.push(GenericContinuation::ProcessGetState {
                            state_ref: state_ref.clone(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start change-state
                    GenericEvalStep::StartChangeState { state_ref, new_value, env, depth } => {
                        continuations.push(GenericContinuation::ProcessChangeStateRef {
                            state_ref: state_ref.clone(),
                            new_value,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start repr
                    GenericEvalStep::StartRepr { atom, env, depth } => {
                        continuations.push(GenericContinuation::ProcessRepr {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start format-args
                    GenericEvalStep::StartFormatArgs { format_arg, args_arg, env, depth } => {
                        continuations.push(GenericContinuation::ProcessFormatArgsString {
                            format_arg: format_arg.clone(),
                            args_arg,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: format_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start println
                    GenericEvalStep::StartPrintln { atom, env, depth } => {
                        continuations.push(GenericContinuation::ProcessPrintln {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start trace
                    GenericEvalStep::StartTrace { message, value_expr, env, depth } => {
                        continuations.push(GenericContinuation::ProcessTraceMessage {
                            message: message.clone(),
                            value_expr,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: message,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start get-metatype
                    GenericEvalStep::StartGetMetatype { atom, env, depth } => {
                        continuations.push(GenericContinuation::ProcessGetMetatype {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start bind
                    GenericEvalStep::StartBind { token, atom_expr, env, depth } => {
                        continuations.push(GenericContinuation::ProcessBind {
                            token,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom_expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start if-reducible: evaluate expr, then compare to original
                    GenericEvalStep::EvalIfReducible { expr, then_branch, else_branch, env, depth } => {
                        continuations.push(GenericContinuation::ProcessIfReducible {
                            original_expr: expr.clone(),
                            then_branch,
                            else_branch,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }

                    // Start match-or: evaluate space, then match with default fallback
                    GenericEvalStep::StartMatchOr { space_arg, pattern, default, template, env, depth } => {
                        continuations.push(GenericContinuation::ProcessMatchOrSpace {
                            space_arg: space_arg.clone(),
                            pattern,
                            default,
                            template,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }
                }
            }

            GenericWorkItem::Resume { result } => {
                // Take ownership of continuation for processing
                let cont = continuations.pop().expect("non-empty continuation stack");
                trace!(target: "mettatron::backend::eval::eval_trampoline_generic", ?cont, result_values = ?result.0, "resume work item");

                // Process continuation - delegate to continuation handler
                process_continuation_generic(
                    cont,
                    result,
                    &mut work_stack,
                    &mut continuations,
                    &mut final_result,
                    ctx,
                );
            }
        }
    }

    // Trace: EvalEnd with matching span_id and measured duration
    #[cfg(feature = "eval-trace")]
    {
        if let Some(tc) = ctx.trace_collector() {
            let result_count = final_result.as_ref().map_or(0, |r| r.0.len()) as u32;
            let end_ns = tc.elapsed_ns();
            let duration = end_ns.saturating_sub(_eval_start_ns);
            tc.emit_timed(
                trace_format::TraceTier::TreeWalker,
                0,
                trace_format::TraceValue::Unit,
                vec![],
                None,
                trace_format::TraceEventKind::EvalEnd { result_count },
                _eval_start_ns,
                Some(duration),
                Some(_eval_span_id),
            );
        }
    }

    // Clear thread-local trace collector before returning.
    #[cfg(feature = "eval-trace")]
    {
        crate::backend::trace::thread_local_sink::clear_thread_trace_collector();
    }

    // Return final result
    final_result.unwrap_or_else(|| (vec![], env))
}

/// Process a continuation with generic value types.
///
/// This function handles all continuation types, converting at boundaries
/// where necessary to interact with heap-based infrastructure (rules, environment).
fn process_continuation_generic<C: EvalContext>(
    cont: GenericContinuation<C::Value, ContextEnv<C>>,
    result: GenericEvalResult<C::Value, ContextEnv<C>>,
    work_stack: &mut Vec<GenericWorkItem<C::Value, ContextEnv<C>>>,
    continuations: &mut Vec<GenericContinuation<C::Value, ContextEnv<C>>>,
    final_result: &mut Option<GenericEvalResult<C::Value, ContextEnv<C>>>,
    ctx: &C,
) where
    C::Value: Clone,
{
    match cont {
        GenericContinuation::Done => {
            *final_result = Some(result);
        }

        GenericContinuation::CollectSExpr {
            mut remaining,
            mut collected,
            original_env,
            depth,
        } => {
            collected.push(result);

            if remaining.is_empty() {
                // All items evaluated, process collected results
                // Use generic version - zero conversion needed!
                let processed = process_collected_sexpr_generic(collected, original_env.clone(), depth, ctx.factory());

                match processed {
                    GenericProcessedSExpr::Done((results, env)) => {
                        work_stack.push(GenericWorkItem::Resume {
                            result: (results, env),
                        });
                    }
                    GenericProcessedSExpr::EvalRuleMatches { matches, env, depth, base_results } => {
                        if matches.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (base_results, env),
                            });
                        } else {
                            // Already generic types - no conversion needed!
                            let mut matches_deque = matches;
                            let _total = (matches_deque.len() + 1) as u32;
                            let (rhs, bindings) = matches_deque.pop_front().expect("matches is non-empty");

                            // Trace: BranchStart for the first branch
                            #[cfg(feature = "eval-trace")]
                            let (_branch_span, _branch_start) = {
                                if let Some(tc) = ctx.trace_collector() {
                                    if _total > 1 {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            trace_format::TraceValue::Unit,
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::NondeterministicFork { branch_count: _total },
                                        );
                                    }
                                    let sid = tc.next_span_id();
                                    let sns = tc.elapsed_ns();
                                    tc.emit_timed(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        trace_format::TraceValue::Unit,
                                        vec![],
                                        None,
                                        trace_format::TraceEventKind::BranchStart { branch_index: 0, total_branches: _total },
                                        sns, None, Some(sid),
                                    );
                                    (sid, sns)
                                } else { (0u64, 0u64) }
                            };

                            continuations.push(GenericContinuation::ProcessRuleMatches {
                                remaining_matches: matches_deque,
                                results: base_results,
                                env: env.clone(),
                                depth,
                                #[cfg(feature = "eval-trace")]
                                branch_span_id: _branch_span,
                                #[cfg(feature = "eval-trace")]
                                branch_start_ns: _branch_start,
                                #[cfg(feature = "eval-trace")]
                                branch_index: 0,
                                #[cfg(feature = "eval-trace")]
                                total_branches: _total,
                            });

                            // Apply generic bindings - no conversion needed
                            let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        }
                    }
                    GenericProcessedSExpr::EvalCombinations { combinations, env, depth } => {
                        continuations.push(GenericContinuation::ProcessCombinations {
                            combinations,
                            results: vec![],
                            pending_rule_matches: VecDeque::new(),
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![], env),
                        });
                    }
                    GenericProcessedSExpr::RedispatchSExpr { items, env, depth: redispatch_depth } => {
                        let sexpr = ctx.factory().sexpr(items);
                        work_stack.push(GenericWorkItem::Eval {
                            value: sexpr,
                            env,
                            depth: redispatch_depth,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }
                }
            } else {
                let next = remaining.pop_front().expect("remaining is non-empty");

                continuations.push(GenericContinuation::CollectSExpr {
                    remaining,
                    collected,
                    original_env: original_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next,
                    env: original_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessRuleMatches {
            mut remaining_matches,
            mut results,
            env: _,
            depth,
            #[cfg(feature = "eval-trace")]
            branch_span_id,
            #[cfg(feature = "eval-trace")]
            branch_start_ns,
            #[cfg(feature = "eval-trace")]
            branch_index,
            #[cfg(feature = "eval-trace")]
            total_branches,
        } => {
            let result_count = result.0.len() as u32;
            results.extend(result.0);
            let env = result.1;

            // Trace: BranchEnd for the branch that just completed
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let end_ns = tc.elapsed_ns();
                    let duration = end_ns.saturating_sub(branch_start_ns);
                    tc.emit_timed(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::BranchEnd {
                            branch_index,
                            result_count,
                        },
                        branch_start_ns,
                        Some(duration),
                        Some(branch_span_id),
                    );
                }
            }

            if remaining_matches.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    result: (results, env),
                });
            } else {
                // remaining_matches is already in generic type (V, GenericBindings<V>)
                let (rhs, bindings) = remaining_matches.pop_front().expect("remaining_matches is non-empty");

                // Trace: BranchStart for the next branch
                #[cfg(feature = "eval-trace")]
                let (_next_span_id, _next_start_ns, _next_branch_index) = {
                    if let Some(tc) = ctx.trace_collector() {
                        let next_idx = branch_index + 1;
                        let span_id = tc.next_span_id();
                        let start_ns = tc.elapsed_ns();
                        tc.emit_timed(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::BranchStart {
                                branch_index: next_idx,
                                total_branches,
                            },
                            start_ns,
                            None,
                            Some(span_id),
                        );
                        (span_id, start_ns, next_idx)
                    } else {
                        (0u64, 0u64, 0u32)
                    }
                };

                continuations.push(GenericContinuation::ProcessRuleMatches {
                    remaining_matches,
                    results,
                    env: env.clone(),
                    depth,
                    #[cfg(feature = "eval-trace")]
                    branch_span_id: _next_span_id,
                    #[cfg(feature = "eval-trace")]
                    branch_start_ns: _next_start_ns,
                    #[cfg(feature = "eval-trace")]
                    branch_index: _next_branch_index,
                    #[cfg(feature = "eval-trace")]
                    total_branches,
                });

                // Apply generic bindings - no conversion needed
                let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                // Trace: RuleApplication (tree-walker, subsequent match)
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let bindings_tv: Vec<(String, trace_format::TraceValue)> = bindings
                            .iter()
                            .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                            .collect();
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&rhs),
                            vec![crate::backend::trace::trace_value_generic(&instantiated_rhs)],
                            None,
                            trace_format::TraceEventKind::RuleApplication {
                                rule_lhs: crate::backend::trace::trace_value_generic(&rhs),
                                rule_rhs: crate::backend::trace::trace_value_generic(&instantiated_rhs),
                                bindings: bindings_tv,
                                rule_span: None,
                            },
                        );
                    }
                }

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated_rhs,
                    env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessGroundedOp {
            mut state,
            pending_arg_idx,
            env: _,
            depth,
        } => {
            let (result_values, result_env) = result;

            // Set evaluated arg using the stored arg_idx from the EvalArg return
            state.set_arg(pending_arg_idx, result_values);

            // Try static dispatch first - works with generic type V (NO conversion)
            let op_name = state.op_name.clone();
            if let Some(work) = execute_generic_grounded_op(&op_name, &mut state, ctx.factory()) {
                match work {
                    GenericGroundedWork::Done(results) => {
                        // Results are already in correct type V - NO conversion
                        let values: Vec<C::Value> = results
                            .into_iter()
                            .map(|(v, _)| v)
                            .collect();
                        work_stack.push(GenericWorkItem::Resume {
                            result: (values, result_env),
                        });
                    }
                    GenericGroundedWork::EvalArg { arg_idx, state: new_state } => {
                        continuations.push(GenericContinuation::ProcessGroundedOp {
                            state: new_state.clone(),
                            pending_arg_idx: arg_idx,
                            env: result_env.clone(),
                            depth,
                        });

                        // Arg already in correct type V - NO conversion
                        let arg_to_eval = new_state.args[arg_idx].clone();
                        work_stack.push(GenericWorkItem::Eval {
                            value: arg_to_eval,
                            env: result_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                        });
                    }
                    GenericGroundedWork::Error(e) => {
                        match e {
                            ExecError::NoReduce => {
                                // MeTTa HE semantics: return the original expression unreduced
                                let mut expr_parts = Vec::with_capacity(1 + state.args.len());
                                expr_parts.push(ctx.factory().atom(&state.op_name));
                                for arg in state.args.iter() {
                                    expr_parts.push(arg.clone());
                                }
                                let unreduced = ctx.factory().sexpr(expr_parts);
                                work_stack.push(GenericWorkItem::Resume {
                                    result: (vec![unreduced], result_env),
                                });
                            }
                            _ => {
                                let error_value = match e {
                                    ExecError::Runtime(msg) => ctx.factory().error(&msg, ctx.factory().atom("TypeError")),
                                    ExecError::Arithmetic(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArithmeticError")),
                                    ExecError::IncorrectArgument(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArityError")),
                                    ExecError::NoReduce => unreachable!(),
                                };
                                work_stack.push(GenericWorkItem::Resume {
                                    result: (vec![error_value], result_env),
                                });
                            }
                        }
                    }
                }
            } else {
                // Operation not in generic registry - report error
                // All 14 standard TCO operations are in the generic registry.
                let error_value = ctx.factory().error(
                    &format!("Grounded operation '{}' not found in generic registry", state.op_name),
                    ctx.factory().atom("OperationNotFoundError"),
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![error_value], result_env),
                });
            }
        }

        GenericContinuation::ProcessCombinations {
            mut combinations,
            mut results,
            mut pending_rule_matches,
            env,
            depth,
        } => {
            let (combo_results, result_env) = result;
            results.extend(combo_results);

            // Process pending rule matches first
            // pending_rule_matches is already in generic type (V, GenericBindings<V>)
            if let Some((rhs, bindings)) = pending_rule_matches.pop_front() {
                continuations.push(GenericContinuation::ProcessCombinations {
                    combinations,
                    results,
                    pending_rule_matches,
                    env: result_env.clone(),
                    depth,
                });

                // Apply generic bindings - no conversion needed
                let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated_rhs,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
                return;
            }

            // Get next combination
            if let Some(combo) = combinations.next() {
                // Generic iterator yields SmallVec<[V; 8]> - create generic sexpr
                let generic_sexpr = ctx.factory().sexpr(combo.to_vec());

                // Try to match rules using generic version - no conversion needed!
                let all_matches_with_types = try_match_all_rules_generic(&generic_sexpr, &result_env, *ctx.factory());

                if all_matches_with_types.is_empty() {
                    // No rule matches - expression is data
                    results.push(generic_sexpr);

                    continuations.push(GenericContinuation::ProcessCombinations {
                        combinations,
                        results,
                        pending_rule_matches,
                        env: result_env.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![], result_env),
                    });
                } else {
                    // Rules matched - evaluate them
                    // Strip rhs_type from 3-tuples → 2-tuples
                    let mut matches_deque: VecDeque<_> = all_matches_with_types
                        .into_iter()
                        .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                        .collect();
                    let (rhs, bindings) = matches_deque.pop_front().expect("non-empty");

                    continuations.push(GenericContinuation::ProcessCombinations {
                        combinations,
                        results,
                        pending_rule_matches: matches_deque,
                        env: result_env.clone(),
                        depth,
                    });

                    // Apply generic bindings - no conversion needed
                    let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                    work_stack.push(GenericWorkItem::Eval {
                        value: instantiated_rhs,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                    });
                }
            } else {
                // All combinations processed - results already contains generic values
                work_stack.push(GenericWorkItem::Resume {
                    result: (results, env),
                });
            }
        }

        GenericContinuation::ProcessLet {
            pending_values,
            pattern,
            body,
            mut results,
            env: _,
            depth,
        } => {
            let (result_values, result_env) = result;

            // Phase 8.5: Extract type constraint once for all values
            let type_constraint = extract_type_constraint(&pattern);

            match pending_values {
                None => {
                    // First resumption: result_values are values to pattern match

                    // Trace: value-result phase
                    #[cfg(feature = "eval-trace")]
                    {
                        if let Some(tc) = ctx.trace_collector() {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                depth as u32,
                                crate::backend::trace::trace_value_generic(&pattern),
                                result_values.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                None,
                                trace_format::TraceEventKind::SpecialForm {
                                    form_name: "let".to_string(),
                                    phase: "value-result".to_string(),
                                },
                            );
                        }
                    }

                    let mut values: VecDeque<C::Value> = result_values.into_iter().collect();

                    // Try to find a matching value
                    loop {
                        match values.pop_front() {
                            Some(value) => {
                                // Phase 8.5: Type pre-check for typed patterns (: $var Type)
                                // Only apply to ground-type values (Number/Bool/String) where
                                // type inference is definitive. S-expressions and atoms may
                                // structurally match the pattern even if type inference says otherwise.
                                if let Some(ref tc) = type_constraint {
                                    if get_ground_type(&value).is_some() {
                                        let value_type = infer_type_generic(&value, ctx.factory(), &result_env);
                                        if !types_match_with_subtypes(&value_type, tc, &result_env) {
                                            continue; // Type mismatch — skip
                                        }
                                    }
                                }
                                // Use generic pattern matching - NO conversion needed
                                if let Some(bindings) = pattern_match_generic(&pattern, &value) {
                                    // Trace: pattern-match phase
                                    #[cfg(feature = "eval-trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            tc.emit_converted(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                crate::backend::trace::trace_value_generic(&pattern),
                                                vec![crate::backend::trace::trace_value_generic(&value)],
                                                None,
                                                trace_format::TraceEventKind::SpecialForm {
                                                    form_name: "let".to_string(),
                                                    phase: "pattern-match".to_string(),
                                                },
                                            );
                                        }
                                    }

                                    // Pattern matches - instantiate body and evaluate
                                    let instantiated_body = apply_bindings_generic(&body, &bindings, ctx.factory());

                                    // Restore continuation for collecting more results
                                    continuations.push(GenericContinuation::ProcessLet {
                                        pending_values: Some(values),
                                        pattern,
                                        body,
                                        results,
                                        env: result_env.clone(),
                                        depth,
                                    });

                                    // Push body evaluation - THIS IS TAIL CALL (TCO)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: instantiated_body,
                                        env: result_env,
                                        depth, // TCO: reuse depth for body eval
                                        is_tail_call: true,
                                        expected_type: None,
                                    });
                                    return;
                                }
                                // Trace: pattern-no-match phase
                                #[cfg(feature = "eval-trace")]
                                {
                                    if let Some(tc) = ctx.trace_collector() {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            crate::backend::trace::trace_value_generic(&pattern),
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::SpecialForm {
                                                form_name: "let".to_string(),
                                                phase: "pattern-no-match".to_string(),
                                            },
                                        );
                                    }
                                }
                                // Pattern doesn't match - continue to next value
                            }
                            None => {
                                // No pattern matched - return results to parent
                                work_stack.push(GenericWorkItem::Resume {
                                    result: (results, result_env),
                                });
                                return;
                            }
                        }
                    }
                }
                Some(mut remaining_values) => {
                    // Subsequent resumption: result_values are body evaluation results
                    // Add body results to collected results
                    results.extend(result_values);

                    // Try next value
                    loop {
                        match remaining_values.pop_front() {
                            Some(value) => {
                                // Phase 8.5: Type pre-check for typed patterns (: $var Type)
                                // Only apply to ground-type values (Number/Bool/String) where
                                // type inference is definitive. S-expressions and atoms may
                                // structurally match the pattern even if type inference says otherwise.
                                if let Some(ref tc) = type_constraint {
                                    if get_ground_type(&value).is_some() {
                                        let value_type = infer_type_generic(&value, ctx.factory(), &result_env);
                                        if !types_match_with_subtypes(&value_type, tc, &result_env) {
                                            continue; // Type mismatch — skip
                                        }
                                    }
                                }
                                if let Some(bindings) = pattern_match_generic(&pattern, &value) {
                                    // Trace: pattern-match phase (subsequent resumption)
                                    #[cfg(feature = "eval-trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            tc.emit_converted(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                crate::backend::trace::trace_value_generic(&pattern),
                                                vec![crate::backend::trace::trace_value_generic(&value)],
                                                None,
                                                trace_format::TraceEventKind::SpecialForm {
                                                    form_name: "let".to_string(),
                                                    phase: "pattern-match".to_string(),
                                                },
                                            );
                                        }
                                    }

                                    // Pattern matches - evaluate body with bindings
                                    let instantiated_body = apply_bindings_generic(&body, &bindings, ctx.factory());

                                    // Restore continuation for collecting more results
                                    continuations.push(GenericContinuation::ProcessLet {
                                        pending_values: Some(remaining_values),
                                        pattern,
                                        body,
                                        results,
                                        env: result_env.clone(),
                                        depth,
                                    });

                                    // Push body evaluation - THIS IS TAIL CALL (TCO)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: instantiated_body,
                                        env: result_env,
                                        depth, // TCO: reuse depth for body eval
                                        is_tail_call: true,
                                        expected_type: None,
                                    });
                                    return;
                                }
                                // Trace: pattern-no-match phase (subsequent resumption)
                                #[cfg(feature = "eval-trace")]
                                {
                                    if let Some(tc) = ctx.trace_collector() {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            crate::backend::trace::trace_value_generic(&pattern),
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::SpecialForm {
                                                form_name: "let".to_string(),
                                                phase: "pattern-no-match".to_string(),
                                            },
                                        );
                                    }
                                }
                                // Pattern doesn't match - continue to next value
                            }
                            None => {
                                // All values processed - return results to parent
                                work_stack.push(GenericWorkItem::Resume {
                                    result: (results, result_env),
                                });
                                return;
                            }
                        }
                    }
                }
            }
        }

        GenericContinuation::CollectGroundedArg {
            items,
            grounded_indices,
            current_idx,
            mut evaluated_results,
            env: _,
            depth,
        } => {
            let (result_values, result_env) = result;

            // Store ALL results from evaluation to preserve nondeterminism.
            // A nondeterministic function like (f) → {1, 2, 3} produces 3 results.
            if result_values.is_empty() {
                evaluated_results.push(vec![]);
            } else {
                evaluated_results.push(result_values);
            }

            let next_idx = current_idx + 1;
            if next_idx < grounded_indices.len() {
                // More grounded args to evaluate
                let arg_idx = grounded_indices[next_idx];
                let arg_to_eval = items[arg_idx].clone();

                // Phase 9.2: Derive expected_type for next arg
                let arg_expected_type = derive_arg_expected_type::<C>(
                    &items, arg_idx, &result_env, ctx.factory(),
                );

                continuations.push(GenericContinuation::CollectGroundedArg {
                    items,
                    grounded_indices,
                    current_idx: next_idx,
                    evaluated_results,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: arg_to_eval,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: arg_expected_type,
                });
            } else {
                // All grounded args evaluated — compute Cartesian product of
                // nondeterministic results and evaluate each combination.

                // Check if any arg produced empty results
                if evaluated_results.iter().any(|r| r.is_empty()) {
                    // Empty result from any arg → no combinations possible
                    // (Cartesian product of anything × empty = empty)
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![], result_env),
                    });
                } else {
                    // Build all Cartesian product combinations as sexpr values.
                    // Each combination substitutes one result per grounded arg into items.
                    let mut combinations: VecDeque<C::Value> = VecDeque::new();

                    // Track whether any grounded arg changed after pre-evaluation.
                    // If no args changed (fixpoint), skip re-evaluation to prevent
                    // infinite loop on bloom filter false positives and data constructors.
                    let mut changed = false;
                    for (i, grounded_idx) in grounded_indices.iter().enumerate() {
                        if evaluated_results[i].len() != 1
                            || evaluated_results[i][0] != items[*grounded_idx]
                        {
                            changed = true;
                            break;
                        }
                    }

                    // Compute Cartesian product inline
                    // Start with a single empty combination (indices all 0)
                    let mut combo_indices: Vec<usize> = vec![0; evaluated_results.len()];
                    loop {
                        // Build this combination's items
                        let mut combo_items = items.clone();
                        for (i, grounded_idx) in grounded_indices.iter().enumerate() {
                            combo_items[*grounded_idx] = evaluated_results[i][combo_indices[i]].clone();
                        }
                        combinations.push_back(ctx.factory().sexpr(combo_items));

                        // Advance indices (mixed-radix increment)
                        let mut carry = true;
                        for i in (0..combo_indices.len()).rev() {
                            if carry {
                                combo_indices[i] += 1;
                                if combo_indices[i] < evaluated_results[i].len() {
                                    carry = false;
                                } else {
                                    combo_indices[i] = 0;
                                }
                            }
                        }
                        if carry {
                            break; // All combinations exhausted
                        }
                    }

                    if combinations.len() == 1 {
                        let sexpr = combinations.pop_front().expect("combinations is non-empty");

                        if !changed {
                            // Fixpoint: pre-evaluation didn't change any argument.
                            // This happens when the bloom filter produces a false positive
                            // (e.g., data constructors like `S`, `Z`, `Cons`), or when
                            // a head has facts but no rewrite rules.
                            //
                            // Instead of re-pushing for Eval (which would infinite-loop
                            // through the same bloom filter check), complete Steps 3-4
                            // that were skipped when Step 2 (EvalGroundedArgs) fired.

                            // Step 3: Try rule matching with the (unchanged) expression
                            let all_matches_with_types = try_match_all_rules_generic(
                                &sexpr, &result_env, *ctx.factory()
                            );

                            if !all_matches_with_types.is_empty() {
                                // Rules matched — evaluate RHS
                                // Strip rhs_type from 3-tuples → 2-tuples
                                let mut matches_deque: VecDeque<_> =
                                    all_matches_with_types.into_iter()
                                        .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                                        .collect();
                                let _total_app = (matches_deque.len() + 1) as u32;
                                let (rhs, bindings) =
                                    matches_deque.pop_front().expect("matches is non-empty");

                                #[cfg(feature = "eval-trace")]
                                let (_app_span, _app_start) = {
                                    if let Some(tc) = ctx.trace_collector() {
                                        if _total_app > 1 {
                                            tc.emit_converted(
                                                trace_format::TraceTier::TreeWalker, depth as u32,
                                                trace_format::TraceValue::Unit, vec![], None,
                                                trace_format::TraceEventKind::NondeterministicFork { branch_count: _total_app },
                                            );
                                        }
                                        let sid = tc.next_span_id();
                                        let sns = tc.elapsed_ns();
                                        tc.emit_timed(
                                            trace_format::TraceTier::TreeWalker, depth as u32,
                                            trace_format::TraceValue::Unit, vec![], None,
                                            trace_format::TraceEventKind::BranchStart { branch_index: 0, total_branches: _total_app },
                                            sns, None, Some(sid),
                                        );
                                        (sid, sns)
                                    } else { (0u64, 0u64) }
                                };

                                continuations.push(
                                    GenericContinuation::ProcessRuleMatches {
                                        remaining_matches: matches_deque,
                                        results: vec![],
                                        env: result_env.clone(),
                                        depth,
                                        #[cfg(feature = "eval-trace")]
                                        branch_span_id: _app_span,
                                        #[cfg(feature = "eval-trace")]
                                        branch_start_ns: _app_start,
                                        #[cfg(feature = "eval-trace")]
                                        branch_index: 0,
                                        #[cfg(feature = "eval-trace")]
                                        total_branches: _total_app,
                                    },
                                );

                                let instantiated_rhs = apply_bindings_generic(
                                    &rhs, &bindings, ctx.factory()
                                );

                                work_stack.push(GenericWorkItem::Eval {
                                    value: instantiated_rhs,
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                });
                            } else {
                                // Step 4: No rules matched — return as data constructor
                                work_stack.push(GenericWorkItem::Resume {
                                    result: (vec![sexpr], result_env),
                                });
                            }
                        } else {
                            // Args changed — safe to re-evaluate with new arg values
                            work_stack.push(GenericWorkItem::Eval {
                                value: sexpr,
                                env: result_env,
                                depth,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        }
                    } else {
                        // Multiple combinations — evaluate each and collect results
                        let first = combinations.pop_front().expect("combinations is non-empty");

                        continuations.push(GenericContinuation::CollectApplicativeResults {
                            remaining: combinations,
                            results: vec![],
                            env: result_env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: first,
                            env: result_env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }
                }
            }
        }

        GenericContinuation::CollectApplicativeResults {
            mut remaining,
            mut results,
            env: _,
            depth,
        } => {
            let (result_values, result_env) = result;
            results.extend(result_values);

            if remaining.is_empty() {
                // All combinations evaluated — resume parent with collected results
                work_stack.push(GenericWorkItem::Resume {
                    result: (results, result_env),
                });
            } else {
                // Evaluate next combination
                let next = remaining.pop_front().expect("remaining is non-empty");

                continuations.push(GenericContinuation::CollectApplicativeResults {
                    remaining,
                    results,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessMapAtom {
            mut remaining_elements,
            var_name,
            template,
            mut collected_results,
            env: _,
            depth,
        } => {
            let (mut result_values, result_env) = result;

            // Add first result from evaluation
            if result_values.is_empty() {
                collected_results.push(ctx.factory().unit());
            } else {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![first_result], result_env),
                    });
                    return;
                }
                collected_results.push(first_result);
            }

            if remaining_elements.is_empty() {
                // All elements processed - return result list
                let result_list = ctx.factory().sexpr(collected_results);
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![result_list], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.pop_front().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &template, &var_name, &next_element, ctx.factory(),
                );

                continuations.push(GenericContinuation::ProcessMapAtom {
                    remaining_elements,
                    var_name,
                    template,
                    collected_results,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessFilterAtom {
            current_element,
            mut remaining_elements,
            var_name,
            predicate,
            mut filtered_results,
            env: _,
            depth,
        } => {
            let (mut result_values, result_env) = result;

            // Check predicate result and optionally include current element
            if !result_values.is_empty() {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![first_result], result_env),
                    });
                    return;
                }

                let should_include = if let Some(b) = first_result.as_bool() {
                    b
                } else {
                    !first_result.is_unit()
                };

                if should_include {
                    if let Some(elem) = current_element {
                        filtered_results.push(elem);
                    }
                }
            }

            if remaining_elements.is_empty() {
                // All elements processed - return filtered list
                let result_list = ctx.factory().sexpr(filtered_results);
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![result_list], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.pop_front().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &predicate, &var_name, &next_element, ctx.factory(),
                );

                continuations.push(GenericContinuation::ProcessFilterAtom {
                    current_element: Some(next_element),
                    remaining_elements,
                    var_name,
                    predicate,
                    filtered_results,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessFoldlAtom {
            mut remaining_elements,
            acc_var_name,
            item_var_name,
            operation,
            env: _,
            depth,
        } => {
            let (mut result_values, result_env) = result;

            // Get the new accumulator value from the result
            let accumulator = if result_values.is_empty() {
                ctx.factory().unit()
            } else {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![first_result], result_env),
                    });
                    return;
                }
                first_result
            };

            if remaining_elements.is_empty() {
                // All elements processed - return final accumulator
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![accumulator], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.pop_front().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &operation, &acc_var_name, &accumulator, ctx.factory(),
                );
                let instantiated = substitute_variable_generic(
                    &instantiated, &item_var_name, &next_element, ctx.factory(),
                );

                continuations.push(GenericContinuation::ProcessFoldlAtom {
                    remaining_elements,
                    acc_var_name,
                    item_var_name,
                    operation,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessSortTuple {
            mut sorted,
            mut unsorted,
            current,
            insert_pos,
            var1_name,
            var2_name,
            comparator,
            env: _,
            depth,
        } => {
            let (cmp_results, result_env) = result;

            // Extract boolean comparison result
            let cmp_true = cmp_results.first()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if cmp_true {
                // current < sorted[insert_pos]: insert current here
                sorted.insert(insert_pos, current);
            } else {
                // current >= sorted[insert_pos]: try next position
                let next_pos = insert_pos + 1;
                if next_pos < sorted.len() {
                    // Compare current vs sorted[next_pos]
                    let instantiated = substitute_variable_generic(
                        &comparator, &var1_name, &current, ctx.factory(),
                    );
                    let instantiated = substitute_variable_generic(
                        &instantiated, &var2_name, &sorted[next_pos], ctx.factory(),
                    );

                    continuations.push(GenericContinuation::ProcessSortTuple {
                        sorted,
                        unsorted,
                        current,
                        insert_pos: next_pos,
                        var1_name,
                        var2_name,
                        comparator,
                        env: result_env.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                    });
                    return;
                } else {
                    // current >= all sorted elements: insert at end
                    sorted.push(current);
                }
            }

            // Move to next unsorted element
            if unsorted.is_empty() {
                // Sorting complete
                let result_tuple = ctx.factory().sexpr(sorted);
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![result_tuple], result_env),
                });
            } else {
                let next_current = unsorted.remove(0);

                // Compare next_current vs sorted[0]
                let instantiated = substitute_variable_generic(
                    &comparator, &var1_name, &next_current, ctx.factory(),
                );
                let instantiated = substitute_variable_generic(
                    &instantiated, &var2_name, &sorted[0], ctx.factory(),
                );

                continuations.push(GenericContinuation::ProcessSortTuple {
                    sorted,
                    unsorted,
                    current: next_current,
                    insert_pos: 0,
                    var1_name,
                    var2_name,
                    comparator,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessBestCandidate {
            best,
            best_rank,
            mut remaining,
            current,
            var_name,
            rank_fn,
            env: _,
            depth,
        } => {
            let (rank_results, result_env) = result;

            // Extract numeric rank from evaluation result
            let current_rank = rank_results.first().and_then(|v| {
                v.as_float().or_else(|| v.as_long().map(|l| l as f64))
            });

            // Determine new best
            let (new_best, new_best_rank) = match (current_rank, best_rank) {
                (Some(cr), Some(br)) if cr > br => (current, Some(cr)),
                (Some(cr), None) => (current, Some(cr)),
                _ => (best.unwrap_or(current), best_rank),
            };

            if remaining.is_empty() {
                // Done — return best
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![new_best], result_env),
                });
            } else {
                // Evaluate next element's rank
                let next = remaining.remove(0);

                let instantiated = substitute_variable_generic(
                    &rank_fn, &var_name, &next, ctx.factory(),
                );

                continuations.push(GenericContinuation::ProcessBestCandidate {
                    best: Some(new_best),
                    best_rank: new_best_rank,
                    remaining,
                    current: next,
                    var_name,
                    rank_fn,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessIfCondition {
            then_branch,
            else_branch,
            env: _,
            depth,
        } => {
            let (cond_results, env_after_cond) = result;

            if let Some(first) = cond_results.first() {
                // Trace: condition-result phase
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(first),
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: "if".to_string(),
                                phase: "condition-result".to_string(),
                            },
                        );
                    }
                }

                // Check for error in condition
                if first.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![first.clone()], env_after_cond),
                    });
                    return;
                }

                // MeTTa HE semantics: if is pure pattern matching on True/False.
                // Bool(true) → then branch, Bool(false) → else branch,
                // Everything else (Unit, atoms, S-exprs) → return unreduced.
                if let Some(is_true) = first.as_bool() {
                    let branch = if is_true {
                        // Trace: then-branch phase
                        #[cfg(feature = "eval-trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(first),
                                    vec![crate::backend::trace::trace_value_generic(&then_branch)],
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "if".to_string(),
                                        phase: "then-branch".to_string(),
                                    },
                                );
                            }
                        }
                        then_branch
                    } else {
                        // Trace: else-branch phase
                        #[cfg(feature = "eval-trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(first),
                                    vec![crate::backend::trace::trace_value_generic(&else_branch)],
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "if".to_string(),
                                        phase: "else-branch".to_string(),
                                    },
                                );
                            }
                        }
                        else_branch
                    };
                    work_stack.push(GenericWorkItem::Eval {
                        value: branch,
                        env: env_after_cond,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                    });
                } else {
                    // Non-boolean (including Unit) → return unreduced (if cond then else)
                    // Trace: non-boolean phase
                    #[cfg(feature = "eval-trace")]
                    {
                        if let Some(tc) = ctx.trace_collector() {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                depth as u32,
                                crate::backend::trace::trace_value_generic(first),
                                vec![],
                                None,
                                trace_format::TraceEventKind::SpecialForm {
                                    form_name: "if".to_string(),
                                    phase: "non-boolean".to_string(),
                                },
                            );
                        }
                    }
                    let unreduced = ctx.factory().sexpr(vec![
                        ctx.factory().atom("if"),
                        first.clone(),
                        then_branch,
                        else_branch,
                    ]);
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![unreduced], env_after_cond),
                    });
                }
            } else {
                // MeTTa HE: if-condition produced zero results → entire if produces zero results.
                // This is branch annihilation: an empty condition means the if-expression
                // contributes nothing to the nondeterministic result set.
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![], env_after_cond),
                });
            }
        }

        GenericContinuation::ProcessCaseAtom {
            cases,
            env: _,
            depth,
        } => {
            let (atom_results, atom_env) = result;

            // Trace: scrutinee-result phase
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&cases),
                        atom_results.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: "case".to_string(),
                            phase: "scrutinee-result".to_string(),
                        },
                    );
                }
            }

            // Filter out Empty sentinels
            let filtered_results: Vec<_> = atom_results
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect();

            // Handle case when evaluation returns no results
            if filtered_results.is_empty() {
                // Match Empty against cases - NO conversion needed
                let empty_atom = ctx.factory().atom("Empty");
                match eval_switch_generic(&empty_atom, &cases, ctx.factory()) {
                    GenericSwitchResult::Match(template, _bindings) => {
                        // Template needs evaluation
                        work_stack.push(GenericWorkItem::Eval {
                            value: template,
                            env: atom_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                        });
                    }
                    GenericSwitchResult::Error(err) => {
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![err], atom_env),
                        });
                    }
                    GenericSwitchResult::NoMatch => {
                        // No case matched - prune branch (MeTTa HE returns Empty)
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![], atom_env),
                        });
                    }
                }
                return;
            }

            // MeTTa HE collapse semantics: fully evaluate each scrutinee result
            // before pattern matching. In HE, case is defined as:
            //   (= (case $atom $cases)
            //      (let $c (collapse $atom)
            //        (if (== (noeval $c) ())
            //          (id (switch-minimal Empty $cases))
            //          (chain (eval (superpose $c)) $e (id (switch-minimal $e $cases))))))
            // The `collapse` fully evaluates the scrutinee (including rule application
            // for each nondeterministic result), then `switch-minimal` matches against
            // the already-evaluated results. We mirror this by evaluating each raw
            // scrutinee result before matching.
            let mut remaining_raw: VecDeque<C::Value> = filtered_results.into_iter().collect();
            let first_raw = remaining_raw.pop_front().expect("filtered_results is non-empty");

            continuations.push(GenericContinuation::ProcessCaseEvalScrutineeResults {
                remaining_raw,
                evaluated: vec![],
                cases,
                env: atom_env.clone(),
                depth,
            });

            work_stack.push(GenericWorkItem::Eval {
                value: first_raw,
                env: atom_env,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
            });
        }

        GenericContinuation::ProcessCaseMultiResults {
            mut remaining_atoms,
            cases,
            mut collected,
            env,
            depth,
        } => {
            let (results, _result_env) = result;
            collected.extend(results);

            if let Some(next_atom) = remaining_atoms.pop_front() {
                // Check if atom is empty - use trait methods, NO conversion
                let is_empty_atom = next_atom.is_empty()
                    || next_atom.as_sexpr().map_or(false, |items| items.is_empty());
                let switch_atom = if is_empty_atom {
                    ctx.factory().atom("Empty")
                } else {
                    next_atom
                };

                // Phase 8.6: Type-driven case pattern skipping.
                // If the scrutinee has a known ground type, filter case patterns
                // to only type-compatible ones. This avoids unnecessary pattern
                // matching against structurally incompatible patterns.
                let effective_cases = if let Some(scrutinee_type) = get_ground_type(&switch_atom) {
                    if let Some(case_pairs) = cases.as_sexpr() {
                        let filtered: Vec<C::Value> = case_pairs.iter().filter(|pair| {
                            pair.as_sexpr().map_or(true, |p| {
                                p.first().map_or(true, |pattern| {
                                    is_pattern_type_compatible(pattern, scrutinee_type)
                                })
                            })
                        }).cloned().collect();
                        if filtered.len() < case_pairs.len() {
                            // Some patterns were skipped — use filtered cases
                            ctx.factory().sexpr(filtered)
                        } else {
                            cases.clone() // No change — use original
                        }
                    } else {
                        cases.clone()
                    }
                } else {
                    cases.clone()
                };

                // Use generic switch - NO conversion needed
                match eval_switch_generic(&switch_atom, &effective_cases, ctx.factory()) {
                    GenericSwitchResult::Match(template, _bindings) => {
                        continuations.push(GenericContinuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: template,
                            env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                        });
                    }
                    GenericSwitchResult::Error(err) => {
                        // Collect error and continue
                        collected.push(err);

                        continuations.push(GenericContinuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![], env),
                        });
                    }
                    GenericSwitchResult::NoMatch => {
                        // No match - continue to next atom
                        continuations.push(GenericContinuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![], env),
                        });
                    }
                }
            } else {
                // All atoms processed
                work_stack.push(GenericWorkItem::Resume {
                    result: (collected, env),
                });
            }
        }

        // MeTTa HE collapse semantics: evaluate each raw scrutinee result to
        // normal form before pattern matching. This continuation sequentially
        // evaluates each raw result, collects the evaluated outputs, and then
        // performs the switch/pattern-match phase once all evaluations complete.
        GenericContinuation::ProcessCaseEvalScrutineeResults {
            mut remaining_raw,
            mut evaluated,
            cases,
            env: _,
            depth,
        } => {
            let (eval_results, eval_env) = result;

            // Collect non-empty evaluated results
            evaluated.extend(eval_results.into_iter().filter(|v| !v.is_empty()));

            if let Some(next_raw) = remaining_raw.pop_front() {
                // More raw scrutinee results to evaluate — reuse cont slot
                continuations.push(GenericContinuation::ProcessCaseEvalScrutineeResults {
                    remaining_raw,
                    evaluated,
                    cases,
                    env: eval_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next_raw,
                    env: eval_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                });
            } else {
                // All raw results evaluated — now perform pattern matching
                if evaluated.is_empty() {
                    // All evaluations produced empty — match Empty against cases
                    let empty_atom = ctx.factory().atom("Empty");
                    match eval_switch_generic(&empty_atom, &cases, ctx.factory()) {
                        GenericSwitchResult::Match(template, _bindings) => {
                            work_stack.push(GenericWorkItem::Eval {
                                value: template,
                                env: eval_env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        }
                        GenericSwitchResult::Error(err) => {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![err], eval_env),
                            });
                        }
                        GenericSwitchResult::NoMatch => {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], eval_env),
                            });
                        }
                    }
                    return;
                }

                // Match each evaluated result against cases
                let mut eval_atoms: VecDeque<C::Value> = evaluated.into_iter().collect();

                if let Some(first_atom) = eval_atoms.pop_front() {
                    let is_empty_atom = first_atom.is_empty()
                        || first_atom.as_sexpr().map_or(false, |items| items.is_empty());
                    let switch_atom = if is_empty_atom {
                        ctx.factory().atom("Empty")
                    } else {
                        first_atom
                    };

                    match eval_switch_generic(&switch_atom, &cases, ctx.factory()) {
                        GenericSwitchResult::Match(template, _bindings) => {
                            // Trace: case-match phase
                            #[cfg(feature = "eval-trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&switch_atom),
                                        vec![crate::backend::trace::trace_value_generic(&template)],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "case".to_string(),
                                            phase: "case-match".to_string(),
                                        },
                                    );
                                }
                            }

                            if eval_atoms.is_empty() {
                                work_stack.push(GenericWorkItem::Eval {
                                    value: template,
                                    env: eval_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                });
                            } else {
                                continuations.push(GenericContinuation::ProcessCaseMultiResults {
                                    remaining_atoms: eval_atoms,
                                    cases,
                                    collected: vec![],
                                    env: eval_env.clone(),
                                    depth,
                                });

                                work_stack.push(GenericWorkItem::Eval {
                                    value: template,
                                    env: eval_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                });
                            }
                        }
                        GenericSwitchResult::Error(err) => {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![err], eval_env),
                            });
                        }
                        GenericSwitchResult::NoMatch => {
                            // Trace: case-no-match phase
                            #[cfg(feature = "eval-trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&switch_atom),
                                        vec![],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "case".to_string(),
                                            phase: "case-no-match".to_string(),
                                        },
                                    );
                                }
                            }

                            // No case matched — prune branch (MeTTa HE returns Empty)
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], eval_env),
                            });
                        }
                    }
                }
            }
        }

        GenericContinuation::ProcessEvalEval {
            env: _,
            depth,
        } => {
            let (eval_results, result_env) = result;

            if eval_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![], result_env),
                });
            } else if eval_results.len() == 1 {
                // Single result - evaluate it (TCO)
                // Unwrap Quoted values: (eval (quote X)) → evaluate X.
                // Quoted is self-evaluating, so without this unwrap we'd loop.
                let mut value = eval_results.into_iter().next().unwrap();
                if let Some(inner) = value.as_quoted() {
                    value = inner;
                }
                work_stack.push(GenericWorkItem::Eval {
                    value,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            } else {
                // Multiple results - evaluate each (unwrap Quoted values)
                let mut results_deque: VecDeque<_> = eval_results.into_iter().map(|v| {
                    if let Some(inner) = v.as_quoted() { inner } else { v }
                }).collect();
                let first = results_deque.pop_front().unwrap();

                continuations.push(GenericContinuation::ProcessAmb {
                    remaining_alts: results_deque,
                    results: vec![],
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: first,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessReturn {
            env: _,
            depth: _,
        } => {
            let (arg_results, arg_env) = result;

            // Check for errors first - pass through without wrapping
            if let Some(err) = arg_results.iter().find(|r| r.is_error()) {
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err.clone()], arg_env),
                });
            } else {
                // Wrap results in return structure: (return value)
                let return_results: Vec<C::Value> = arg_results
                    .into_iter()
                    .map(|r| {
                        ctx.factory().sexpr(vec![
                            ctx.factory().atom("return"),
                            r,
                        ])
                    })
                    .collect();
                work_stack.push(GenericWorkItem::Resume {
                    result: (return_results, arg_env),
                });
            }
        }

        GenericContinuation::ProcessChainExpr {
            var,
            body,
            env: _,
            depth,
        } => {
            let (expr_results, result_env) = result;

            // Trace: expr-result or expr-empty phase
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let phase = if expr_results.is_empty() { "expr-empty" } else { "expr-result" };
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&var),
                        expr_results.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: "chain".to_string(),
                            phase: phase.to_string(),
                        },
                    );
                }
            }

            if expr_results.is_empty() {
                // Empty result — produce zero results (branch annihilation, HE-compatible)
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![], result_env),
                });
            } else if expr_results.len() == 1 {
                // Single result - substitute and evaluate body - NO conversion needed
                let var_name = var.as_atom().unwrap_or("");
                let instantiated = substitute_variable_generic(
                    &body,
                    var_name,
                    &expr_results[0],
                    ctx.factory(),
                );

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            } else {
                // Multiple results - chain evaluates each
                let mut results_deque: VecDeque<_> = expr_results.into_iter().collect();
                let first = results_deque.pop_front().unwrap();

                continuations.push(GenericContinuation::ProcessChainBody {
                    remaining_values: results_deque,
                    var: var.clone(),
                    body: body.clone(),
                    results: vec![],
                    env: result_env.clone(),
                    depth,
                });

                // Substitute variable generically - NO conversion needed
                let var_name = var.as_atom().unwrap_or("");
                let instantiated = substitute_variable_generic(
                    &body,
                    var_name,
                    &first,
                    ctx.factory(),
                );

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessChainBody {
            mut remaining_values,
            var,
            body,
            mut results,
            env,
            depth,
        } => {
            let (body_results, _result_env) = result;
            results.extend(body_results);

            if let Some(next_value) = remaining_values.pop_front() {
                continuations.push(GenericContinuation::ProcessChainBody {
                    remaining_values,
                    var: var.clone(),
                    body: body.clone(),
                    results,
                    env: env.clone(),
                    depth,
                });

                // Substitute variable generically - NO conversion needed
                let var_name = var.as_atom().unwrap_or("");
                let instantiated = substitute_variable_generic(
                    &body,
                    var_name,
                    &next_value,
                    ctx.factory(),
                );

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            } else {
                work_stack.push(GenericWorkItem::Resume {
                    result: (results, env),
                });
            }
        }

        GenericContinuation::ProcessFunction {
            iteration_count,
            env: _,
            depth,
        } => {
            const MAX_ITERATIONS: usize = 1000;
            let (eval_results, current_env) = result;

            if eval_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![ctx.factory().unit()], current_env),
                });
            } else {
                // Helper: check if a value is a (return ...) expression
                fn is_return_expr<V: MettaValueTrait>(v: &V) -> bool {
                    if let Some(items) = v.as_sexpr() {
                        if items.len() == 2 {
                            if let Some(s) = items[0].as_atom() {
                                return s == "return";
                            }
                        }
                    }
                    false
                }

                // Partition into return values and continue expressions
                let (final_results, continue_exprs): (Vec<_>, Vec<_>) =
                    eval_results.into_iter().partition(|r| is_return_expr(r));

                if !final_results.is_empty() {
                    // Extract return values - unwrap (return value) to just value
                    let returns: Vec<C::Value> = final_results
                        .into_iter()
                        .map(|r| {
                            if let Some(items) = r.as_sexpr() {
                                items[1].clone()
                            } else {
                                r // shouldn't happen, but be safe
                            }
                        })
                        .collect();
                    work_stack.push(GenericWorkItem::Resume {
                        result: (returns, current_env),
                    });
                } else if continue_exprs.is_empty() {
                    // Nothing to continue
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![ctx.factory().unit()], current_env),
                    });
                } else if iteration_count >= MAX_ITERATIONS {
                    // Hit iteration limit
                    work_stack.push(GenericWorkItem::Resume {
                        result: (continue_exprs, current_env),
                    });
                } else {
                    // Continue evaluating
                    if continue_exprs.len() == 1 {
                        let next_expr = continue_exprs.into_iter().next().unwrap();
                        continuations.push(GenericContinuation::ProcessFunction {
                            iteration_count: iteration_count + 1,
                            env: current_env.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: next_expr,
                            env: current_env,
                            depth, // TCO: reuse depth for iteration
                            is_tail_call: false,
                            expected_type: None,
                        });
                    } else {
                        // Multiple continue expressions - just return them
                        // (more complex handling would evaluate each, but this matches heap engine)
                        work_stack.push(GenericWorkItem::Resume {
                            result: (continue_exprs, current_env),
                        });
                    }
                }
            }
        }

        GenericContinuation::ProcessIsError {
            env: _,
            depth: _,
        } => {
            let (expr_results, result_env) = result;

            let is_error = expr_results.iter().any(|v| v.is_error());
            let result_value = ctx.factory().bool(is_error);

            work_stack.push(GenericWorkItem::Resume {
                result: (vec![result_value], result_env),
            });
        }

        GenericContinuation::ProcessCatch {
            default,
            env: _,
            depth,
        } => {
            let (expr_results, result_env) = result;

            // Check if any result is an error
            let has_error = expr_results.iter().any(|v| v.is_error());

            if has_error {
                // Evaluate default value
                work_stack.push(GenericWorkItem::Eval {
                    value: default,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            } else {
                // No error - return original results
                work_stack.push(GenericWorkItem::Resume {
                    result: (expr_results, result_env),
                });
            }
        }

        GenericContinuation::ProcessConjunction {
            mut remaining_goals,
            mut accumulated_results,
            env: _,
            depth,
        } => {
            let (goal_results, result_env) = result;

            // Check for error or empty result
            if goal_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![], result_env),
                });
                return;
            }

            if goal_results.iter().any(|v| v.is_error()) {
                let error = goal_results.into_iter().find(|v| v.is_error()).unwrap();
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![error], result_env),
                });
                return;
            }

            accumulated_results.extend(goal_results);

            if let Some(next_goal) = remaining_goals.pop_front() {
                continuations.push(GenericContinuation::ProcessConjunction {
                    remaining_goals,
                    accumulated_results,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next_goal,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                });
            } else {
                // All goals evaluated - return last result
                let final_result = accumulated_results.pop().unwrap_or_else(|| ctx.factory().unit());
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![final_result], result_env),
                });
            }
        }

        GenericContinuation::ProcessUnifyPattern1 {
            pattern2,
            success_body,
            failure_body,
            env: _,
            depth,
        } => {
            let (pattern1_results, result_env) = result;

            if pattern1_results.is_empty() {
                // Empty - evaluate failure body
                work_stack.push(GenericWorkItem::Eval {
                    value: failure_body,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            } else if pattern1_results.len() == 1 {
                // Single result - check if it's a Space (special handling)
                let val1 = pattern1_results.into_iter().next().unwrap();

                if let Some(handle) = val1.as_space() {
                    // Space unification - match pattern2 against space atoms
                    // Check if this is a simple boolean check using generic trait methods
                    // (NO heap conversion needed for this check)
                    let is_boolean_check = is_boolean_check_pattern(&success_body, &failure_body);

                    if is_boolean_check {
                        // Simple existence check - use generic method (no call-site conversion)
                        let exists = if handle.is_module_space() || handle.name == "self" {
                            // Module/self spaces use Environment's generic match_space_exists
                            result_env.match_space_exists(&pattern2)
                        } else {
                            // Non-module spaces use SpaceHandle's generic collapse
                            let atoms: Vec<C::Value> = handle.collapse_generic(ctx.factory());
                            atoms.iter().any(|atom| {
                                pattern_match_generic(&pattern2, atom).is_some()
                                    || pattern_match_generic(atom, &pattern2).is_some()
                            })
                        };
                        let result_value = ctx.factory().bool(exists);
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![result_value], result_env),
                        });
                    } else {
                        // Full space matching with body evaluation
                        if handle.is_module_space() || handle.name == "self" {
                            // Module/self spaces use Environment's match_space
                            let matches: Vec<(C::Value, usize)> =
                                result_env.match_space(&pattern2, &pattern2)
                                    .into_iter()
                                    .map(|m| (m.value, m.count))
                                    .collect();

                            if matches.is_empty() {
                                // No matches - evaluate failure body
                                work_stack.push(GenericWorkItem::Eval {
                                    value: failure_body,
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                });
                            } else {
                                // Build bodies to evaluate for each match - values already generic
                                let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                                let mut found_match = false;
                                for (generic_value, count) in &matches {
                                    if let Some(bindings) = pattern_match_generic(&pattern2, generic_value) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..*count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    } else if let Some(bindings) = pattern_match_generic(generic_value, &pattern2) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..*count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    }
                                }

                                // If no pattern matched, evaluate failure body
                                if !found_match {
                                    bodies_to_eval.push_back(failure_body.clone());
                                }

                                if let Some(first_body) = bodies_to_eval.pop_front() {
                                    if bodies_to_eval.is_empty() {
                                        // Single body - tail call directly
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth,
                                            is_tail_call: true,
                                            expected_type: None,
                                        });
                                    } else {
                                        // Multiple bodies - use ProcessUnifyBodies
                                        continuations.push(GenericContinuation::ProcessUnifyBodies {
                                            remaining_bodies: bodies_to_eval,
                                            results: vec![],
                                            env: result_env.clone(),
                                            depth,
                                        });
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth: depth + 1,
                                            is_tail_call: false,
                                            expected_type: None,
                                        });
                                    }
                                } else {
                                    // No bodies at all (shouldn't happen, but handle gracefully)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: failure_body,
                                        env: result_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                    });
                                }
                            }
                        } else {
                            // GENERIC: Non-module spaces - use generic collapse_with_multiplicity

                            let matches: Vec<GenericMultiplicityMatch<C::Value>> =
                                handle.collapse_with_multiplicity_generic(ctx.factory());

                            if matches.is_empty() {
                                // No matches - evaluate failure body
                                work_stack.push(GenericWorkItem::Eval {
                                    value: failure_body,
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                });
                            } else {
                                // Build bodies to evaluate for each match - NO conversion needed
                                let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                                let mut found_match = false;
                                for m in &matches {
                                    if let Some(bindings) = pattern_match_generic(&pattern2, &m.value) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..m.count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    } else if let Some(bindings) = pattern_match_generic(&m.value, &pattern2) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..m.count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    }
                                }

                                // If no pattern matched, evaluate failure body
                                if !found_match {
                                    bodies_to_eval.push_back(failure_body.clone());
                                }

                                if let Some(first_body) = bodies_to_eval.pop_front() {
                                    if bodies_to_eval.is_empty() {
                                        // Single body - tail call directly
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth,
                                            is_tail_call: true,
                                            expected_type: None,
                                        });
                                    } else {
                                        // Multiple bodies - use ProcessUnifyBodies
                                        continuations.push(GenericContinuation::ProcessUnifyBodies {
                                            remaining_bodies: bodies_to_eval,
                                            results: vec![],
                                            env: result_env.clone(),
                                            depth,
                                        });
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth: depth + 1,
                                            is_tail_call: false,
                                            expected_type: None,
                                        });
                                    }
                                } else {
                                    // No bodies at all (shouldn't happen, but handle gracefully)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: failure_body,
                                        env: result_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                    });
                                }
                            }
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    continuations.push(GenericContinuation::ProcessUnifyPattern2 {
                        val1,
                        pattern2: pattern2.clone(),
                        success_body,
                        failure_body,
                        env: result_env.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: pattern2,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                    });
                }
            } else {
                // Multiple results - iterate over them
                let mut remaining: VecDeque<_> = pattern1_results.into_iter().collect();
                let first = remaining.pop_front().unwrap();

                continuations.push(GenericContinuation::ProcessUnifyPattern1Iter {
                    remaining_pattern1_results: remaining,
                    pattern2: pattern2.clone(),
                    success_body: success_body.clone(),
                    failure_body: failure_body.clone(),
                    all_results: vec![],
                    env: result_env.clone(),
                    depth,
                });

                // Check if first is a Space
                if let Some(handle) = first.as_space() {
                    // Space unification for first value
                    if handle.is_module_space() || handle.name == "self" {
                        // Module/self spaces use Environment's match_space
                        let matches: Vec<(C::Value, usize)> =
                            result_env.match_space(&pattern2, &pattern2)
                                .into_iter()
                                .map(|m| (m.value, m.count))
                                .collect();

                        // Build bodies for matches - values already generic
                        let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                        let mut found_match = false;
                        for (generic_value, count) in &matches {
                            if let Some(bindings) = pattern_match_generic(&pattern2, generic_value) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..*count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            } else if let Some(bindings) = pattern_match_generic(generic_value, &pattern2) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..*count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            }
                        }

                        // If no pattern matched, include failure body
                        if !found_match {
                            bodies_to_eval.push_back(failure_body.clone());
                        }

                        if let Some(first_body) = bodies_to_eval.pop_front() {
                            continuations.push(GenericContinuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_to_eval,
                                results: vec![],
                                env: result_env.clone(),
                                depth,
                            });
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_body,
                                env: result_env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        } else {
                            // No bodies at all - send empty to iterator
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], result_env),
                            });
                        }
                    } else {
                        // GENERIC: Non-module spaces - use generic collapse_with_multiplicity

                        let matches: Vec<GenericMultiplicityMatch<C::Value>> =
                            handle.collapse_with_multiplicity_generic(ctx.factory());

                        // Build bodies for matches - NO conversion needed
                        let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(bindings) = pattern_match_generic(&pattern2, &m.value) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            } else if let Some(bindings) = pattern_match_generic(&m.value, &pattern2) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            }
                        }

                        // If no pattern matched, include failure body
                        if !found_match {
                            bodies_to_eval.push_back(failure_body.clone());
                        }

                        if let Some(first_body) = bodies_to_eval.pop_front() {
                            continuations.push(GenericContinuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_to_eval,
                                results: vec![],
                                env: result_env.clone(),
                                depth,
                            });
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_body,
                                env: result_env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        } else {
                            // No bodies at all - send empty to iterator
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], result_env),
                            });
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    continuations.push(GenericContinuation::ProcessUnifyPattern2 {
                        val1: first,
                        pattern2: pattern2.clone(),
                        success_body: ctx.factory().atom("__unify_success__"),
                        failure_body: ctx.factory().atom("__unify_failure__"),
                        env: result_env.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: pattern2,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                    });
                }
            }
        }

        GenericContinuation::ProcessUnifyPattern1Iter {
            mut remaining_pattern1_results,
            pattern2,
            success_body,
            failure_body,
            mut all_results,
            env: _iter_env,
            depth,
        } => {
            let (body_results, env_after) = result;

            // Accumulate results from the pattern1 value we just processed
            all_results.extend(body_results);

            // Get next pattern1 value to process
            if let Some(val1) = remaining_pattern1_results.pop_front() {
                // Create new iterator continuation for the REMAINING values
                // (after this one we're about to process)
                continuations.push(GenericContinuation::ProcessUnifyPattern1Iter {
                    remaining_pattern1_results,
                    pattern2: pattern2.clone(),
                    success_body: success_body.clone(),
                    failure_body: failure_body.clone(),
                    all_results,
                    env: env_after.clone(),
                    depth,
                });

                // Process this pattern1 value
                if let Some(handle) = val1.as_space() {
                    let pattern = pattern2.clone();
                    // Boolean check optimization: if success is True and failure is False
                    let is_boolean_check = {
                        let success_true = success_body.as_bool() == Some(true)
                            || success_body.as_atom() == Some("True");
                        let failure_false = failure_body.as_bool() == Some(false)
                            || failure_body.as_atom() == Some("False");
                        success_true && failure_false
                    };

                    if is_boolean_check {
                        // Optimized exists check - no need to collect all matches
                        let exists = if handle.is_module_space() || handle.name == "self" {
                            env_after.match_space_exists(&pattern)
                        } else {
                            let atoms: Vec<C::Value> = handle.collapse_generic(ctx.factory());
                            atoms.iter().any(|atom| {
                                pattern_match_generic(&pattern, atom).is_some()
                                    || pattern_match_generic(atom, &pattern).is_some()
                            })
                        };
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![ctx.factory().bool(exists)], env_after),
                        });
                    } else {
                        // Full match: get matches from appropriate source

                        let matches: Vec<GenericMultiplicityMatch<C::Value>> =
                            if handle.is_module_space() || handle.name == "self" {
                                // Module/self spaces - use match_space
                                env_after
                                    .match_space(&pattern, &pattern)
                                    .into_iter()
                                    .map(|m| GenericMultiplicityMatch { value: m.value, count: m.count })
                                    .collect()
                            } else {
                                // Non-module spaces - use collapse_with_multiplicity_generic
                                handle.collapse_with_multiplicity_generic(ctx.factory())
                            };

                        let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(bindings) = pattern_match_generic(&pattern, &m.value) {
                                found_match = true;
                                let instantiated =
                                    apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(instantiated.clone());
                                }
                            } else if let Some(bindings) = pattern_match_generic(&m.value, &pattern)
                            {
                                found_match = true;
                                let instantiated =
                                    apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(instantiated.clone());
                                }
                            }
                        }
                        if !found_match {
                            bodies_to_eval.push_back(failure_body.clone());
                        }

                        if let Some(first_body) = bodies_to_eval.pop_front() {
                            continuations.push(GenericContinuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_to_eval,
                                results: Vec::new(),
                                env: env_after.clone(),
                                depth,
                            });
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_body,
                                env: env_after,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                            });
                        } else {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], env_after),
                            });
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    continuations.push(GenericContinuation::ProcessUnifyPattern2 {
                        val1,
                        pattern2: pattern2.clone(),
                        success_body,
                        failure_body,
                        env: env_after.clone(),
                        depth,
                    });
                    work_stack.push(GenericWorkItem::Eval {
                        value: pattern2,
                        env: env_after,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                    });
                }
            } else {
                // No more pattern1 values - return all accumulated results
                if all_results.is_empty() {
                    work_stack.push(GenericWorkItem::Eval {
                        value: failure_body,
                        env: env_after,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                    });
                } else {
                    work_stack.push(GenericWorkItem::Resume {
                        result: (all_results, env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessUnifyPattern2 {
            val1,
            pattern2: _,
            success_body,
            failure_body,
            env: _,
            depth,
        } => {
            let (pattern2_results, result_env) = result;

            if pattern2_results.is_empty() {
                work_stack.push(GenericWorkItem::Eval {
                    value: failure_body,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            } else {
                // Try to unify with each pattern2 result - NO conversion needed
                let mut all_bindings = Vec::new();
                for p2_result in &pattern2_results {
                    if let Some(bindings) = pattern_match_generic(&val1, p2_result) {
                        all_bindings.push(bindings);
                    }
                }

                if all_bindings.is_empty() {
                    work_stack.push(GenericWorkItem::Eval {
                        value: failure_body,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                    });
                } else if all_bindings.len() == 1 {
                    // Apply bindings generically - NO conversion needed
                    let instantiated = apply_bindings_generic(&success_body, &all_bindings[0], ctx.factory());

                    work_stack.push(GenericWorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                    });
                } else {
                    // Multiple bindings - pre-instantiate all bodies generically
                    let mut bodies_to_eval: VecDeque<C::Value> = all_bindings.iter()
                        .map(|bindings| {
                            apply_bindings_generic(&success_body, bindings, ctx.factory())
                        })
                        .collect();

                    let first_body = bodies_to_eval.pop_front().unwrap();

                    continuations.push(GenericContinuation::ProcessUnifyBodies {
                        remaining_bodies: bodies_to_eval,
                        results: vec![],
                        env: result_env.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: first_body,
                        env: result_env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                    });
                }
            }
        }

        GenericContinuation::ProcessUnifyBodies {
            mut remaining_bodies,
            mut results,
            env: _,
            depth,
        } => {
            let (body_results, env_after_body) = result;
            results.extend(body_results);

            if let Some(next_body) = remaining_bodies.pop_front() {
                continuations.push(GenericContinuation::ProcessUnifyBodies {
                    remaining_bodies,
                    results,
                    env: env_after_body.clone(),
                    depth,
                });
                work_stack.push(GenericWorkItem::Eval {
                    value: next_body,
                    env: env_after_body,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                });
            } else {
                work_stack.push(GenericWorkItem::Resume {
                    result: (results, env_after_body),
                });
            }
        }

        GenericContinuation::ProcessCollapse {
            env: _,
            depth,
        } => {
            let (expr_results, result_env) = result;

            // Empty results: return empty tuple immediately
            if expr_results.is_empty() {
                let result_list = ctx.factory().sexpr(vec![]);
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![result_list], result_env),
                });
                return;
            }

            // MeTTa HE collapse semantics: evaluate each result to normal form.
            // HE's collapse calls `metta` (the full recursive interpreter) which
            // evaluates every nondeterministic result before assembling the tuple.
            let mut remaining_raw: VecDeque<C::Value> = expr_results.into_iter().collect();
            let first_raw = remaining_raw.pop_front().expect("expr_results is non-empty");

            continuations.push(GenericContinuation::ProcessCollapseEvalResults {
                remaining_raw,
                evaluated: Vec::new(),
                is_bind: false,
                env: result_env.clone(),
                depth,
            });

            work_stack.push(GenericWorkItem::Eval {
                value: first_raw,
                env: result_env,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
            });
        }

        GenericContinuation::ProcessCollapseBind {
            env: _,
            depth,
        } => {
            let (expr_results, result_env) = result;

            // Empty results: return empty tuple immediately
            if expr_results.is_empty() {
                let result_list = ctx.factory().sexpr(vec![]);
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![result_list], result_env),
                });
                return;
            }

            // MeTTa HE collapse-bind semantics: evaluate each result to normal form.
            let mut remaining_raw: VecDeque<C::Value> = expr_results.into_iter().collect();
            let first_raw = remaining_raw.pop_front().expect("expr_results is non-empty");

            continuations.push(GenericContinuation::ProcessCollapseEvalResults {
                remaining_raw,
                evaluated: Vec::new(),
                is_bind: true,
                env: result_env.clone(),
                depth,
            });

            work_stack.push(GenericWorkItem::Eval {
                value: first_raw,
                env: result_env,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
            });
        }

        GenericContinuation::ProcessCollapseEvalResults {
            mut remaining_raw,
            mut evaluated,
            is_bind: _,
            env: _,
            depth,
        } => {
            let (eval_results, result_env) = result;

            // Collect evaluated results (filter empty/pruned branches)
            evaluated.extend(eval_results.into_iter().filter(|v| !v.is_empty()));

            if let Some(next_raw) = remaining_raw.pop_front() {
                // More results to evaluate — reuse continuation slot
                continuations.push(GenericContinuation::ProcessCollapseEvalResults {
                    remaining_raw,
                    evaluated,
                    is_bind: false,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next_raw,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                });
            } else {
                // All results evaluated — assemble the tuple
                let result_list = ctx.factory().sexpr(evaluated);

                // Trace: collapse-result phase
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&result_list),
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: "collapse".to_string(),
                                phase: "collapse-result".to_string(),
                            },
                        );
                    }
                }

                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![result_list], result_env),
                });
            }
        }

        GenericContinuation::ProcessAmb {
            mut remaining_alts,
            mut results,
            env: _,
            depth,
        } => {
            let (alt_results, result_env) = result;
            results.extend(alt_results);

            if let Some(next_alt) = remaining_alts.pop_front() {
                continuations.push(GenericContinuation::ProcessAmb {
                    remaining_alts,
                    results,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next_alt,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                });
            } else {
                work_stack.push(GenericWorkItem::Resume {
                    result: (results, result_env),
                });
            }
        }

        GenericContinuation::ProcessGuard {
            env: _,
            depth: _,
        } => {
            let (cond_results, result_env) = result;

            match cond_results.first() {
                Some(v) if v.as_bool() == Some(true) => {
                    // Guard passes - return Unit
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![ctx.factory().unit()], result_env),
                    });
                }
                Some(v) if v.as_bool() == Some(false) => {
                    // Guard fails - return empty (nondeterministic failure)
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![], result_env),
                    });
                }
                Some(v) if v.is_error() => {
                    // Error propagates
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![v.clone()], result_env),
                    });
                }
                Some(v) => {
                    // Type error - condition must be Bool
                    let err = ctx.factory().error(
                        &format!(
                            "guard: condition must evaluate to Bool, got {}",
                            v.friendly_repr()
                        ),
                        v.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], result_env),
                    });
                }
                None => {
                    // Empty results - guard fails
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![], result_env),
                    });
                }
            }
        }

        GenericContinuation::ProcessGetAtoms {
            space_ref,
            env: _,
            depth: _,
        } => {
            let (space_results, result_env) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "get-atoms: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], result_env),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    // GENERIC: Use collapse_generic to avoid heap conversion
                    let atoms: Vec<C::Value> = handle.collapse_generic(ctx.factory());
                    if atoms.is_empty() {
                        // Empty space returns empty results
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![], result_env),
                        });
                    } else {
                        // Return all atoms as separate results (superposition)
                        work_stack.push(GenericWorkItem::Resume {
                            result: (atoms, result_env),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!("get-atoms: first argument must be a space, got {}", first.friendly_repr()),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], result_env),
                    });
                }
            }
        }

        GenericContinuation::ProcessMatchSpace {
            space_arg,
            pattern,
            template,
            env,
            depth,
        } => {
            let (space_results, env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "match: space evaluated to empty",
                    space_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    if handle.is_module_space() || handle.name == "self" {
                        // Phase 8.4: Type-aware match optimization.
                        // If pattern is (: $var TypeName), use the types HashMap as a
                        // reverse index instead of scanning the entire MORK space.
                        let type_filtered = if let Some(pat_items) = pattern.as_sexpr() {
                            if pat_items.len() == 3 {
                                if let (Some(":"), Some(var), Some(type_name)) = (
                                    pat_items[0].as_atom(),
                                    pat_items[1].as_atom(),
                                    pat_items[2].as_atom(),
                                ) {
                                    if var.starts_with('$') {
                                        // Use type index: O(k) where k = atoms of matching type
                                        let matching_atoms = env.get_atoms_of_type(type_name);
                                        let results: Vec<C::Value> = matching_atoms.iter()
                                            .map(|name| {
                                                let mut bindings = crate::backend::models::GenericBindings::new();
                                                bindings.insert(var.to_string(), ctx.factory().atom(name));
                                                apply_bindings_generic(&template, &bindings, ctx.factory())
                                            })
                                            .collect();
                                        Some(results)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let generic_results: Vec<C::Value> = if let Some(filtered) = type_filtered {
                            filtered
                        } else {
                            // Standard path: match_space which handles serialization internally
                            let matches = env.match_space(&pattern, &template);
                            // Expand multiplicities into flat list
                            matches
                                .into_iter()
                                .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                                .collect()
                        };

                        // Trace: space-result phase (&self / module space)
                        #[cfg(feature = "eval-trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&pattern),
                                    generic_results.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "match".to_string(),
                                        phase: "space-result".to_string(),
                                    },
                                );
                            }
                        }

                        work_stack.push(GenericWorkItem::Resume {
                            result: (generic_results, env_after),
                        });
                    } else {
                        // Owned space - match against atoms in SpaceHandle via unified match_pattern_generic
                        let instantiated_templates: Vec<C::Value> =
                            handle.match_pattern_generic(&pattern, &template, ctx.factory());

                        // Trace: space-result phase (owned space)
                        #[cfg(feature = "eval-trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&pattern),
                                    instantiated_templates.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "match".to_string(),
                                        phase: "space-result".to_string(),
                                    },
                                );
                            }
                        }

                        if instantiated_templates.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                result: (vec![], env_after),
                            });
                        } else if instantiated_templates.len() == 1 {
                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated_templates.into_iter().next().unwrap(),
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        } else {
                            // Multiple matches - queue template evaluations
                            let mut generic_templates: VecDeque<C::Value> = instantiated_templates
                                .into_iter()
                                .collect();
                            let first_template = generic_templates.pop_front().unwrap();

                            continuations.push(GenericContinuation::ProcessMatchTemplates {
                                remaining_templates: generic_templates,
                                results: vec![],
                                env: env_after.clone(),
                                depth,
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_template,
                                env: forked_env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        }
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "match: first argument must be a space, got {}. Usage: (match space pattern template)",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessMatchTemplates {
            mut remaining_templates,
            mut results,
            env,
            depth,
        } => {
            let (template_results, _env_after) = result;
            results.extend(template_results);

            if remaining_templates.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    result: (results, env),
                });
            } else {
                let next_template = remaining_templates.pop_front().unwrap();

                continuations.push(GenericContinuation::ProcessMatchTemplates {
                    remaining_templates,
                    results,
                    env: env.clone(),
                    depth,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next_template,
                    env: env.fork_for_nondeterminism(),
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            }
        }

        GenericContinuation::ProcessAddAtomSpace {
            space_ref,
            atom,
            env: _,
            depth: _,
        } => {
            let (space_results, mut env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "add-atom: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    // MeTTa HE semantics: add the UNEVALUATED atom to the space.
                    // The atom is NOT evaluated — per HE docs: "Adds atom into the
                    // atomspace without reducing it".
                    let is_self_space = handle.is_module_space() || handle.name == "self";

                    if is_self_space {
                        // &self space: add directly to environment's PathMap/RuleIndex.
                        // match &self and get-atoms query env.match_space() / env.get_all_atoms(),
                        // NOT the SpaceHandle, so atoms must live in the environment.
                        // add_to_space() handles routing: rules → add_rule() (PathMap + RuleIndex),
                        // type assertions → types HashMap, all atoms → PathMap.
                        env_after.add_to_space(&atom);
                    } else {
                        // Named space: add to SpaceHandle (match queries SpaceHandle
                        // for non-&self spaces via handle.collapse_generic()).
                        handle.add_atom_generic(&atom);
                    }

                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![ctx.factory().unit()], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "add-atom: first argument must be a space reference, got {}. Usage: (add-atom space atom)",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        // Disabled: ProcessAddAtomAtom is no longer constructed. The atom evaluation
        // step has been eliminated — add-atom now takes unevaluated atoms per MeTTa HE
        // semantics. See ProcessAddAtomSpace above.
        // GenericContinuation::ProcessAddAtomAtom {
        //     space_handle,
        //     atom,
        //     env: _,
        //     depth: _,
        //     parent_cont,
        // } => {
        //     let (atom_results, env_after) = result;
        //
        //     if atom_results.is_empty() {
        //         let err = ctx.factory().error(
        //             "add-atom: atom evaluated to empty",
        //             atom,
        //         );
        //         work_stack.push(GenericWorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (vec![err], env_after),
        //         });
        //     } else {
        //         // GENERIC: Use add_atom_generic to avoid heap conversion
        //         space_handle.add_atom_generic(&atom_results[0]);
        //         work_stack.push(GenericWorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (vec![ctx.factory().unit()], env_after),
        //         });
        //     }
        // }

        GenericContinuation::ProcessRemoveAtomSpace {
            space_ref,
            atom,
            env: _,
            depth: _,
        } => {
            let (space_results, mut env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "remove-atom: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    // MeTTa HE semantics: remove the UNEVALUATED atom from the space.
                    // The atom is NOT evaluated — mirrors add-atom behavior.
                    let is_self_space = handle.is_module_space() || handle.name == "self";

                    if is_self_space {
                        // &self space: remove from environment's PathMap/RuleIndex.
                        // Mirrors the add-atom routing: match &self queries the
                        // environment, so removals must target the environment.
                        // remove_from_space() handles routing: rules → De Bruijn removal
                        // + RuleIndex sync, type assertions → types HashMap, all atoms → PathMap.
                        env_after.remove_from_space(&atom);
                    } else {
                        // Named space: remove from SpaceHandle
                        handle.remove_atom_generic(&atom);
                    }

                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![ctx.factory().unit()], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "remove-atom: first argument must be a space reference, got {}. Usage: (remove-atom space atom)",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        // Disabled: ProcessRemoveAtomAtom is no longer constructed. The atom evaluation
        // step has been eliminated — remove-atom now takes unevaluated atoms per MeTTa HE
        // semantics. See ProcessRemoveAtomSpace above.
        // GenericContinuation::ProcessRemoveAtomAtom {
        //     space_handle,
        //     atom,
        //     env: _,
        //     depth: _,
        //     parent_cont,
        // } => {
        //     let (atom_results, env_after) = result;
        //
        //     if atom_results.is_empty() {
        //         let err = ctx.factory().error(
        //             "remove-atom: atom evaluated to empty",
        //             atom,
        //         );
        //         work_stack.push(GenericWorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (vec![err], env_after),
        //         });
        //     } else {
        //         // Remove the atom from the space
        //         // NOTE: Arena engine returns Unit() regardless of whether removal succeeded
        //         // GENERIC: Use remove_atom_generic to avoid heap conversion
        //         space_handle.remove_atom_generic(&atom_results[0]);
        //         work_stack.push(GenericWorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (vec![ctx.factory().unit()], env_after),
        //         });
        //     }
        // }

        GenericContinuation::ProcessNewState {
            initial_value,
            env: _,
            depth: _,
        } => {
            let (init_results, mut env_after) = result;

            if init_results.is_empty() {
                let err = ctx.factory().error(
                    "new-state: initial value evaluated to empty",
                    initial_value,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                // Use create_state directly - values are already V
                let state_id = env_after.create_state(&init_results[0]);
                let state_value = ctx.factory().state(state_id);
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![state_value], env_after),
                });
            }
        }

        GenericContinuation::ProcessGetState {
            state_ref,
            env: _,
            depth: _,
        } => {
            let (state_results, env_after) = result;

            if state_results.is_empty() {
                let err = ctx.factory().error(
                    "get-state: state reference evaluated to empty",
                    state_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &state_results[0];
                if let Some(state_id) = first.as_state() {
                    // Use get_state directly - returns V
                    if let Some(generic_value) = env_after.get_state(state_id) {
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![generic_value], env_after),
                        });
                    } else {
                        let err = ctx.factory().error(
                            &format!("get-state: state {} not found", state_id),
                            first.clone(),
                        );
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![err], env_after),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "get-state: argument must be a state reference, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessChangeStateRef {
            state_ref,
            new_value,
            env: _,
            depth,
        } => {
            let (state_results, env_after) = result;

            if state_results.is_empty() {
                let err = ctx.factory().error(
                    "change-state!: state reference evaluated to empty",
                    state_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &state_results[0];
                if first.as_state().is_some() {
                    continuations.push(GenericContinuation::ProcessChangeStateValue {
                        state_value: first.clone(),
                        new_value: new_value.clone(),
                        env: env_after.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: new_value,
                        env: env_after,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "change-state!: first argument must be a state reference, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessChangeStateValue {
            state_value,
            new_value,
            env: _,
            depth: _,
        } => {
            let (value_results, mut env_after) = result;

            if value_results.is_empty() {
                let err = ctx.factory().error(
                    "change-state!: new value evaluated to empty",
                    new_value,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                // Get the state ID from state_value
                if let Some(state_id) = state_value.as_state() {
                    // Use change_state directly - values are already V
                    env_after.change_state(state_id, &value_results[0]);
                    let result_state = ctx.factory().state(state_id);
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![result_state], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        "change-state!: expected state value",
                        state_value,
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessRepr {
            atom: _,
            env: _,
            depth: _,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![ctx.factory().string("")], env_after),
                });
            } else {
                let repr = atom_results[0].friendly_repr();
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![ctx.factory().string(&repr)], env_after),
                });
            }
        }

        GenericContinuation::ProcessFormatArgsString {
            format_arg,
            args_arg,
            env: _,
            depth,
        } => {
            let (format_results, env_after) = result;

            if format_results.is_empty() {
                let err = ctx.factory().error(
                    "format-args: format string evaluated to empty",
                    format_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &format_results[0];
                if let Some(format_str) = first.as_string() {
                    continuations.push(GenericContinuation::ProcessFormatArgsArgs {
                        format_str: format_str.to_string(),
                        args_arg: args_arg.clone(),
                        env: env_after.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: args_arg,
                        env: env_after,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "format-args: first argument must be a string, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessFormatArgsArgs {
            format_str,
            args_arg,
            env: _,
            depth: _,
        } => {
            let (args_results, env_after) = result;

            if args_results.is_empty() {
                let err = ctx.factory().error(
                    "format-args: args evaluated to empty",
                    args_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                // Get args as a list - use native generic values directly
                let args_list: Vec<&C::Value> = if let Some(items) = args_results[0].as_sexpr() {
                    items.iter().collect()
                } else {
                    args_results.iter().collect()
                };

                // Simple format string substitution using friendly_repr
                let mut result_str = format_str.clone();
                for (i, arg) in args_list.iter().enumerate() {
                    let placeholder = format!("{{{}}}", i);
                    let repr = arg.friendly_repr();
                    result_str = result_str.replace(&placeholder, &repr);
                }

                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![ctx.factory().string(&result_str)], env_after),
                });
            }
        }

        GenericContinuation::ProcessPrintln {
            atom: _,
            env: _,
            depth: _,
        } => {
            let (atom_results, env_after) = result;

            for atom_result in &atom_results {
                // Use to_display_string() - prints strings without quotes
                println!("{}", atom_result.to_display_string());
            }

            work_stack.push(GenericWorkItem::Resume {
                result: (vec![ctx.factory().unit()], env_after),
            });
        }

        GenericContinuation::ProcessTraceMessage {
            message: _,
            value_expr,
            env: _,
            depth,
        } => {
            let (msg_results, env_after) = result;

            // HE semantics: print message on its own line, no prefix
            if let Some(first) = msg_results.first() {
                eprintln!("{}", first.friendly_repr());
            }

            // Now evaluate the value
            continuations.push(GenericContinuation::ProcessTraceValue {
                value_expr: value_expr.clone(),
                env: env_after.clone(),
                depth,
            });

            work_stack.push(GenericWorkItem::Eval {
                value: value_expr,
                env: env_after,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
            });
        }

        GenericContinuation::ProcessTraceValue {
            value_expr: _,
            env: _,
            depth: _,
        } => {
            let (value_results, env_after) = result;

            // HE semantics: return evaluated value(s) as-is.
            // If empty, propagate empty (valid nondeterministic dead-end).
            // HE trace! does not print the value — only the message.
            work_stack.push(GenericWorkItem::Resume {
                result: (value_results, env_after),
            });
        }

        GenericContinuation::ProcessGetMetatype {
            atom: _,
            env: _,
            depth: _,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![ctx.factory().atom("Undefined")], env_after),
                });
            } else {
                let first = &atom_results[0];
                let metatype = match first.inner_raw() {
                    MettaValueInner::Quoted(_) | MettaValueInner::SExpr(_) => "Expression",
                    MettaValueInner::Atom(s) if is_variable_str(s) => "Variable",
                    MettaValueInner::Atom(_) => "Symbol",
                    MettaValueInner::Bool(_) | MettaValueInner::Long(_)
                    | MettaValueInner::Float(_) | MettaValueInner::String(_) => "Grounded",
                    MettaValueInner::Error(..) => "Error",
                    MettaValueInner::Spanned(..) => {
                        let stripped = first.strip_one_span();
                        // Re-dispatch on the stripped value
                        match stripped.inner_raw() {
                            MettaValueInner::Quoted(_) | MettaValueInner::SExpr(_) => "Expression",
                            MettaValueInner::Atom(s) if is_variable_str(s) => "Variable",
                            MettaValueInner::Atom(_) => "Symbol",
                            MettaValueInner::Bool(_) | MettaValueInner::Long(_)
                            | MettaValueInner::Float(_) | MettaValueInner::String(_) => "Grounded",
                            MettaValueInner::Error(..) => "Error",
                            _ => "Undefined",
                        }
                    }
                    _ => "Undefined",
                };

                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![ctx.factory().atom(metatype)], env_after),
                });
            }
        }

        GenericContinuation::ProcessBind {
            token,
            env: _,
            depth: _,
        } => {
            let (atom_results, mut env_after) = result;

            if atom_results.is_empty() {
                let err = ctx.factory().error(
                    "bind!: atom evaluated to empty",
                    ctx.factory().atom(&token),
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                env_after.register_token(&token, atom_results[0].clone());
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![ctx.factory().unit()], env_after),
                });
            }
        }

        // if-reducible: expr has been evaluated, compare to original
        GenericContinuation::ProcessIfReducible {
            original_expr,
            then_branch,
            else_branch,
            env: _,
            depth,
        } => {
            let (eval_results, env_after) = result;

            // Determine if the expression reduced:
            // - Empty results → irreducible (nothing produced)
            // - Single result equal to original → irreducible
            // - Otherwise → reduced (result changed or multiple results)
            let is_irreducible = if eval_results.is_empty() {
                true
            } else if eval_results.len() == 1 {
                eval_results[0] == original_expr
            } else {
                // Multiple results means the expression nondeterministically reduced
                false
            };

            // Trace: reduced / irreducible phase
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let phase = if is_irreducible { "irreducible" } else { "reduced" };
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&original_expr),
                        eval_results.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: "if-reducible".to_string(),
                            phase: phase.to_string(),
                        },
                    );
                }
            }

            if is_irreducible {
                // Expression didn't change — evaluate else branch
                work_stack.push(GenericWorkItem::Eval {
                    value: else_branch,
                    env: env_after,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            } else {
                // Expression reduced — evaluate then branch
                work_stack.push(GenericWorkItem::Eval {
                    value: then_branch,
                    env: env_after,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            }
        }

        // match-or: space has been evaluated, now perform match with default fallback
        GenericContinuation::ProcessMatchOrSpace {
            space_arg: _,
            pattern,
            default,
            template,
            env,
            depth,
        } => {
            let (space_results, env_after) = result;

            if space_results.is_empty() {
                // Trace: default-branch phase (space empty)
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&pattern),
                            vec![crate::backend::trace::trace_value_generic(&default)],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: "match-or".to_string(),
                                phase: "default-branch".to_string(),
                            },
                        );
                    }
                }

                // Space evaluated to empty — use default
                work_stack.push(GenericWorkItem::Eval {
                    value: default,
                    env: env_after,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    if handle.is_module_space() || handle.name == "self" {
                        // &self or module space — use env.match_space
                        let matches = env.match_space(&pattern, &template);
                        let generic_results: Vec<C::Value> = matches
                            .into_iter()
                            .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                            .collect();

                        if generic_results.is_empty() {
                            // Trace: default-branch phase (&self no match)
                            #[cfg(feature = "eval-trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&pattern),
                                        vec![crate::backend::trace::trace_value_generic(&default)],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "match-or".to_string(),
                                            phase: "default-branch".to_string(),
                                        },
                                    );
                                }
                            }

                            // No matches — evaluate default
                            work_stack.push(GenericWorkItem::Eval {
                                value: default,
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        } else if generic_results.len() == 1 {
                            // Single match — evaluate template result
                            work_stack.push(GenericWorkItem::Eval {
                                value: generic_results.into_iter().next().expect("non-empty"),
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        } else {
                            // Multiple matches — queue template evaluations
                            let mut templates: VecDeque<C::Value> = generic_results.into_iter().collect();
                            let first_template = templates.pop_front().expect("non-empty");

                            continuations.push(GenericContinuation::ProcessMatchTemplates {
                                remaining_templates: templates,
                                results: vec![],
                                env: env_after.clone(),
                                depth,
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_template,
                                env: forked_env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        }
                    } else {
                        // Owned space — match against SpaceHandle
                        let instantiated_templates: Vec<C::Value> =
                            handle.match_pattern_generic(&pattern, &template, ctx.factory());

                        if instantiated_templates.is_empty() {
                            // Trace: default-branch phase (owned space no match)
                            #[cfg(feature = "eval-trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&pattern),
                                        vec![crate::backend::trace::trace_value_generic(&default)],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "match-or".to_string(),
                                            phase: "default-branch".to_string(),
                                        },
                                    );
                                }
                            }

                            // No matches — evaluate default
                            work_stack.push(GenericWorkItem::Eval {
                                value: default,
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        } else if instantiated_templates.len() == 1 {
                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated_templates.into_iter().next().expect("non-empty"),
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        } else {
                            let mut generic_templates: VecDeque<C::Value> =
                                instantiated_templates.into_iter().collect();
                            let first_template = generic_templates.pop_front().expect("non-empty");

                            continuations.push(GenericContinuation::ProcessMatchTemplates {
                                remaining_templates: generic_templates,
                                results: vec![],
                                env: env_after.clone(),
                                depth,
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_template,
                                env: forked_env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                            });
                        }
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "match-or: first argument must be a space, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        // Memo-related continuations - delegate to heap conversion for now
        GenericContinuation::ProcessMemoTable {
            memo_ref,
            expr,
            first_only,
            env: _,
            depth,
        } => {
            let (memo_results, env_after) = result;

            if memo_results.is_empty() {
                let err = ctx.factory().error(
                    "memo/memo!: memo reference evaluated to empty",
                    memo_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &memo_results[0];
                if let Some(memo_handle) = first.as_memo() {
                    // Check if already cached - use generic lookup
                    if let Some(cached) = memo_handle.lookup_generic(&expr, ctx.factory()) {
                        work_stack.push(GenericWorkItem::Resume {
                            result: (cached, env_after),
                        });
                    } else {
                        // Not cached - evaluate and cache result
                        continuations.push(GenericContinuation::ProcessMemoExpr {
                            memo_handle: memo_handle.clone(),
                            expr: expr.clone(),
                            first_only,
                            env: env_after.clone(),
                            depth,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env: env_after,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "memo/memo!: first argument must be a memo table, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessMemoExpr {
            memo_handle,
            expr,
            first_only,
            env: _,
            depth: _,
        } => {
            let (expr_results, env_after) = result;

            // Cache the result using generic store
            if first_only && !expr_results.is_empty() {
                memo_handle.store_generic(&expr, &expr_results[..1]);
            } else {
                memo_handle.store_generic(&expr, &expr_results);
            }

            work_stack.push(GenericWorkItem::Resume {
                result: (expr_results, env_after),
            });
        }

        GenericContinuation::ProcessNewMemoName {
            name_arg,
            size_arg,
            env: _,
            depth,
        } => {
            let (name_results, env_after) = result;

            if name_results.is_empty() {
                let err = ctx.factory().error(
                    "new-memo: name evaluated to empty",
                    name_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &name_results[0];
                let name = if let Some(s) = first.as_string() {
                    s.to_string()
                } else if let Some(a) = first.as_atom() {
                    a.to_string()
                } else {
                    first.friendly_repr()
                };

                if let Some(size_value) = size_arg {
                    continuations.push(GenericContinuation::ProcessNewMemoSize {
                        name,
                        size_arg: size_value.clone(),
                        env: env_after.clone(),
                        depth,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: size_value,
                        env: env_after,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                    });
                } else {
                    // No size argument - create memo with default size (no limit)
                    let memo_handle = crate::backend::models::MemoHandle::new(name);
                    let memo_value = ctx.factory().memo(memo_handle);
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![memo_value], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessNewMemoSize {
            name,
            size_arg,
            env: _,
            depth: _,
        } => {
            let (size_results, env_after) = result;

            if size_results.is_empty() {
                let err = ctx.factory().error(
                    "new-memo: size evaluated to empty",
                    size_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let size = size_results[0].as_long().unwrap_or(1000) as usize;
                let memo_handle = crate::backend::models::MemoHandle::with_max_size(name, size);
                let memo_value = ctx.factory().memo(memo_handle);
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![memo_value], env_after),
                });
            }
        }

        GenericContinuation::ProcessMemoOp {
            memo_ref,
            is_clear,
            env: _,
            depth: _,
        } => {
            let (memo_results, env_after) = result;

            if memo_results.is_empty() {
                let op_name = if is_clear { "clear-memo!" } else { "memo-stats" };
                let err = ctx.factory().error(
                    &format!("{}: memo reference evaluated to empty", op_name),
                    memo_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    result: (vec![err], env_after),
                });
            } else {
                let first = &memo_results[0];
                if let Some(memo_handle) = first.as_memo() {
                    if is_clear {
                        memo_handle.clear();
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![ctx.factory().unit()], env_after),
                        });
                    } else {
                        let stats = memo_handle.stats();
                        let stats_sexpr = ctx.factory().sexpr(vec![
                            ctx.factory().atom("hits"),
                            ctx.factory().long(stats.0 as i64),
                            ctx.factory().atom("misses"),
                            ctx.factory().long(stats.1 as i64),
                            ctx.factory().atom("size"),
                            ctx.factory().long(stats.2 as i64),
                        ]);
                        work_stack.push(GenericWorkItem::Resume {
                            result: (vec![stats_sexpr], env_after),
                        });
                    }
                } else {
                    let op_name = if is_clear { "clear-memo!" } else { "memo-stats" };
                    let err = ctx.factory().error(
                        &format!(
                            "{}: argument must be a memo table, got {}",
                            op_name,
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        result: (vec![err], env_after),
                    });
                }
            }
        }
    }
}
