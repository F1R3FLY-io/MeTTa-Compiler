//! Monomorphized Evaluation Helpers for MettaValue
//!
//! Concrete specializations of the generic evaluation functions for the
//! MettaValue/GcFactory type pair. These thin wrappers enable callers to
//! use simple function signatures without generic type parameters.
//!
//! MettaValue is Copy (8-byte tagged pointer), so clone() is a no-op memcpy.
//! GcFactory is Copy + Clone, used as a global allocator.

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{GcFactory, GenericBindings, MettaValue};

/// Type aliases for the concrete monomorphized types.
pub type Environment = GenericEnvironment<MettaValue, GcFactory>;
pub type Bindings = GenericBindings<MettaValue>;

/// Apply bindings to a MettaValue, substituting variables with bound values.
///
/// MettaValue is Copy, so `value.clone()` in the generic version is a free
/// 8-byte pointer copy. No heap allocation for the return value itself.
#[inline]
pub fn apply_bindings(value: &MettaValue, bindings: &Bindings, factory: &GcFactory) -> MettaValue {
    super::generic_engine::apply_bindings_generic(value, bindings, factory)
}

/// Pattern match a pattern against a value, returning bindings if successful.
#[inline]
pub fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
    super::generic_engine::pattern_match_generic(pattern, value)
}

/// Match all rules against an expression, returning (rhs, bindings, rhs_type) triples.
#[inline]
pub fn try_match_all_rules(
    expr: &MettaValue,
    env: &Environment,
    factory: GcFactory,
) -> Vec<(MettaValue, Bindings, Option<MettaValue>)> {
    super::generic_engine::try_match_all_rules_generic(expr, env, factory)
}

/// Try binding-aware rule matching (SG1 path) without materializing.
#[inline]
pub fn try_match_rules_with_bindings(
    template: &MettaValue,
    outer_bindings: &Bindings,
    resolved_head: &str,
    arity: usize,
    env: &Environment,
    factory: &GcFactory,
) -> Option<Vec<(MettaValue, Bindings)>> {
    super::generic_engine::try_match_rules_with_bindings(
        template, outer_bindings, resolved_head, arity, env, factory,
    )
}

/// Try deterministic chain evaluation (Phase F fast path).
#[inline]
pub fn try_deterministic_chain(
    expr: &MettaValue,
    env: &Environment,
    factory: &GcFactory,
) -> Option<MettaValue> {
    super::generic_engine::try_deterministic_chain(expr, env, factory)
}

/// Try deferred deterministic chain for EvalWithBindings.
pub use super::generic_engine::DeferredChainResult;

#[inline]
pub fn try_deferred_deterministic_chain(
    template: &MettaValue,
    bindings: &Bindings,
    env: &Environment,
    factory: &GcFactory,
) -> Option<DeferredChainResult<MettaValue>> {
    super::generic_engine::try_deferred_deterministic_chain(template, bindings, env, factory)
}

/// Evaluate switch/case expression.
pub use super::generic_engine::GenericSwitchResult as SwitchResult;

#[inline]
pub fn eval_switch(
    atom: &MettaValue,
    cases: &MettaValue,
    factory: &GcFactory,
) -> super::generic_engine::GenericSwitchResult<MettaValue> {
    super::generic_engine::eval_switch_generic(atom, cases, factory)
}

/// Check if a pattern is a boolean check optimization candidate.
#[inline]
pub fn is_boolean_check_pattern(success_body: &MettaValue, failure_body: &MettaValue) -> bool {
    super::generic_engine::is_boolean_check_pattern(success_body, failure_body)
}
