// Eval function: Lazy evaluation with pattern matching and built-in dispatch
//
// eval(a: atom, env) = a, env
// eval((t1 .. tn), env):
//   r1, env_1 = eval(t1, env) | ... | rn, env_n = eval(tn, env)
//   env' = union env_i
//   return fold over rules & grounded functions (emptyset, env')

#[macro_use]
mod macros;

mod bindings;
pub(crate) mod bindings_generic;
mod builtin;
mod cartesian;
mod conjunction;
mod control_flow;
mod errors;
mod evaluation;
mod expression;
pub mod fixed_point;
mod helpers;
mod io;
mod list_ops;
mod modules;
pub(crate) mod modules_generic;
mod mork_forms;
pub(crate) mod mork_forms_generic;
mod pattern;
pub mod priority;
mod processing;
mod quoting;
mod rules;
mod space;
mod step;
mod strings;
pub mod trampoline;
mod types;
pub(crate) mod types_generic;
mod utilities;

#[cfg(test)]
mod eval_tests;

use tracing::debug;

use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{EvalResult, MettaValue};

// Re-export from cartesian module
use cartesian::{cartesian_product_lazy, CartesianProductResult};
// Re-export CartesianProductIter for trampoline module access
pub(crate) use cartesian::CartesianProductIter;

// Re-export from pattern module
pub use pattern::pattern_match;
#[allow(unused_imports)]
use pattern::pattern_match_impl;

// Re-export from io module
#[allow(unused_imports)]
pub(crate) use io::atom_to_string;

// Re-export from helpers module
pub use helpers::apply_bindings;
pub(crate) use helpers::friendly_value_repr;
#[allow(unused_imports)]
use helpers::{
    friendly_type_name, get_head_symbol, is_eager_special_form, is_grounded_op,
    pattern_specificity, preprocess_space_refs, resolve_tokens_shallow,
    resolve_tokens_shallow_generic, suggest_special_form_with_context, try_eval_builtin,
    values_equal, SPECIAL_FORMS,
};

// Re-export from rules module
#[allow(unused_imports)]
use rules::{try_match_all_rules, try_match_all_rules_iterative, try_match_all_rules_query_multi};

// Re-export from trampoline module
pub use trampoline::{eval_trampoline, eval_trampoline_arena, create_arena_context};
// Re-export arena context types for external use
pub use trampoline::{ArenaEnvironment, StaticArenaContext};

// Re-export from step module
#[allow(unused_imports)]
pub(crate) use step::{eval_sexpr_step, eval_step, EvalStep, MemoOpType, ProcessedSExpr};

// Re-export from processing module
#[allow(unused_imports)]
pub(crate) use processing::{
    handle_no_rule_match, process_collected_sexpr, process_single_combination,
};

// Re-export from control_flow module for trampoline access
pub(crate) use control_flow::eval_switch_minimal_trampoline;

