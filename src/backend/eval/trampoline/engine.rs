//! Monomorphized Evaluation Helpers for MettaValue
//!
//! Concrete specializations of the generic evaluation functions for the
//! MettaValue/GcFactory type pair. These thin wrappers enable callers to
//! use simple function signatures without generic type parameters.
//!
//! MettaValue is Copy (8-byte tagged pointer), so clone() is a no-op memcpy.
//! GcFactory is Copy + Clone, used as a global allocator.
//!
//! ## Migration Path
//!
//! Callers should progressively migrate from `generic_engine::*` to `engine::*`.
//! Once all callers are migrated, the generic versions can be inlined here
//! and the generic_engine module removed.

use smallvec::SmallVec;

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{MettaValue, GcFactory, GenericBindings, MettaValueTrait, MettaValueFactory};

/// Concrete type aliases (monomorphized from generic types).
pub type Environment = GenericEnvironment<MettaValue, GcFactory>;
pub type Bindings = GenericBindings<MettaValue>;
pub type WorkItem = super::generic_types::GenericWorkItem<MettaValue, Environment>;
pub type Continuation = super::generic_types::GenericContinuation<MettaValue, Environment>;
pub type EvalResult = super::generic_types::GenericEvalResult<MettaValue>;

/// Apply bindings to a MettaValue, substituting variables with bound values.
///
/// Monomorphized for MettaValue (Copy, 8-byte tagged pointer).
/// All `value.clone()` calls are zero-cost copies.
#[inline]
pub fn apply_bindings(value: &MettaValue, bindings: &Bindings, factory: &GcFactory) -> MettaValue {
    // Fast path: empty bindings means no substitutions possible.
    // MettaValue is Copy — returning *value is a free 8-byte copy.
    if bindings.is_empty() {
        return *value;
    }

    // Peel Spanned wrapper: process inner value, re-wrap with same span
    if let Some(span) = value.span() {
        let span = *span;
        let result = apply_bindings_inner(value, bindings, factory);
        if result.span().is_some() {
            return result;
        }
        return factory.spanned(result, span);
    }

    apply_bindings_inner(value, bindings, factory)
}

/// Inner implementation after Spanned is peeled.
/// MettaValue is Copy — no heap allocation for returns.
fn apply_bindings_inner(value: &MettaValue, bindings: &Bindings, factory: &GcFactory) -> MettaValue {
    // Handle variables (atoms starting with $, &, or ')
    if let Some(var_name) = value.as_atom() {
        if (var_name.starts_with('$') || var_name.starts_with('&') || var_name.starts_with('\''))
            && var_name != "&"
        {
            if let Some(bound_value) = bindings.get(var_name) {
                return *bound_value; // Copy: O(1) 8-byte pointer copy
            }
        }
        return *value;
    }

    // Fast path: no variables → return unchanged (O(1) tagged-pointer flag check)
    if !value.has_variables_fast() {
        return *value;
    }

    // Types are returned as-is (no substitution)
    if value.is_type() {
        return *value;
    }

    // S-expressions: recursively apply bindings with identity short-circuit
    if let Some(items) = value.as_sexpr() {
        let mut any_changed = false;
        let new_items: SmallVec<[MettaValue; 8]> = items
            .iter()
            .map(|item| {
                if !item.has_variables_fast() {
                    return *item;
                }
                let result = apply_bindings(item, bindings, factory);
                if !any_changed && !result.identity_eq(item) {
                    any_changed = true;
                }
                result
            })
            .collect();
        if !any_changed {
            return *value;
        }
        return factory.sexpr_from_slice(&new_items);
    }

    // Conjunctions: same identity short-circuit
    if let Some(goals) = value.as_conjunction() {
        let mut any_changed = false;
        let new_goals: SmallVec<[MettaValue; 8]> = goals
            .iter()
            .map(|goal| {
                if !goal.has_variables_fast() {
                    return *goal;
                }
                let result = apply_bindings(goal, bindings, factory);
                if !any_changed && !result.identity_eq(goal) {
                    any_changed = true;
                }
                result
            })
            .collect();
        if !any_changed {
            return *value;
        }
        return factory.conjunction_from_slice(&new_goals);
    }

    // Errors: identity short-circuit on details
    if let Some((msg, details)) = value.as_error() {
        let new_details = apply_bindings(&details, bindings, factory);
        if new_details.identity_eq(&details) {
            return *value;
        }
        return factory.error(msg, new_details);
    }

    // Ground values: return as-is (Copy)
    *value
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
) -> SwitchResult<MettaValue> {
    super::generic_engine::eval_switch_generic(atom, cases, factory)
}

/// Check if a pattern is a boolean check optimization candidate.
#[inline]
pub fn is_boolean_check_pattern(success_body: &MettaValue, failure_body: &MettaValue) -> bool {
    super::generic_engine::is_boolean_check_pattern(success_body, failure_body)
}

/// Apply bindings to a template using the iterative implementation.
///
/// This delegates to the iterative `apply_bindings_generic` from
/// `bindings_generic.rs`, which uses an explicit work stack instead of
/// recursion. Used by environment matching and space operations.
#[inline]
pub fn apply_bindings_iterative(template: &MettaValue, bindings: &Bindings, factory: &GcFactory) -> MettaValue {
    crate::backend::eval::bindings_generic::apply_bindings_generic(template, bindings, factory)
}
