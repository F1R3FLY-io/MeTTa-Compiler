//! Arena-based Trampoline Engine
//!
//! This module provides zero-conversion arena evaluation using `StaticArenaContext`.
//! By using `Box::leak` to create a `'static` arena, `ArenaValue<'static>` can satisfy
//! the `'static` bound required by `GenericEnvironment`.
//!
//! ## Design
//!
//! The key insight is that we can use `Box::leak` to create a `'static` arena:
//! - Thread-local storage ensures each thread has its own arena
//! - The `'static` lifetime allows `ArenaValue<'static>` to be stored in DashMap/PathMap
//! - Unsafe `Send + Sync` implementations are safe because arena values are immutable
//!
//! ## Zero-Conversion Pipeline
//!
//! When `METTA_USE_ARENA=1`:
//! 1. `compile_arena()` parses directly to `ArenaValue<'static>`
//! 2. `eval_trampoline_arena()` evaluates `ArenaValue<'static>` throughout
//! 3. Results are `Vec<ArenaValue<'static>>` - no conversion needed
//!
//! ## Memory Management
//!
//! The static arena is never dropped (that's what `Box::leak` does). This is
//! intentional for batch processing where all allocations happen during evaluation.
//! For long-running processes, consider periodic arena resets.

use bumpalo::Bump;

use crate::backend::models::ArenaValue;

use super::context::{ArenaContext, ArenaEnvironment, StaticArenaContext};
use super::generic_trampoline::eval_trampoline_generic;
use super::generic_types::GenericEvalResult;

/// Type alias for arena evaluation result.
///
/// This is the return type of `eval_trampoline_arena` - a tuple of:
/// - `Vec<ArenaValue<'static>>`: The evaluation results
/// - `ArenaEnvironment`: The updated environment
pub type ArenaEvalResult = GenericEvalResult<ArenaValue<'static>, ArenaEnvironment>;

/// Create an arena context for short-lived operations.
///
/// Helper function to create an ArenaContext from a Bump arena.
/// This is useful for operations that don't need the static arena,
/// such as parsing or transformation within a specific scope.
///
/// For evaluation that requires storage in `GenericEnvironment`, use
/// `StaticArenaContext::get()` instead.
#[inline]
pub fn create_arena_context(arena: &Bump) -> ArenaContext<'_> {
    ArenaContext::new(arena)
}

/// Zero-conversion arena evaluation.
///
/// This function evaluates `ArenaValue<'static>` using the unified generic trampoline
/// engine. No conversions are performed - values remain as `ArenaValue<'static>`
/// throughout the entire evaluation process.
///
/// # Arguments
///
/// - `value`: The value to evaluate (must be `ArenaValue<'static>`)
/// - `env`: The evaluation environment (`ArenaEnvironment`)
///
/// # Returns
///
/// A tuple of (results, final_environment) where results are `Vec<ArenaValue<'static>>`.
///
/// # Example
///
/// ```ignore
/// use mettatron::backend::compile::compile_arena;
/// use mettatron::backend::eval::trampoline::{eval_trampoline_arena, StaticArenaContext};
///
/// // Compile directly to ArenaValue<'static>
/// let exprs = compile_arena("!(+ 1 2)").unwrap();
///
/// // Create arena environment
/// let env = StaticArenaContext::new_env();
///
/// // Evaluate - zero conversions throughout
/// for expr in exprs {
///     let (results, env) = eval_trampoline_arena(expr, env);
///     for result in results {
///         println!("{}", result.friendly_repr());
///     }
/// }
/// ```
#[inline]
pub fn eval_trampoline_arena(value: ArenaValue<'static>, env: ArenaEnvironment) -> ArenaEvalResult {
    let ctx = StaticArenaContext::get();
    eval_trampoline_generic(value, env, &ctx)
}

/// Check if arena mode is available.
///
/// Returns `true` - arena evaluation is now fully implemented using
/// `StaticArenaContext` with `Box::leak` for `'static` arena.
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
        let factory = get_static_factory();
        let env = StaticArenaContext::new_env();
        let value = factory.atom("hello");

        let (results, _env) = eval_trampoline_arena(value, env);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_atom());
        assert_eq!(results[0].as_atom(), Some("hello"));
    }

    #[test]
    fn test_eval_simple_number() {
        let factory = get_static_factory();
        let env = StaticArenaContext::new_env();
        let value = factory.long(42);

        let (results, _env) = eval_trampoline_arena(value, env);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_long());
        assert_eq!(results[0].as_long(), Some(42));
    }

    #[test]
    fn test_eval_simple_arithmetic() {
        let factory = get_static_factory();
        let env = StaticArenaContext::new_env();

        // Create (+ 1 2)
        let value = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);

        let (results, _env) = eval_trampoline_arena(value, env);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_long());
        assert_eq!(results[0].as_long(), Some(3));
    }
}
