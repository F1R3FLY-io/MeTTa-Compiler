//! Arena-based Trampoline Engine
//!
//! This module provides zero-conversion arena evaluation using session-scoped
//! dual-arena allocation with O(1) bulk deallocation.
//!
//! ## Dual Arena Model
//!
//! - **Eval Arena**: Thread-local, generation-based reset for intermediates (~95% of allocations)
//! - **Storage Arena**: Session-owned via ArenaState, O(1) bulk free on drop (~5% of allocations)
//!
//! ## Zero-Conversion Pipeline
//!
//! When `METTA_USE_ARENA=1`:
//! 1. `compile_arena()` compiles directly to `ArenaState` with `ArenaValue<'static>`
//! 2. `eval_trampoline_arena()` evaluates `ArenaValue<'static>` using `SessionContext`
//! 3. Results are `Vec<ArenaValue<'static>>` - no conversion needed
//!
//! ## Memory Management
//!
//! When `ArenaState` drops, ALL arena memory is freed instantly via O(1) bulk
//! deallocation (no recursive tree traversal). The eval generation counter is
//! incremented, causing thread-local eval arenas to reset lazily on next access.

use bumpalo::Bump;

use crate::backend::models::{ArenaState, ArenaValue};

use super::context::{ArenaEnvironment, StaticArenaContext};
use super::generic_trampoline::eval_trampoline_generic;
use super::generic_types::GenericEvalResult;
use super::session_context::SessionContext;

/// Type alias for arena evaluation result.
///
/// This is the return type of `eval_trampoline_arena` - a tuple of:
/// - `Vec<ArenaValue<'static>>`: The evaluation results
/// - `ArenaEnvironment`: The updated environment
pub type ArenaEvalResult = GenericEvalResult<ArenaValue<'static>, ArenaEnvironment>;

/// Zero-conversion arena evaluation using session-scoped dual-arena model.
///
/// This function evaluates `ArenaValue<'static>` using the unified generic trampoline
/// engine with `SessionContext` for dual-arena allocation. No conversions are
/// performed - values remain as `ArenaValue<'static>` throughout.
///
/// # Arguments
///
/// - `value`: The value to evaluate (must be `ArenaValue<'static>`)
/// - `env`: The evaluation environment (`ArenaEnvironment`)
/// - `state`: The `ArenaState` owning the storage arena
///
/// # Returns
///
/// A tuple of (results, final_environment) where results are `Vec<ArenaValue<'static>>`.
///
/// # Example
///
/// ```ignore
/// use mettatron::backend::compile::compile_arena;
/// use mettatron::backend::eval::trampoline::eval_trampoline_arena;
///
/// // Compile to ArenaState
/// let state = compile_arena("!(+ 1 2)").unwrap();
///
/// // Create arena environment
/// let env = ArenaEnvironment::new(state.storage_factory().into());
///
/// // Evaluate - zero conversions throughout
/// for &expr in state.source() {
///     let (results, env) = eval_trampoline_arena(expr, env, &state);
///     for result in results {
///         println!("{}", result.friendly_repr());
///     }
/// }
/// ```
#[inline]
pub fn eval_trampoline_arena(
    value: ArenaValue<'static>,
    env: ArenaEnvironment,
    state: &ArenaState,
) -> ArenaEvalResult {
    let ctx = SessionContext::new(state);
    eval_trampoline_generic(value, env, &ctx)
}

/// Check if arena mode is available.
///
/// Returns `true` - arena evaluation is now fully implemented using
/// session-scoped dual-arena allocation.
#[inline]
pub fn is_arena_mode_available() -> bool {
    true
}

/// Get the thread-local static arena.
///
/// This returns the `&'static Bump` arena used by arena evaluation.
/// Useful for external code that needs to allocate values in the same
/// arena used by evaluation.
///
/// # Thread Safety
///
/// Each thread has its own arena via `thread_local!`. Values created
/// in one thread's arena should not be shared with other threads.
#[inline]
pub fn get_static_arena() -> &'static Bump {
    StaticArenaContext::get_arena()
}

/// Get the thread-local static factory.
///
/// This returns the factory for creating `ArenaValue<'static>` in the
/// thread-local static arena. Useful for external code that needs to
/// create values in the same arena used by evaluation.
#[inline]
pub fn get_static_factory() -> crate::backend::models::ArenaValueFactory<'static> {
    StaticArenaContext::get_factory()
}

/// Create a new `ArenaEnvironment` for session-based evaluation.
///
/// The environment uses the eval arena factory from the `ArenaState`,
/// which is appropriate since most environment operations (pattern matching,
/// binding lookup) work with intermediates that don't need to persist.
#[inline]
pub fn new_arena_env() -> ArenaEnvironment {
    use crate::backend::models::{get_eval_arena, ArenaValueFactory};
    ArenaEnvironment::new(ArenaValueFactory::new(get_eval_arena()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValueFactory, MettaValueTrait};

    #[test]
    fn test_arena_mode_available() {
        assert!(is_arena_mode_available());
    }

    #[test]
    fn test_get_static_arena() {
        let arena = get_static_arena();
        // Allocate something to verify arena works
        let s = arena.alloc_str("test");
        assert_eq!(s, "test");
    }

    #[test]
    fn test_get_static_factory() {
        let factory = get_static_factory();
        let value = factory.atom("test");
        assert!(value.is_atom());
        assert_eq!(value.as_atom(), Some("test"));
    }

    #[test]
    fn test_eval_simple_atom() {
        let state = ArenaState::new();
        let factory = state.storage_factory();
        let env = new_arena_env();
        let value = factory.atom("hello");

        let (results, _env) = eval_trampoline_arena(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_atom());
        assert_eq!(results[0].as_atom(), Some("hello"));
    }

    #[test]
    fn test_eval_simple_number() {
        let state = ArenaState::new();
        let factory = state.storage_factory();
        let env = new_arena_env();
        let value = factory.long(42);

        let (results, _env) = eval_trampoline_arena(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_long());
        assert_eq!(results[0].as_long(), Some(42));
    }

    #[test]
    fn test_eval_simple_arithmetic() {
        let state = ArenaState::new();
        let factory = state.storage_factory();
        let env = new_arena_env();

        // Create (+ 1 2)
        let value = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);

        let (results, _env) = eval_trampoline_arena(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_long());
        assert_eq!(results[0].as_long(), Some(3));
    }
}