/// Evaluate a MettaValue in the given environment
/// Returns (results, new_environment)
/// This is the public entry point that uses iterative evaluation with an explicit work stack
/// to prevent stack overflow for large expressions.
///
/// Implements tiered execution with asynchronous background compilation:
///
/// ```text
/// Tier 0: Tree-Walker Interpreter (cold code, 0-1 executions)
/// Tier 1: Bytecode VM (warm code, 2+ executions)
/// Tier 2: JIT Stage 1 (hot code, 100+ executions)
/// Tier 3: JIT Stage 2 (very hot code, 500+ executions)
/// ```
///
/// Each execution records a count and triggers background compilation at thresholds.
/// The HybridExecutor handles tier dispatch, with graceful fallback to lower tiers.
pub fn eval(value: MettaValue, env: HeapEnvironment) -> EvalResult {
    debug!(metta_val = ?value);

    use crate::backend::bytecode::{
        can_compile_cached, can_compile_with_env, eval_bytecode_hybrid, eval_bytecode_with_env,
        global_tiered_cache, ExecutionTier, TierStatusKind,
    };

    // Track concurrent evaluations for sequential mode detection (hybrid scheduler only)
    #[cfg(feature = "hybrid-p2-priority-scheduler")]
    {
        use crate::backend::bytecode::enter_eval;
        enter_eval();
    }

    // Record execution in unified tiered cache for async background compilation
    // Triggers bytecode compilation at threshold (default 1), JIT Stage 1 at 100, JIT Stage 2 at 500
    let state = global_tiered_cache().record_execution(&value);

    // Check if this expression can be compiled to bytecode
    // Only some expressions are compilable - others need tree-walker semantics
    // (e.g., expressions requiring rule lookup need environment access)
    if can_compile_cached(&value) {
        // Check if bytecode is ready in the unified cache
        // If so, execute it via HybridExecutor (which handles JIT tiering internally)
        if state.bytecode_status() == TierStatusKind::Ready {
            if let Some(chunk) = state.bytecode_chunk() {
                if let Ok(results) = execute_bytecode_chunk(&chunk) {
                    global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);
                    #[cfg(feature = "hybrid-p2-priority-scheduler")]
                    {
                        use crate::backend::bytecode::exit_eval;
                        exit_eval();
                    }
                    return (results, env);
                }
                // Bytecode execution failed, fall through to hybrid path
            }
        }

        // Try existing hybrid evaluation path (also handles bytecode caching and JIT)
        if let Ok(results) = eval_bytecode_hybrid(&value) {
            global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);
            #[cfg(feature = "hybrid-p2-priority-scheduler")]
            {
                use crate::backend::bytecode::exit_eval;
                exit_eval();
            }
            return (results, env);
        }
    }

    // Try environment-aware bytecode for expressions that need rule dispatch
    if can_compile_with_env(&value) {
        if let Ok((results, new_env)) = eval_bytecode_with_env(&value, env.clone()) {
            global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);
            #[cfg(feature = "hybrid-p2-priority-scheduler")]
            {
                use crate::backend::bytecode::exit_eval;
                exit_eval();
            }
            return (results, new_env);
        }
    }

    // Tier 0: Tree-walker interpreter (cold code or fallback)
    global_tiered_cache().record_tier_execution(ExecutionTier::Interpreter);

    // This function takes MettaValue, so always use heap-based evaluation.
    // For zero-conversion arena evaluation, use the arena-specific functions:
    // - compile_arena() → ArenaValue<'static>
    // - eval_trampoline_arena() → ArenaValue<'static>
    //
    // The METTA_USE_ARENA switch should be handled at the main.rs level
    // to choose between the heap and arena pipelines.
    let result = eval_trampoline(value, env);

    #[cfg(feature = "hybrid-p2-priority-scheduler")]
    {
        use crate::backend::bytecode::exit_eval;
        exit_eval();
    }
    result
}

/// Execute a bytecode chunk via HybridExecutor
///
/// The HybridExecutor handles JIT tier dispatch internally, executing via:
/// - JIT native code if hot and compiled
/// - Bytecode VM otherwise
fn execute_bytecode_chunk(
    chunk: &std::sync::Arc<crate::backend::bytecode::BytecodeChunk>,
) -> Result<Vec<MettaValue>, ()> {
    use crate::backend::bytecode::{global_space_registry, HybridExecutor, SpaceRegistry};

    let mut executor = HybridExecutor::new();

    // Connect the global space registry
    let registry_ptr = global_space_registry() as *const SpaceRegistry as *mut ();
    unsafe {
        executor.set_space_registry(registry_ptr);
    }

    executor.run(chunk).map_err(|_| ())
}

// =============================================================================
// Arena-based Evaluation with Bytecode/JIT Tiering
// =============================================================================

use crate::backend::models::ArenaValue;

