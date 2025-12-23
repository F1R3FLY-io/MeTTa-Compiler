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

/// Evaluate a MettaValue in the given environment
/// Returns (results, new_environment)
/// This is the public entry point that uses iterative evaluation with an explicit work stack
/// to prevent stack overflow for large expressions.
///
/// When the `bytecode` feature is enabled, supported expressions are compiled to bytecode
/// and executed by the bytecode VM for improved performance. Complex expressions that
/// require environment access (rules, spaces, etc.) fall back to the tree-walking evaluator.
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    debug!(metta_val = ?value);

    // Try JIT-enabled hybrid evaluation when the jit feature is enabled
    #[cfg(feature = "jit")]
    {
        use crate::backend::bytecode::{
            can_compile_cached, can_compile_with_env, eval_bytecode_hybrid,
            eval_bytecode_with_env, BYTECODE_ENABLED,
        };

        // Only try bytecode/JIT for expressions that don't need environment
        // and when bytecode is enabled at runtime
        if BYTECODE_ENABLED && can_compile_cached(&value) {
            if let Ok(results) = eval_bytecode_hybrid(&value) {
                // Hybrid (JIT/bytecode) evaluation succeeded - return results with unchanged env
                return (results, env);
            }
            // Hybrid evaluation failed (e.g., unsupported operation encountered)
            // Fall through to environment-aware check or tree-walker
        }

        // Try environment-aware bytecode for expressions that need rule dispatch
        // This enables bytecode for workloads like mmverify that use rules
        if BYTECODE_ENABLED && can_compile_with_env(&value) {
            if let Ok((results, new_env)) = eval_bytecode_with_env(&value, env.clone()) {
                return (results, new_env);
            }
            // Environment-aware bytecode failed - fall through to tree-walker
        }
    }

    // Bytecode-only path (without JIT) when jit feature is not enabled
    #[cfg(all(feature = "bytecode", not(feature = "jit")))]
    {
        use crate::backend::bytecode::{
            can_compile_cached, can_compile_with_env, eval_bytecode, eval_bytecode_with_env,
            BYTECODE_ENABLED,
        };

        // Only try bytecode for expressions that don't need environment
        // and when bytecode is enabled at runtime
        if BYTECODE_ENABLED && can_compile_cached(&value) {
            if let Ok(results) = eval_bytecode(&value) {
                // Bytecode evaluation succeeded - return results with unchanged env
                return (results, env);
            }
            // Bytecode failed (e.g., unsupported operation encountered)
            // Fall through to environment-aware check or tree-walker
        }

        // Try environment-aware bytecode for expressions that need rule dispatch
        // This enables bytecode for workloads like mmverify that use rules
        if BYTECODE_ENABLED && can_compile_with_env(&value) {
            if let Ok((results, new_env)) = eval_bytecode_with_env(&value, env.clone()) {
                return (results, new_env);
            }
            // Environment-aware bytecode failed - fall through to tree-walker
        }
    }

    eval_trampoline(value, env)
}
