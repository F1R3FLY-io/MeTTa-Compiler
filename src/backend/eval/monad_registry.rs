//! Monadic Type Registry
//!
//! Single source of truth for which type constructors are monadic.
//! Monadic types represent computations with effects (IO, State, etc.)
//! that must not be memoized — repeated evaluations must re-execute
//! their side effects.
//!
//! To add a new monad: add its constructor name to `MONADIC_CONSTRUCTORS`
//! and add a corresponding `TypeExpr` variant in `builtin_signatures.rs`.

/// Known monadic type constructor names.
///
/// A type `(M X)` where `M` is in this list is considered monadic.
/// The caching system refuses to memoize expressions that return
/// monadic types, ensuring side effects are re-executed on each call.
static MONADIC_CONSTRUCTORS: &[&str] = &["IO", "StateMonad"];

/// Check if a type constructor name is a known monad.
#[inline]
pub fn is_monadic_constructor(name: &str) -> bool {
    MONADIC_CONSTRUCTORS.contains(&name)
}

/// Check if a type constructor name is specifically the IO monad.
#[inline]
pub fn is_io_constructor(name: &str) -> bool {
    name == "IO"
}