/// Type alias for arena evaluation result.
pub type ArenaEvalResult = (Vec<ArenaValue<'static>>, ArenaEnvironment);

/// Evaluate an ArenaValue with bytecode/JIT tiering.
///
/// This function provides zero-conversion evaluation for ArenaValue expressions.
/// It uses the arena tiered cache to track executions and trigger background
/// bytecode compilation, then executes via:
/// - Bytecode VM if bytecode is ready
/// - Tree-walker interpreter otherwise
///
/// # Arguments
/// * `value` - The ArenaValue expression to evaluate
/// * `env` - The arena-based environment
///
/// # Returns
/// A tuple of (results, updated_environment) where results are `Vec<ArenaValue<'static>>`.
///
/// # Example
/// ```ignore
/// use mettatron::backend::compile::compile_arena;
/// use mettatron::backend::eval::{eval_arena, trampoline::StaticArenaContext};
///
/// let exprs = compile_arena("!(+ 1 2)").unwrap();
/// let env = StaticArenaContext::new_env();
///
/// for expr in exprs {
///     let (results, env) = eval_arena(expr, env);
///     println!("{:?}", results);
/// }
/// ```
pub fn eval_arena(value: ArenaValue<'static>, env: ArenaEnvironment) -> ArenaEvalResult {
    use crate::backend::bytecode::{
        can_compile_arena, can_compile_arena_with_env, eval_bytecode_arena_with_env,
        execute_arena, global_arena_tiered_cache,
        ExecutionTier, TierStatusKind,
    };

    // Record execution in arena tiered cache
    // This triggers background bytecode and JIT compilation at thresholds
    let state = global_arena_tiered_cache().record_execution(&value);

    // Check if this expression can be compiled to bytecode (pure expressions)
    if can_compile_arena(&value) {
        // Check for JIT execution first (highest tier)
        // JIT Stage 2 (very hot code, 500+ executions)
        // Now properly threads environment through execution.
        if state.jit2_status() == TierStatusKind::Ready {
            if let Some(code) = state.jit2_code() {
                match execute_jit_arena_with_env(&state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        global_arena_tiered_cache()
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
        // Now properly threads environment through execution.
        if state.jit1_status() == TierStatusKind::Ready {
            if let Some(code) = state.jit1_code() {
                match execute_jit_arena_with_env(&state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        global_arena_tiered_cache()
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
        if state.bytecode_status() == TierStatusKind::Ready {
            if let Some(chunk) = state.bytecode_chunk() {
                // Execute via generic bytecode VM (zero-conversion)
                match execute_arena(chunk, env.clone()) {
                    Ok((results, new_env)) => {
                        global_arena_tiered_cache()
                            .record_tier_execution(ExecutionTier::Bytecode);
                        return (results, new_env);
                    }
                    Err(_) => {
                        // Bytecode execution failed, fall through to tree-walker
                    }
                }
            }
        }
    }

    // Try environment-aware bytecode for expressions that need rule dispatch.
    // This mirrors the can_compile_with_env path in eval() for heap values.
    // It handles expressions that aren't pure (can_compile_arena returns false)
    // but can still benefit from bytecode VM execution with environment access.
    if can_compile_arena_with_env(&value) {
        match eval_bytecode_arena_with_env(&value, env.clone()) {
            Ok((results, new_env)) => {
                global_arena_tiered_cache()
                    .record_tier_execution(ExecutionTier::Bytecode);
                return (results, new_env);
            }
            Err(_) => {
                // Bytecode compilation/execution failed, fall through to tree-walker
            }
        }
    }

    // Tier 0: Tree-walker interpreter (cold code or fallback)
    global_arena_tiered_cache().record_tier_execution(ExecutionTier::Interpreter);
    eval_trampoline_arena(value, env)
}

/// Execute JIT-compiled code for arena expression with environment threading.
///
/// This function sets up a HybridExecutor in arena mode and executes the
/// JIT-compiled native code. The environment is properly threaded through
/// execution, allowing JIT runtime functions to access and modify it.
///
/// # Arguments
/// * `state` - The compilation state containing bytecode chunk
/// * `native_ptr` - Pointer to JIT-compiled function
/// * `env` - The arena environment to thread through execution
///
/// # Returns
/// Tuple of (results, updated_environment) or an error
fn execute_jit_arena_with_env(
    state: &std::sync::Arc<crate::backend::bytecode::ArenaExprCompilationState>,
    native_ptr: *const (),
    env: ArenaEnvironment,
) -> Result<(Vec<ArenaValue<'static>>, ArenaEnvironment), ()> {
    use crate::backend::bytecode::jit::HybridExecutor;
    use crate::backend::eval::trampoline::{get_static_arena, get_static_factory};

    // Get arena and factory from thread-local storage
    let arena = get_static_arena();
    let factory = get_static_factory();

    // Get the bytecode chunk (needed for constants)
    let chunk = state.bytecode_chunk().ok_or(())?;

    // Create hybrid executor and run in arena mode with environment
    let mut executor = HybridExecutor::new();
    executor
        .execute_jit_arena_with_env(&chunk, native_ptr, arena, &factory, env)
        .map_err(|_| ())
}
