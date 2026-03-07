// Eval module: Arena-based lazy evaluation with pattern matching and built-in dispatch
//
// The evaluation pipeline uses arena-allocated MettaValue exclusively.
// The eval() entry point handles bytecode/JIT tiering with
// tree-walker fallback via eval_trampoline().

pub(crate) mod bindings_generic;
pub(crate) mod frame_chain;
pub(crate) mod freshening;
mod helpers;
pub(crate) mod space_match;
mod list_ops;
pub(crate) mod alpha_equiv;
pub(crate) mod set_ops;
pub(crate) mod testing_ops;
pub(crate) mod modules_generic;
pub(crate) mod mork_forms_generic;
mod pattern;
pub mod priority;
mod processing;
pub(crate) mod step;
pub mod trampoline;
pub(crate) mod type_fixpoint;
pub(crate) mod types_generic;

#[cfg(test)]
mod arena_tests;

// Re-export from pattern module
pub use pattern::pattern_match;

// Re-export from helpers module
pub use helpers::apply_bindings;
// Re-export helpers for step/grounded module access
pub(crate) use helpers::{is_eager_special_form, is_grounded_op};

// Re-export from trampoline module
pub use trampoline::eval_trampoline;
// Re-export arena context types for external use
pub use trampoline::{MettaEnvironment, StaticEvalContext};

// Type system re-exports for benchmarking
pub use types_generic::{
    infer_types_generic, infer_type_generic,
    types_match_generic, types_match_with_subtypes,
    match_types_with_bindings, apply_type_bindings,
    freshen_type_variables, is_meta_type,
    eval_get_type_generic, eval_check_type_generic,
    eval_type_cast_generic, eval_validate_atom_generic,
    eval_get_type_space_generic,
    infer_arrow_type_from_rule,
    extract_type_constraint, get_ground_type, is_pattern_type_compatible,
};
pub use type_fixpoint::run_type_fixpoint;
pub use step::{
    find_typed_arg_indices_generic, find_grounded_arg_indices_generic,
    is_declared_value_type, validate_grounded_arg_types,
    extract_arg_types, extract_return_type, is_arrow_type,
    is_meta_type_value,
};

// =============================================================================
// Arena-based Evaluation with Bytecode/JIT Tiering
// =============================================================================

use crate::backend::models::MettaValue;

/// Type alias for arena evaluation result.
pub type EvalResult = (Vec<MettaValue>, MettaEnvironment);

/// Evaluate an MettaValue with bytecode/JIT tiering.
///
/// This function provides zero-conversion evaluation for MettaValue expressions
/// using session-scoped dual-arena allocation. It uses the arena tiered cache
/// to track executions and trigger background bytecode compilation, then
/// executes via the highest available tier:
///
/// 1. JIT Stage 2 (native code, 500+ executions)
/// 2. JIT Stage 1 (native code, 100+ executions)
/// 3. Bytecode VM (2+ executions)
/// 4. Tree-walker interpreter (fallback)
///
/// The `EvalGuard` is explicitly scoped so it is dropped before attempting
/// GC lifecycle work. This ensures library and test code that calls `eval()`
/// directly (without the `main.rs` between-expression loop) still reaches
/// quiescent points where GC can trigger and backpressure can be released.
///
/// # Arguments
/// * `value` - The MettaValue expression to evaluate
/// * `env` - The arena-based environment
/// * `state` - The MettaState owning the storage arena
///
/// # Returns
/// A tuple of (results, updated_environment) where results are `Vec<MettaValue>`.
///
/// # Example
/// ```ignore
/// use mettatron::backend::compile::compile;
/// use mettatron::backend::eval::eval;
/// use mettatron::backend::eval::trampoline::new_env;
///
/// let state = compile("!(+ 1 2)").unwrap();
/// let env = new_env();
///
/// for &expr in state.source() {
///     let (results, env) = eval(expr, env, &state);
///     println!("{:?}", results);
/// }
/// ```
pub fn eval(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
) -> EvalResult {
    use crate::backend::models::EvalGuard;

    // Scope the EvalGuard so it drops after eval completes.
    let result = {
        let _guard = EvalGuard::enter();
        eval_inner(value, env, state)
    };
    // _guard dropped here — ACTIVE_EVALUATORS decremented.

    // Phase 10.5: Run type fixpoint if rules were added during this eval.
    // O(1) atomic check; no-op when no new types were registered.
    // Must be OUTSIDE EvalGuard scope: run_type_fixpoint() acquires rule_index.read(),
    // and add_rule() (which completed during eval) holds rule_index.write().
    // Calling after eval() returns ensures all locks are released (no deadlock).
    result.1.maybe_run_type_fixpoint();

    // Session-based GC: reclamation is triggered by SessionGuard::drop() between
    // top-level expressions. No post-eval GC lifecycle needed here — the caller
    // (main.rs, rholang_integration.rs) manages SessionGuard around eval+format.

    result
}

