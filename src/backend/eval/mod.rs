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
mod mork_forms;
mod pattern;
pub mod priority;
mod processing;
mod quoting;
mod rules;
mod space;
mod step;
mod strings;
mod trampoline;
mod types;
mod utilities;

#[cfg(test)]
mod eval_tests;

use tracing::debug;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

// Re-export from cartesian module
use cartesian::{
    cartesian_product_lazy, CartesianProductResult,
};
// Re-export CartesianProductIter for trampoline module access
pub(crate) use cartesian::CartesianProductIter;

// Re-export from pattern module
pub use pattern::pattern_match;
use pattern::pattern_match_impl;

// Re-export from helpers module
pub use helpers::apply_bindings;
pub(crate) use helpers::friendly_value_repr;
use helpers::{
    friendly_type_name, get_head_symbol, is_grounded_op, pattern_specificity,
    preprocess_space_refs, resolve_tokens_shallow, suggest_special_form_with_context,
    try_eval_builtin, values_equal, SPECIAL_FORMS,
};

// Re-export from rules module
use rules::{try_match_all_rules, try_match_all_rules_iterative, try_match_all_rules_query_multi};

// Re-export from trampoline module
use trampoline::{eval_trampoline, Continuation, WorkItem, MAX_EVAL_DEPTH};

// Re-export from step module
pub(crate) use step::{eval_step, eval_sexpr_step, EvalStep, ProcessedSExpr};

// Re-export from processing module
pub(crate) use processing::{handle_no_rule_match, process_collected_sexpr, process_single_combination};

// Re-export from conjunction module
use conjunction::eval_conjunction;

/// Fast-path check: Is this expression potentially compilable?
///
/// Returns `true` only for SExprs with atom heads that could be compilable operations.
/// Skips tiered cache overhead for:
/// - Atoms (evaluate to themselves instantly)
/// - Literals: Long, Bool, String (evaluate to themselves)
/// - Empty SExprs
/// - SExprs with non-atom heads (data lists like (1 2 3))
///
/// This eliminates 60-80% of tiered cache overhead for typical MeTTa programs.
#[inline]
fn is_potentially_compilable(value: &MettaValue) -> bool {
    match value {
        MettaValue::SExpr(items) if !items.is_empty() => {
            matches!(&items[0], MettaValue::Atom(_))
        }
        _ => false,
    }
}

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
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    debug!(metta_val = ?value);

    // Fast-path: Skip tiered cache for expressions that don't benefit from compilation
    // This avoids hash computation, cache lookup, and stats tracking for atoms/literals
    if !is_potentially_compilable(&value) {
        return eval_trampoline(value, env);
    }

    use crate::backend::bytecode::{
        can_compile_cached, can_compile_with_env, eval_bytecode_hybrid,
        eval_bytecode_with_env, global_tiered_cache, ExecutionTier, TierStatusKind,
    };

    // Record execution in unified tiered cache for async background compilation
    // Returns None during warm-up period to skip tracking overhead for small workloads
    // After warm-up: triggers bytecode compilation at 2 executions, JIT Stage 1 at 100, JIT Stage 2 at 500
    let state_opt = global_tiered_cache().record_execution(&value);

    // During warm-up period, use tree-walker interpreter directly (zero overhead)
    let Some(state) = state_opt else {
        return eval_trampoline(value, env);
    };

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
                    return (results, env);
                }
                // Bytecode execution failed, fall through to hybrid path
            }
        }

        // Try existing hybrid evaluation path (also handles bytecode caching and JIT)
        if let Ok(results) = eval_bytecode_hybrid(&value) {
            global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);
            return (results, env);
        }
    }

    // Try environment-aware bytecode for expressions that need rule dispatch
    if can_compile_with_env(&value) {
        if let Ok((results, new_env)) = eval_bytecode_with_env(&value, env.clone()) {
            global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);
            return (results, new_env);
        }
    }

    // Tier 0: Tree-walker interpreter (cold code or fallback)
    global_tiered_cache().record_tier_execution(ExecutionTier::Interpreter);
    eval_trampoline(value, env)
}

/// Execute a bytecode chunk via HybridExecutor
///
/// The HybridExecutor handles JIT tier dispatch internally, executing via:
/// - JIT native code if hot and compiled
/// - Bytecode VM otherwise
fn execute_bytecode_chunk(chunk: &std::sync::Arc<crate::backend::bytecode::BytecodeChunk>) -> Result<Vec<MettaValue>, ()> {
    use crate::backend::bytecode::{HybridExecutor, global_space_registry, SpaceRegistry};

    let mut executor = HybridExecutor::new();

    // Connect the global space registry
    let registry_ptr = global_space_registry() as *const SpaceRegistry as *mut ();
    unsafe {
        executor.set_space_registry(registry_ptr);
    }

    executor.run(chunk).map_err(|_| ())
}
