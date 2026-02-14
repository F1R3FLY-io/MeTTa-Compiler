// Eval module: Arena-based lazy evaluation with pattern matching and built-in dispatch
//
// The evaluation pipeline uses arena-allocated MettaValue exclusively.
// The eval() entry point handles bytecode/JIT tiering with
// tree-walker fallback via eval_trampoline().

#[macro_use]
mod macros;

pub(crate) mod bindings_generic;
mod builtin;
mod cartesian;
mod helpers;
mod list_ops;
pub(crate) mod modules_generic;
pub(crate) mod mork_forms_generic;
mod pattern;
pub mod priority;
mod processing;
mod step;
pub mod trampoline;
pub(crate) mod types_generic;

#[cfg(test)]
mod arena_tests;

// Re-export CartesianProductIter for step module access
pub(crate) use cartesian::CartesianProductIter;

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

    // Session-based GC: reclamation is triggered by SessionGuard::drop() between
    // top-level expressions. No post-eval GC lifecycle needed here — the caller
    // (main.rs, rholang_integration.rs) manages SessionGuard around eval+format.

    result
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
        ExecutionTier, TierStatusKind,
    };

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
                    Ok((results, new_env)) => {
                        global_tiered_cache()
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
    if can_compile_with_env(&value) {
        match eval_bytecode_arena_with_env(&value, env.clone()) {
            Ok((results, new_env)) => {
                global_tiered_cache()
                    .record_tier_execution(ExecutionTier::Bytecode);
                return (results, new_env);
            }
            Err(_) => {
                // Bytecode compilation/execution failed, fall through to tree-walker
            }
        }
    }

    // Tier 0: Tree-walker interpreter (cold code or fallback)
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