/// Evaluate an MettaValue with bytecode/JIT tiering and trace collection.
///
/// Identical to [`eval`] but threads a `TraceCollector` through the evaluation
/// pipeline so that all tree-walker events are recorded. The bytecode/JIT tiers
/// also fall back to the trace-enabled tree-walker on bailout.
///
/// Only available when the `eval-trace` feature is enabled.
#[cfg(feature = "eval-trace")]
pub fn eval_with_trace(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
    collector: &std::sync::Arc<crate::backend::trace::TraceCollector>,
) -> EvalResult {
    use crate::backend::models::EvalGuard;

    // Wire the trace collector into the work pool so worker threads can emit
    // trace events (WorkPoolTaskEnqueued, WorkPoolScaleEvent, etc.).
    // OnceLock inside — only the first call has effect; subsequent are no-ops.
    crate::backend::models::work_pool::set_work_pool_trace_collector(collector);

    let result = {
        let _guard = EvalGuard::enter();
        eval_inner_with_trace(value, env, state, collector)
    };

    result.1.maybe_run_type_fixpoint();
    result
}

/// Inner eval body with trace — bytecode/JIT tiered execution with trace-enabled
/// tree-walker fallback.
///
/// Sets the thread-local trace collector before each bytecode/JIT dispatch so that
/// opcode handlers and JIT runtime helpers can emit trace events via
/// `with_thread_trace_collector()`. Emits `TierDispatch` events at each tier
/// selection point for tier-transition visibility.
#[cfg(feature = "eval-trace")]
fn eval_inner_with_trace(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
    collector: &std::sync::Arc<crate::backend::trace::TraceCollector>,
) -> EvalResult {
    use crate::backend::bytecode::{
        can_compile, can_compile_with_env, eval_bytecode_arena_with_env,
        execute_arena, global_tiered_cache,
        TierStatusKind,
    };
    #[cfg(feature = "track-stats")]
    use crate::backend::bytecode::ExecutionTier;
    use crate::backend::trace::thread_local_sink::{
        set_thread_trace_collector, clear_thread_trace_collector,
    };

    let compilation_state = global_tiered_cache().record_execution(&value);
    let execution_count = compilation_state.execution_count.load(std::sync::atomic::Ordering::Relaxed);
    let expr_hash = compilation_state.expr_hash;

    // Set thread-local trace collector for bytecode/JIT instrumentation.
    set_thread_trace_collector(collector);

    if can_compile(&value) {
        // JIT Stage 2
        if compilation_state.jit2_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit2_code() {
                // Emit TierDispatch event
                collector.emit_converted(
                    trace_format::TraceTier::JitStage2, 0,
                    crate::backend::trace::trace_value_generic(&value),
                    vec![], None,
                    trace_format::TraceEventKind::TierDispatch {
                        expression_hash: expr_hash,
                        selected_tier: trace_format::TraceTier::JitStage2,
                        execution_count,
                    },
                );

                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache()
                            .record_tier_execution(ExecutionTier::JitStage2);
                        clear_thread_trace_collector();
                        return (results, new_env);
                    }
                    Err(_) => {}
                }
            }
        }

        // JIT Stage 1
        if compilation_state.jit1_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit1_code() {
                collector.emit_converted(
                    trace_format::TraceTier::JitStage1, 0,
                    crate::backend::trace::trace_value_generic(&value),
                    vec![], None,
                    trace_format::TraceEventKind::TierDispatch {
                        expression_hash: expr_hash,
                        selected_tier: trace_format::TraceTier::JitStage1,
                        execution_count,
                    },
                );

                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache()
                            .record_tier_execution(ExecutionTier::JitStage1);
                        clear_thread_trace_collector();
                        return (results, new_env);
                    }
                    Err(_) => {}
                }
            }
        }

        // Bytecode VM
        if compilation_state.bytecode_status() == TierStatusKind::Ready {
            if let Some(chunk) = compilation_state.bytecode_chunk() {
                collector.emit_converted(
                    trace_format::TraceTier::BytecodeVM, 0,
                    crate::backend::trace::trace_value_generic(&value),
                    vec![], None,
                    trace_format::TraceEventKind::TierDispatch {
                        expression_hash: expr_hash,
                        selected_tier: trace_format::TraceTier::BytecodeVM,
                        execution_count,
                    },
                );

                match execute_arena(chunk, env.clone()) {
                    Ok((results, new_env, unreduced)) => {
                        if unreduced {
                            // Bytecode couldn't reduce — fall through
                        } else {
                            #[cfg(feature = "track-stats")]
                            global_tiered_cache()
                                .record_tier_execution(ExecutionTier::Bytecode);
                            clear_thread_trace_collector();
                            return (results, new_env);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }

    // Environment-aware bytecode
    if can_compile_with_env(&value) {
        collector.emit_converted(
            trace_format::TraceTier::BytecodeVM, 0,
            crate::backend::trace::trace_value_generic(&value),
            vec![], None,
            trace_format::TraceEventKind::TierDispatch {
                expression_hash: expr_hash,
                selected_tier: trace_format::TraceTier::BytecodeVM,
                execution_count,
            },
        );

        match eval_bytecode_arena_with_env(&value, env.clone()) {
            Ok((results, new_env, unreduced)) => {
                if unreduced {
                    // Bytecode couldn't reduce — fall through to tree-walker
                } else {
                    #[cfg(feature = "track-stats")]
                    global_tiered_cache()
                        .record_tier_execution(ExecutionTier::Bytecode);
                    clear_thread_trace_collector();
                    return (results, new_env);
                }
            }
            Err(_) => {}
        }
    }

    // Tier 0: Tree-walker with trace collection
    collector.emit_converted(
        trace_format::TraceTier::TreeWalker, 0,
        crate::backend::trace::trace_value_generic(&value),
        vec![], None,
        trace_format::TraceEventKind::TierDispatch {
            expression_hash: expr_hash,
            selected_tier: trace_format::TraceTier::TreeWalker,
            execution_count,
        },
    );

    clear_thread_trace_collector();
    #[cfg(feature = "track-stats")]
    global_tiered_cache().record_tier_execution(ExecutionTier::Interpreter);
    trampoline::eval_trampoline_with_trace(value, env, state, collector)
}

/// Inner eval body — bytecode/JIT tiered execution with tree-walker fallback.
///
/// Called from `eval()` while an `EvalGuard` is held (ACTIVE_EVALUATORS > 0).
/// This function must NOT create its own `EvalGuard` — the caller manages the
/// guard's lifetime to ensure proper quiescent-state transitions.
fn eval_inner(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
) -> EvalResult {
    use crate::backend::bytecode::{
        can_compile, can_compile_with_env, eval_bytecode_arena_with_env,
        execute_arena, global_tiered_cache,
        TierStatusKind,
    };
    #[cfg(feature = "track-stats")]
    use crate::backend::bytecode::ExecutionTier;

    // Record execution in arena tiered cache
    // This triggers background bytecode and JIT compilation at thresholds
    let compilation_state = global_tiered_cache().record_execution(&value);

    // Check if this expression can be compiled to bytecode (pure expressions)
    if can_compile(&value) {
        // Check for JIT execution first (highest tier)
        // JIT Stage 2 (very hot code, 500+ executions)
        if compilation_state.jit2_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit2_code() {
                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache()
                            .record_tier_execution(ExecutionTier::JitStage2);
                        return (results, new_env);
                    }
                    Err(_) => {
                        // JIT execution failed, fall through
                    }
                }
            }
        }

        // JIT Stage 1 (hot code, 100+ executions)
        if compilation_state.jit1_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit1_code() {
                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache()
                            .record_tier_execution(ExecutionTier::JitStage1);
                        return (results, new_env);
                    }
                    Err(_) => {
                        // JIT execution failed, fall through
                    }
                }
            }
        }

        // Bytecode VM (warm code, 2+ executions)
        if compilation_state.bytecode_status() == TierStatusKind::Ready {
            if let Some(chunk) = compilation_state.bytecode_chunk() {
                // Execute via generic bytecode VM (zero-conversion)
                match execute_arena(chunk, env.clone()) {
                    Ok((results, new_env, unreduced)) => {
                        if unreduced {
                            // Bytecode couldn't reduce — fall through
                        } else {
                            #[cfg(feature = "track-stats")]
                            global_tiered_cache()
                                .record_tier_execution(ExecutionTier::Bytecode);
                            return (results, new_env);
                        }
                    }
                    Err(_) => {
                        // Bytecode execution failed, fall through to tree-walker
                    }
                }
            }
        }
    }

    // Try environment-aware bytecode for expressions that need rule dispatch.
    if can_compile_with_env(&value) {
        match eval_bytecode_arena_with_env(&value, env.clone()) {
            Ok((results, new_env, unreduced)) => {
                if unreduced {
                    // Bytecode couldn't reduce — fall through to tree-walker
                } else {
                    #[cfg(feature = "track-stats")]
                    global_tiered_cache()
                        .record_tier_execution(ExecutionTier::Bytecode);
                    return (results, new_env);
                }
            }
            Err(_) => {
                // Bytecode compilation/execution failed, fall through to tree-walker
            }
        }
    }

    // Tier 0: Tree-walker interpreter (cold code or fallback)
    #[cfg(feature = "track-stats")]
    global_tiered_cache().record_tier_execution(ExecutionTier::Interpreter);
    eval_trampoline(value, env, state)
}

/// Execute JIT-compiled code for arena expression with environment threading.
fn execute_jit_arena_with_env(
    state: &std::sync::Arc<crate::backend::bytecode::ExprCompilationState>,
    native_ptr: *const (),
    env: MettaEnvironment,
) -> Result<(Vec<MettaValue>, MettaEnvironment), ()> {
    use crate::backend::bytecode::jit::HybridExecutor;
    use crate::backend::models::{global_allocator, global_factory};

    // Get allocator and factory from global singleton
    let allocator = global_allocator();
    let factory = global_factory();

    // Get the bytecode chunk (needed for constants)
    let chunk = state.bytecode_chunk().ok_or(())?;

    // Create hybrid executor and run in arena mode with environment
    let mut executor = HybridExecutor::new();
    executor
        .execute_jit_arena_with_env(&chunk, native_ptr, allocator, &factory, env)
        .map_err(|_| ())
}
