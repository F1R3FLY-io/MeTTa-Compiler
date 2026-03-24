//! Generic Evaluation Helpers - Zero-Conversion Utilities
//!
//! This module provides generic helper functions for evaluation that work with any
//! value type implementing `MettaValueTrait`. These utilities enable zero-conversion
//! evaluation for both heap-allocated (`MettaValue`) and arena-allocated (`MettaValue`)
//! values.
//!
//! ## Design
//!
//! The generic helpers use:
//! - `MettaValueTrait` for type checking and value inspection
//! - `MettaValueFactory` for value construction
//! - `GenericBindings<V>` for storing bindings in the native value type
//!
//! ## Key Functions
//!
//! - `apply_bindings_generic` - Apply bindings to a value (zero-conversion)
//! - `pattern_match_generic` - Pattern matching returning native bindings
//! - `pattern_specificity_generic` - REMOVED: MeTTa HE has no specificity filter
//! - `try_match_all_rules_generic` - Match all rules against an expression
//! - `eval_switch_generic` - Generic switch/case evaluation
//! - `is_boolean_check_pattern` - Detect boolean check optimization patterns
//!
//! ## Zero-Conversion Architecture
//!
//! These functions achieve zero MettaValue <-> MettaValue conversion by:
//! 1. Using `GenericBindings<V>` - bindings store values in their native type
//! 2. Pattern matching returns bindings in the value's native type
//! 3. Binding application operates natively on the value type
//! 4. Rule matching deserializes rules directly to the target type V

use smallvec::SmallVec;

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

use super::dispatch_hints::{match_result_get, match_result_put};

// MettaValue only used in tests
#[cfg(test)]
use crate::backend::models::MettaValue;

// ============================================================================
// Generic Helper Functions
// ============================================================================

/// Apply bindings to a value using trait methods.
///
/// This is a generic version of `apply_bindings` that works with any value
/// type implementing `MettaValueTrait`.
///
/// This implementation matches the heap-based `apply_bindings` in `helpers.rs`,
/// including the "&" exclusion and NOT recursing into Type variants.
///
/// ## Zero-Conversion Design
///
/// When using `GenericBindings<V>`, bound values are stored in the same type
/// as the input value, so no conversion is needed:
/// - `MettaValue.clone()` = O(1) Arc increment
/// - `MettaValue.clone()` = O(1) pointer copy
pub fn apply_bindings_generic<V, F>(value: &V, bindings: &GenericBindings<V>, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Fast path: empty bindings means no substitutions possible
    if bindings.is_empty() {
        return value.clone();
    }

    // Peel Spanned wrapper: process inner value, re-wrap with same span
    if let Some(span) = value.span() {
        let span = *span; // Copy — Span is Copy
        // as_atom()/as_sexpr()/etc. see through Spanned, so we can let the
        // rest of the function process the value normally, then re-wrap.
        let result = apply_bindings_generic_inner(value, bindings, factory);
        // Avoid double-Spanned: if the result already carries a span (e.g.,
        // a variable was substituted with a value that has its own span),
        // use the result as-is rather than wrapping it in another Spanned layer.
        // Double-Spanned values cause incorrect behavior in condition checks
        // (e.g., `if` only strips one span layer).
        if result.span().is_some() {
            return result;
        }
        return factory.spanned(result, span);
    }

    apply_bindings_generic_inner(value, bindings, factory)
}

/// Inner implementation of apply_bindings_generic (called after Spanned is peeled).
fn apply_bindings_generic_inner<V, F>(value: &V, bindings: &GenericBindings<V>, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Handle variables (atoms starting with $, &, or ')
    // IMPORTANT: standalone "&" is a literal operator (used in match), not a variable
    if let Some(var_name) = value.as_atom() {
        if (var_name.starts_with('$') || var_name.starts_with('&') || var_name.starts_with('\''))
            && var_name != "&"
        {
            if let Some(bound_value) = bindings.get(var_name) {
                // NO CONVERSION NEEDED - bound_value is already type V
                // This is the key optimization: O(1) clone with no deep allocation
                return bound_value.clone();
            }
        }
        return value.clone();
    }

    // Fast path: if value contains no variables, bindings cannot affect it.
    // This avoids O(N) recursion + Vec allocation + factory.sexpr() for ground
    // values like (+ 1 2). MettaValue::clone() is O(1) pointer copy.
    // Uses O(1) tagged pointer flag check instead of O(depth) tree walk.
    if !value.has_variables_fast() {
        return value.clone();
    }

    // Type variants do NOT recurse (matching heap behavior in helpers.rs)
    // Types are returned as-is without substitution
    if value.is_type() {
        return value.clone();
    }

    // Handle S-expressions - recursively apply bindings.
    // Identity short-circuit: if no child was actually substituted, return the
    // original value (O(1) pointer copy) instead of allocating a new S-expression.
    // SmallVec<[V; 8]> avoids heap allocation for arity ≤ 8 (the vast majority).
    if let Some(items) = value.as_sexpr() {
        let mut any_changed = false;
        let new_items: SmallVec<[V; 8]> = items
            .iter()
            .map(|item| {
                // Per-child fast path: skip recursion for ground children.
                // Avoids function call overhead (Spanned check, atom check, etc.)
                // for children that cannot be affected by bindings.
                if !item.has_variables_fast() {
                    return item.clone();
                }
                let result = apply_bindings_generic(item, bindings, factory);
                // O(1) identity check via tagged pointer comparison.
                // Avoids O(n) structural PartialEq fallthrough.
                if !any_changed && !result.identity_eq(item) {
                    any_changed = true;
                }
                result
            })
            .collect();
        if !any_changed {
            return value.clone();
        }
        return factory.sexpr_from_slice(&new_items);
    }

    // Handle conjunctions - same identity short-circuit optimization
    if let Some(goals) = value.as_conjunction() {
        let mut any_changed = false;
        let new_goals: SmallVec<[V; 8]> = goals
            .iter()
            .map(|goal| {
                if !goal.has_variables_fast() {
                    return goal.clone();
                }
                let result = apply_bindings_generic(goal, bindings, factory);
                if !any_changed && !result.identity_eq(goal) {
                    any_changed = true;
                }
                result
            })
            .collect();
        if !any_changed {
            return value.clone();
        }
        return factory.conjunction(new_goals.into_vec());
    }

    // Handle errors - identity short-circuit
    if let Some((msg, details)) = value.as_error() {
        let new_details = apply_bindings_generic(details, bindings, factory);
        if new_details.identity_eq(details) {
            return value.clone();
        }
        return factory.error(msg, new_details);
    }

    // All other types (ground values: Long, Float, Bool, String, Nil, Unit,
    // Space, State, Memo, Empty) are returned as-is
    value.clone()
}


/// Pattern match two values generically.
///
/// Returns bindings if the pattern matches the value, None otherwise.
///
/// This implementation matches the heap-based `pattern_match` in `pattern.rs`,
/// including cross-type matching for Nil/Unit/Empty and the "&" exclusion.
///
/// ## Zero-Conversion Design
///
/// By using `GenericBindings<V>`, matched values are stored directly in their
/// native type with no conversion:
/// - `MettaValue.clone()` = O(1) Arc increment
/// - `MettaValue.clone()` = O(1) pointer copy
pub fn pattern_match_generic<V>(pattern: &V, value: &V) -> Option<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
{
    // Helper to check if a name is a variable
    // IMPORTANT: standalone "&" is a literal operator (used in match), not a variable
    fn is_variable(name: &str) -> bool {
        (name.starts_with('$') || name.starts_with('&') || name.starts_with('\'')) && name != "&"
    }

    // Helper to check if a name is a wildcard
    fn is_wildcard(name: &str) -> bool {
        name == "_"
    }

    // Handle pattern atom
    if let Some(pattern_name) = pattern.as_atom() {
        // Wildcard matches anything
        if is_wildcard(pattern_name) {
            return Some(GenericBindings::new());
        }

        // Variable binds to value (excluding standalone "&")
        if is_variable(pattern_name) {
            let mut bindings = GenericBindings::new();
            // NO CONVERSION - store value directly in its native type
            // This is O(1) clone (Arc increment for MettaValue, pointer copy for MettaValue)
            bindings.insert(pattern_name, value.clone());
            return Some(bindings);
        }

        // Empty atom pattern matches Empty sentinel
        if pattern_name == "Empty" && value.is_empty() {
            return Some(GenericBindings::new());
        }

        // Non-variable atom must match exactly
        if let Some(value_name) = value.as_atom() {
            if pattern_name == value_name {
                return Some(GenericBindings::new());
            }
        }
        return None;
    }

    // Handle S-expression pattern
    if let Some(pattern_items) = pattern.as_sexpr() {
        // Empty S-expression () matches only empty values (empty S-expr, Nil, Unit, or Empty atom)
        if pattern_items.is_empty() {
            // Empty S-expr matches empty S-expr
            if let Some(value_items) = value.as_sexpr() {
                if value_items.is_empty() {
                    return Some(GenericBindings::new());
                }
            }
            // Empty S-expr matches Unit
            if value.is_unit() {
                return Some(GenericBindings::new());
            }
            // Empty S-expr matches Atom("Empty")
            if let Some(name) = value.as_atom() {
                if name == "Empty" {
                    return Some(GenericBindings::new());
                }
            }
            return None;
        }

        if let Some(value_items) = value.as_sexpr() {
            if pattern_items.len() != value_items.len() {
                return None;
            }

            let mut combined_bindings = GenericBindings::new();
            for (p, v) in pattern_items.iter().zip(value_items.iter()) {
                match pattern_match_generic(p, v) {
                    Some(sub_bindings) => {
                        // Merge bindings using the merge method which checks for conflicts
                        if !combined_bindings.merge(&sub_bindings) {
                            return None; // Conflict detected
                        }
                    }
                    None => return None,
                }
            }
            return Some(combined_bindings);
        }
        return None;
    }

    // Handle Conjunction pattern
    if let Some(pattern_goals) = pattern.as_conjunction() {
        if let Some(value_goals) = value.as_conjunction() {
            if pattern_goals.len() != value_goals.len() {
                return None;
            }

            let mut combined_bindings = GenericBindings::new();
            for (p, v) in pattern_goals.iter().zip(value_goals.iter()) {
                match pattern_match_generic(p, v) {
                    Some(sub_bindings) => {
                        if !combined_bindings.merge(&sub_bindings) {
                            return None; // Conflict
                        }
                    }
                    None => return None,
                }
            }
            return Some(combined_bindings);
        }
        return None;
    }

    // Handle Error pattern
    if let Some((pattern_msg, pattern_details)) = pattern.as_error() {
        if let Some((value_msg, value_details)) = value.as_error() {
            if pattern_msg != value_msg {
                return None;
            }
            return pattern_match_generic(pattern_details, value_details);
        }
        return None;
    }

    // Handle ground types - must match exactly (use direct equality, not epsilon)
    if let (Some(p), Some(v)) = (pattern.as_bool(), value.as_bool()) {
        return if p == v {
            Some(GenericBindings::new())
        } else {
            None
        };
    }

    if let (Some(p), Some(v)) = (pattern.as_long(), value.as_long()) {
        return if p == v {
            Some(GenericBindings::new())
        } else {
            None
        };
    }

    // Float comparison uses direct equality (matching heap behavior)
    if let (Some(p), Some(v)) = (pattern.as_float(), value.as_float()) {
        return if p == v {
            Some(GenericBindings::new())
        } else {
            None
        };
    }

    if let (Some(p), Some(v)) = (pattern.as_string(), value.as_string()) {
        return if p == v {
            Some(GenericBindings::new())
        } else {
            None
        };
    }

    // Unit pattern matches Unit and empty S-expr
    if pattern.is_unit() {
        if value.is_unit() {
            return Some(GenericBindings::new());
        }
        if let Some(items) = value.as_sexpr() {
            if items.is_empty() {
                return Some(GenericBindings::new());
            }
        }
        if let Some(name) = value.as_atom() {
            if name == "Empty" {
                return Some(GenericBindings::new());
            }
        }
        return None;
    }

    // Empty matches empty
    if pattern.is_empty() && value.is_empty() {
        return Some(GenericBindings::new());
    }

    None
}


// ============================================================================
// Generic Rule Matching
// ============================================================================

// DEAD CODE: pattern_specificity_generic was removed because MeTTa HE has no
// specificity filter — all matching rules fire nondeterministically. The specificity
// filter in rule_management.rs was the only consumer, and it has been removed.
// The function incorrectly dropped structurally-more-specific rules when a variable-only
// rule happened to have fewer NewVar tags (e.g. PLN's `(f ($c $tv) $y)` with 3 vars
// beat `(f ((Implication $A $B) $TV) $Y)` with 4 vars despite the latter being more
// specific due to the `(Implication ...)` constructor constraint).
//
// pub fn pattern_specificity_generic<V: MettaValueTrait>(pattern: &V) -> usize { ... }

/// Try to match all rules against a generic expression.
///
/// This is the generic version of `try_match_all_rules` that works with any
/// value type implementing `MettaValueTrait`. It retrieves rules from the
/// environment using `get_matching_rules_for_expr` and performs pattern matching
/// without any value type conversions.
///
/// # Zero-Conversion Design
///
/// This function:
/// 1. Retrieves `(lhs, rhs, multiplicity)` tuples from the environment
/// 2. Pattern matches using `pattern_match_generic` (no conversion)
/// 3. Returns `GenericBindings<MettaValue>` (no conversion)
///
/// # Type Parameters
///
/// - `V`: The value type (MettaValue or MettaValue)
/// - `F`: The factory type (must implement MettaValueFactory<V> + Copy)
///
/// # Returns
///
/// A vector of (rhs, bindings) pairs for all matching rules, sorted by specificity
/// and expanded by rule multiplicity.
/// Phase 8.7: Return type includes `rhs_type` for branch pruning.
/// The third element is the cached RHS type from the rule entry (if available).
pub fn try_match_all_rules_generic<V, F>(
    expr: &V,
    env: &GenericEnvironment<V, F>,
    _factory: F,
) -> Vec<(V, GenericBindings<V>, Option<V>)>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    let is_metta = std::any::TypeId::of::<V>() == std::any::TypeId::of::<crate::backend::models::MettaValue>();
    let expr_arity = expr.get_arity();

    // Phase E: For single-candidate all-structural operators, skip hash computation
    // and match_result_cache entirely. The cache rarely hits for these (different args
    // each call), so the hash overhead (~700ns) exceeds any cache benefit.
    if is_metta {
        if let Some(head) = expr.as_sexpr().and_then(|items| items.first()).and_then(|h| h.as_atom()) {
            if let Some(cache_entry) = operator_cache_get(head, expr_arity) {
                if cache_entry.all_structural && cache_entry.candidate_count == 1 {
                    // Fast path: skip hash, skip match_result_cache, go straight to structural match
                    let results = env.match_rules_native(expr, |v: &V, _: &GenericBindings<V>, _: &F| v.clone());
                    return results
                        .into_iter()
                        .map(|r| (r.rhs_template, r.bindings, r.rhs_type))
                        .collect();
                }
            }
        }
    }

    // Standard path: compute hash and use match_result_cache
    let expr_hash = expr.hash_value();

    if is_metta {
        if let Some(cached) = match_result_get(expr_hash, expr_arity) {
            // Safety: V = MettaValue verified by TypeId check above.
            // Both types have identical layout, so Vec reinterpretation is sound.
            return unsafe {
                let mut md = std::mem::ManuallyDrop::new(cached);
                Vec::from_raw_parts(
                    md.as_mut_ptr() as *mut (V, GenericBindings<V>, Option<V>),
                    md.len(),
                    md.capacity(),
                )
            };
        }
    }

    // Use native byte-level matching via RuleIndex + extract_data.
    // match_rules_native also populates the operator cache for Phase E.
    let results = env.match_rules_native(expr, |v: &V, _: &GenericBindings<V>, _: &F| v.clone());
    let result_vec: Vec<(V, GenericBindings<V>, Option<V>)> = results
        .into_iter()
        .map(|r| (r.rhs_template, r.bindings, r.rhs_type))
        .collect();

    // Store in match result cache
    if is_metta && !result_vec.is_empty() {
        // Safety: V = MettaValue verified by TypeId check above.
        let slice: &[(crate::backend::models::MettaValue, GenericBindings<crate::backend::models::MettaValue>, Option<crate::backend::models::MettaValue>)] = unsafe {
            std::slice::from_raw_parts(
                result_vec.as_ptr()
                    as *const (crate::backend::models::MettaValue, GenericBindings<crate::backend::models::MettaValue>, Option<crate::backend::models::MettaValue>),
                result_vec.len(),
            )
        };
        match_result_put(expr_hash, expr_arity, slice);
    }

    result_vec
}

// ============================================================================
// SG1: Binding-Aware Rule Matching (WAM-style lazy variable resolution)
// ============================================================================
//
// Instead of materializing `apply_bindings(template, outer_bindings)` and
// then calling `try_match_all_rules_generic` on the result, this function
// matches rules directly against the unresolved template by resolving
// variables on-the-fly through `outer_bindings` inside the structural matcher.
//
// This eliminates the O(tree) allocation from `apply_bindings_generic` for
// the common case where all rule candidates have structural matchers.
//
// Returns `Some(matches)` if binding-aware matching was possible (even if
// no rules matched — empty vec means "no match, self-evaluate").
// Returns `None` if binding-aware matching was not possible (caller must
// fall back to materialization).

/// Try to match rules against a template + bindings without materializing.
///
/// # Returns
/// - `Some(matches)` — binding-aware matching succeeded.  `matches` may be empty
///   (no rule matched → the expression is self-evaluating).
/// - `None` — cannot use binding-aware path (e.g. some candidate lacks a
///   structural matcher, or the head can't be resolved).  Caller should
///   fall back to `apply_bindings_generic + Eval`.
/// Resolve captured values in match_bindings through outer_bindings.
///
/// When `try_match_with_bindings` captures a sub-expression from the template
/// that contains variables (e.g., `(+ $x 1)` where `$x` is in outer_bindings),
/// the captured value still references those template variables. This function
/// applies `outer_bindings` to each captured value that `has_variables_fast()`,
/// producing concrete values for downstream evaluation.
///
/// Only values with variables are resolved — concrete captures (the common case)
/// pass through with zero allocation cost.
#[inline]
fn resolve_match_bindings_through<V, F>(
    match_bindings: &mut GenericBindings<V>,
    outer_bindings: &GenericBindings<V>,
    factory: &F,
)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    match match_bindings {
        GenericBindings::Empty => {}
        GenericBindings::Single((_, ref mut val)) => {
            if val.has_variables_fast() {
                *val = apply_bindings_generic(val, outer_bindings, factory);
            }
        }
        GenericBindings::Small(ref mut vec) => {
            for (_, val) in vec.iter_mut() {
                if val.has_variables_fast() {
                    *val = apply_bindings_generic(val, outer_bindings, factory);
                }
            }
        }
    }
}

pub fn try_match_rules_with_bindings<V, F>(
    template: &V,
    outer_bindings: &GenericBindings<V>,
    resolved_head: &str,
    arity: usize,
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Option<Vec<(V, GenericBindings<V>)>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    use crate::backend::environment::rule_management::get_first_arg_head;

    // Resolve the first argument's head through outer_bindings for second-level
    // index narrowing.  Template: (head $x ...) where $x may bind to (Foo ...).
    let first_arg_head: Option<&str> = if let Some(items) = template.as_sexpr() {
        if items.len() > 1 {
            let first_arg = &items[1];
            if let Some(var_name) = first_arg.as_atom() {
                if var_name.starts_with('$')
                    || (var_name.starts_with('&') && var_name != "&" && var_name != "&self")
                    || var_name.starts_with('\'')
                {
                    // Variable — resolve through bindings and extract head
                    outer_bindings.get(var_name).and_then(|v| get_first_arg_head(v))
                } else {
                    // Concrete atom used as first arg — its "head" for index purposes
                    // is itself (if it's an S-expr) or None (if it's a plain atom arg)
                    get_first_arg_head(first_arg)
                }
            } else {
                get_first_arg_head(first_arg)
            }
        } else {
            None
        }
    } else {
        None
    };

    // Read rule index and collect candidates
    let rule_index = env.shared.rule_index.read();
    let candidates: SmallVec<[&crate::backend::environment::rule_management::RuleEntry<V>; 16]> =
        rule_index.get_candidates(resolved_head, arity, first_arg_head).collect();

    if candidates.is_empty() {
        return Some(Vec::new()); // No candidates — self-evaluating
    }

    // ALL candidates must have structural matchers for binding-aware path.
    // If any lacks one, bail to materialization (MORK matching needs a concrete expr).
    if !candidates.iter().all(|e| e.structural_matcher.is_some()) {
        return None;
    }

    // Match each candidate's structural matcher against the template,
    // resolving variables through outer_bindings on the fly.
    let mut matches: Vec<(V, GenericBindings<V>)> = Vec::new();

    for entry in &candidates {
        let matcher = entry.structural_matcher.as_ref().expect("checked above");
        if let Some(mut match_bindings) = matcher.try_match_with_bindings(template, outer_bindings) {
            // Deep-resolve captured values through outer_bindings.
            //
            // navigate_resolving resolves variables at navigation boundaries but
            // NOT within captured S-expression sub-trees. For example, if the
            // template is (f (+ $x 1)) with outer_bindings {$x → 1} and pattern
            // (f $y), the VarBind captures (+ $x 1) with $x still present.
            // We must resolve $x → 1 within the captured value so that downstream
            // evaluation (via EvalWithBindings) sees (+ 1 1), not (+ $x 1).
            if !outer_bindings.is_empty() {
                resolve_match_bindings_through(&mut match_bindings, outer_bindings, factory);
            }

            let multiplicity = entry.multiplicity.max(1);
            if multiplicity == 1 {
                matches.push((entry.rhs.clone(), match_bindings));
            } else {
                for _ in 0..multiplicity {
                    matches.push((entry.rhs.clone(), match_bindings.clone()));
                }
            }
        }
    }

    Some(matches)
}


// ============================================================================
// Phase F: Tight Deterministic Eval Loop
// ============================================================================

use super::dispatch_hints::{
    operator_cache_get, is_normal_form_bounded, REDUCIBLE_HEADS,
};

/// Try to evaluate a deterministic chain of user-defined rule applications
/// without going through the full trampoline push/pop cycle.
///
/// Eligible when ALL of the following hold for each step:
/// 1. Head is a plain atom (not variable, not special form, not grounded op)
/// 2. Operator cache reports `all_structural` AND `candidate_count == 1`
/// 3. The single structural matcher succeeds
/// 4. Chain length bounded by `MAX_CHAIN_LENGTH` (prevents infinite loops)
///
/// Returns `Some(result_value)` if the chain produced a final value,
/// `None` if any step wasn't eligible (caller falls through to standard path).
///
/// # Performance
///
/// Each chain step costs ~100-200 ns (structural match + apply_bindings).
/// The trampoline alternative costs ~3-5 μs per step (push/pop work item,
/// dispatch, push continuation, collect results). For PLN deterministic
/// operators (kbstatic, kbdynamic, PLNcategorizeObject), this saves ~80%
/// of per-step overhead.
#[inline]
pub fn try_deterministic_chain<V, F>(
    expr: &V,
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Option<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    const MAX_CHAIN_LENGTH: usize = 64;

    // Only attempt for MettaValue (compile-time constant after monomorphization)
    if std::any::TypeId::of::<V>() != std::any::TypeId::of::<crate::backend::models::MettaValue>() {
        return None;
    }

    let items = expr.as_sexpr()?;
    if items.is_empty() { return None; }

    let head = items[0].as_atom()?;
    if head.starts_with('$') { return None; } // Variable head
    if REDUCIBLE_HEADS.contains(head) { return None; } // Special form / grounded op

    let arity = items.len() - 1;
    let cache_entry = operator_cache_get(head, arity)?;
    if !cache_entry.all_structural || cache_entry.candidate_count != 1 {
        return None;
    }

    // First step: structural match against the single candidate
    let mut current = try_deterministic_step(expr, head, arity, env, factory)?;

    // Chain subsequent steps
    for _ in 1..MAX_CHAIN_LENGTH {
        let next_items = match current.as_sexpr() {
            Some(items) if !items.is_empty() => items,
            _ => return Some(current), // Not an S-expr or empty → done
        };

        let next_head = match next_items[0].as_atom() {
            Some(h) => h,
            None => return Some(current), // Non-atom head → done
        };

        if next_head.starts_with('$') { return Some(current); }
        if REDUCIBLE_HEADS.contains(next_head) { return None; } // Need trampoline for special forms

        let next_arity = next_items.len() - 1;
        let next_cache = match operator_cache_get(next_head, next_arity) {
            Some(c) if c.all_structural && c.candidate_count == 1 => c,
            _ => return Some(current), // Non-deterministic or no cache → return current for trampoline
        };
        let _ = next_cache;

        match try_deterministic_step(&current, next_head, next_arity, env, factory) {
            Some(result) => current = result,
            None => return Some(current), // Match failed → return current for trampoline
        }
    }

    // Chain too long → bail, return current result for the trampoline to handle
    Some(current)
}

/// Execute a single deterministic step: structural match + apply bindings.
///
/// Reads the rule index to get the single candidate, runs its structural
/// matcher, and applies bindings if variables exist in the RHS.
#[inline]
fn try_deterministic_step<V, F>(
    expr: &V,
    head: &str,
    arity: usize,
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Option<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    use crate::backend::environment::rule_management::get_first_arg_head;

    let first_arg_head = get_first_arg_head(expr);
    let rule_index = env.shared.rule_index.read();
    let mut candidates = rule_index.get_candidates(head, arity, first_arg_head);

    let entry = candidates.next()?;
    // Verify it's actually a single candidate (no wildcard extras, etc.)
    if candidates.next().is_some() { return None; }

    let matcher = entry.structural_matcher.as_ref()?;
    let bindings = matcher.try_match(expr)?;

    let result = if entry.rhs_has_variables {
        apply_bindings_generic(&entry.rhs, &bindings, factory)
    } else {
        entry.rhs.clone()
    };

    Some(result)
}

// ============================================================================
// Stretch Goal 1: Binding-Aware Deterministic Chain
// ============================================================================
//
// Extends the deterministic chain to work inside EvalWithBindings. Instead of
// materializing the template + pushing Eval (which then decomposes, matches,
// dispatches — 4-5 trampoline iterations), this function:
//
// 1. Materializes the template once (needed for structural matching)
// 2. If the head is a deterministic single-rule operator, structural-matches
// 3. If the RHS has variables, composes bindings and chains into the next
//    EvalWithBindings — NO allocation for the substituted RHS tree
// 4. Repeats until non-deterministic or non-chainable
//
// Net savings: eliminates 2-3 trampoline iterations per deterministic chain step
// and avoids intermediate apply_bindings allocations for variable-containing RHS.

/// Attempt to chain deterministic rule applications from an EvalWithBindings context.
///
/// Given a `(template, bindings)` pair where the template is an S-expression:
/// 1. Materializes the expression via `apply_bindings_generic`
/// 2. Checks if the head is a deterministic single-rule operator
/// 3. If so, performs structural matching and chains into the next step
/// 4. Returns `Some((final_rhs_template, composed_bindings))` for further
///    EvalWithBindings dispatch, or `Some((materialized_value, EMPTY_BINDINGS))`
///    if the chain terminates in a concrete value.
/// 5. Returns `None` if the first step isn't chainable (caller falls through).
///
/// # Performance
///
/// For a deterministic chain of length N, this replaces N×(materialize + Eval +
/// eval_step + eval_sexpr_step + try_match_all_rules + dispatch_rule_matches)
/// with N×(materialize + structural_match) + 1×EvalWithBindings. Saves ~60%
/// of trampoline overhead for each chain step.
#[inline]
pub fn try_deferred_deterministic_chain<V, F>(
    template: &V,
    bindings: &GenericBindings<V>,
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Option<DeferredChainResult<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    const MAX_CHAIN_LENGTH: usize = 64;

    // Only attempt for MettaValue (compile-time constant after monomorphization)
    if std::any::TypeId::of::<V>() != std::any::TypeId::of::<crate::backend::models::MettaValue>() {
        return None;
    }

    // Template must be an S-expr with a resolvable head
    let items = template.as_sexpr()?;
    if items.is_empty() { return None; }

    // Resolve head through bindings if it's a variable
    let head_item = &items[0];
    let head = if let Some(var) = head_item.as_atom() {
        if var.starts_with('$') {
            bindings.get(var).and_then(|v| v.as_atom())?
        } else {
            var
        }
    } else {
        return None;
    };

    // Head must not be a special form or grounded op
    if head.starts_with('$') { return None; }
    if REDUCIBLE_HEADS.contains(head) { return None; }

    let arity = items.len() - 1;
    let cache_entry = operator_cache_get(head, arity)?;
    if !cache_entry.all_structural || cache_entry.candidate_count != 1 {
        return None;
    }

    // First step: materialize and match
    let materialized = apply_bindings_generic(template, bindings, factory);
    let (rhs_template, match_bindings) = try_deterministic_match(&materialized, head, arity, env)?;

    // If RHS has variables, we can defer materialization by composing bindings
    if rhs_template.has_variables_fast() {
        // Chain subsequent steps with composed bindings
        let mut current_template = rhs_template;
        let mut current_bindings = match_bindings;

        for _ in 1..MAX_CHAIN_LENGTH {
            // Check if current template is an S-expr with a chainable head
            let next_items = current_template.as_sexpr()?;
            if next_items.is_empty() { break; }

            // Resolve head through current bindings
            let next_head_item = &next_items[0];
            let next_head = if let Some(var) = next_head_item.as_atom() {
                if var.starts_with('$') {
                    match current_bindings.get(var).and_then(|v| v.as_atom()) {
                        Some(h) => h,
                        None => break,
                    }
                } else {
                    var
                }
            } else {
                break;
            };

            if next_head.starts_with('$') { break; }
            if REDUCIBLE_HEADS.contains(next_head) { break; }

            let next_arity = next_items.len() - 1;
            let next_cache = match operator_cache_get(next_head, next_arity) {
                Some(c) if c.all_structural && c.candidate_count == 1 => c,
                _ => break,
            };
            let _ = next_cache;

            // Materialize current template with current bindings for matching
            let next_materialized = apply_bindings_generic(&current_template, &current_bindings, factory);
            match try_deterministic_match(&next_materialized, next_head, next_arity, env) {
                Some((next_rhs, next_match_bindings)) => {
                    if next_rhs.has_variables_fast() {
                        current_template = next_rhs;
                        current_bindings = next_match_bindings;
                    } else {
                        // Ground RHS — check if normal form
                        if is_normal_form_bounded(&next_rhs, env, 2) {
                            return Some(DeferredChainResult::Done(next_rhs));
                        }
                        // Not normal form — return as concrete value for Eval
                        return Some(DeferredChainResult::Concrete(next_rhs));
                    }
                }
                None => break,
            }
        }

        return Some(DeferredChainResult::Deferred {
            template: current_template,
            bindings: current_bindings,
        });
    }

    // Ground RHS — check if we can chain further
    if is_normal_form_bounded(&rhs_template, env, 2) {
        return Some(DeferredChainResult::Done(rhs_template));
    }

    // Try chaining through the ground RHS
    match try_deterministic_chain(&rhs_template, env, factory) {
        Some(chained) => Some(DeferredChainResult::Concrete(chained)),
        None => Some(DeferredChainResult::Concrete(rhs_template)),
    }
}

/// Result of a deferred deterministic chain.
pub enum DeferredChainResult<V: MettaValueTrait + Clone> {
    /// Chain terminated with a deferred (template, bindings) pair.
    /// Push EvalWithBindings to continue evaluation.
    Deferred { template: V, bindings: GenericBindings<V> },
    /// Chain terminated with a concrete value needing further evaluation.
    /// Push Eval to continue.
    Concrete(V),
    /// Chain terminated with a normal-form value. Push Resume directly.
    Done(V),
}

/// Execute a single deterministic match step, returning the RHS template and bindings.
///
/// Unlike `try_deterministic_step` which applies bindings immediately, this
/// returns the raw (rhs_template, match_bindings) for deferred binding composition.
#[inline]
fn try_deterministic_match<V, F>(
    expr: &V,
    head: &str,
    arity: usize,
    env: &GenericEnvironment<V, F>,
) -> Option<(V, GenericBindings<V>)>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    use crate::backend::environment::rule_management::get_first_arg_head;

    let first_arg_head = get_first_arg_head(expr);
    let rule_index = env.shared.rule_index.read();
    let mut candidates = rule_index.get_candidates(head, arity, first_arg_head);

    let entry = candidates.next()?;
    if candidates.next().is_some() { return None; }

    let matcher = entry.structural_matcher.as_ref()?;
    let bindings = matcher.try_match(expr)?;

    Some((entry.rhs.clone(), bindings))
}

/// Check if success/failure bodies represent a simple boolean check pattern.
///
/// Returns true if (success_body, failure_body) match:
/// - (Bool(true), Bool(false))
/// - (Atom("True"), Atom("False"))
///
/// This is used to optimize unify operations that are existence checks.
#[inline]
pub fn is_boolean_check_pattern<V: MettaValueTrait>(success_body: &V, failure_body: &V) -> bool {
    // Check for Bool(true), Bool(false) pattern
    if let (Some(true), Some(false)) = (success_body.as_bool(), failure_body.as_bool()) {
        return true;
    }

    // Check for Atom("True"), Atom("False") pattern
    if let (Some(s), Some(f)) = (success_body.as_atom(), failure_body.as_atom()) {
        if s == "True" && f == "False" {
            return true;
        }
    }

    false
}

// ============================================================================
// Generic Switch/Case Evaluation
// ============================================================================

/// Result type for generic switch evaluation
pub enum GenericSwitchResult<V: MettaValueTrait + Clone> {
    /// Match found - return the instantiated template and bindings
    Match(V, GenericBindings<V>),
    /// No match found
    NoMatch,
    /// Error occurred
    Error(V),
}

/// Generic switch/case evaluation - works with any value type implementing MettaValueTrait.
///
/// This function evaluates a switch/case expression by matching the atom against
/// the pattern in each case, and returns the instantiated template if a match is found.
///
/// # Type Parameters
///
/// - `V`: The value type (must implement `MettaValueTrait + Clone`)
/// - `F`: The factory type (must implement `MettaValueFactory<V>`)
///
/// # Arguments
///
/// - `atom`: The value to match against patterns
/// - `cases`: The cases s-expression: ((pattern1 template1) (pattern2 template2) ...)
/// - `factory`: The factory for constructing new values
///
/// # Returns
///
/// - `GenericSwitchResult::Match(template, bindings)` if a pattern matches
/// - `GenericSwitchResult::NoMatch` if no pattern matches
/// - `GenericSwitchResult::Error(err)` if there's an error (malformed case)
///
/// # Performance
///
/// This generic version eliminates boundary conversions by operating directly
/// on the generic value type. Pattern matching uses `pattern_match_generic`
/// and binding application uses `apply_bindings_generic`, both of which operate
/// without type conversion.
pub fn eval_switch_generic<V, F>(atom: &V, cases: &V, factory: &F) -> GenericSwitchResult<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Cases must be an S-expression
    let Some(case_items) = cases.as_sexpr() else {
        let err = factory.error(
            &format!(
                "switch-minimal expects expression as second argument, got: {}",
                cases.friendly_type_name()
            ),
            cases.clone(),
        );
        return GenericSwitchResult::Error(err);
    };

    // No cases - return NoMatch (caller should handle as NotReducible)
    if case_items.is_empty() {
        return GenericSwitchResult::NoMatch;
    }

    // Iterate through cases looking for a match
    for case in case_items.iter() {
        // Each case must be an S-expression (pattern template)
        let Some(case_parts) = case.as_sexpr() else {
            let err = factory.error(
                "switch case should be an expression (pattern-template pair)",
                case.clone(),
            );
            return GenericSwitchResult::Error(err);
        };

        // Each case must have exactly 2 elements: pattern and template
        if case_parts.len() != 2 {
            let err = factory.error(
                &format!(
                    "switch case should be a pattern-template pair with exactly 2 elements, got {}. \
                    Usage: (switch expr (pattern1 result1) (pattern2 result2) ...)",
                    case_parts.len()
                ),
                case.clone(),
            );
            return GenericSwitchResult::Error(err);
        }

        let pattern = &case_parts[0];
        let template = &case_parts[1];

        // Try to match pattern against atom using generic pattern matching
        // NO CONVERSION NEEDED - operates directly on V
        if let Some(bindings) = pattern_match_generic(pattern, atom) {
            // Pattern matches - apply bindings to template
            // NO CONVERSION NEEDED - apply_bindings_generic operates on V
            let instantiated = apply_bindings_generic(template, &bindings, factory);
            return GenericSwitchResult::Match(instantiated, bindings);
        }
        // No match - continue to next case
    }

    // No case matched
    GenericSwitchResult::NoMatch
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::GcFactory;

    #[test]
    fn test_pattern_match_variable() {
        let pattern = MettaValue::Atom("$x".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.unwrap();
        assert_eq!(bindings.get("$x").map(|v| v.as_long()), Some(Some(42)));
    }

    #[test]
    fn test_pattern_match_wildcard() {
        let pattern = MettaValue::Atom("_".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
        assert!(bindings.unwrap().is_empty());
    }

    #[test]
    fn test_pattern_match_sexpr() {
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Long(42),
        ]);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.unwrap();
        assert_eq!(bindings.get("$x").map(|v| v.as_long()), Some(Some(42)));
    }

    #[test]
    fn test_apply_bindings_generic() {
        let factory = GcFactory::default();
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x", MettaValue::Long(42));

        let template = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);

        let result = apply_bindings_generic(&template, &bindings, &factory);
        assert!(result.is_sexpr());
        let items = result.as_sexpr().unwrap();
        assert_eq!(items[1].as_long(), Some(42));
    }

    // Tests for semantic alignment with heap pattern_match

    #[test]
    fn test_pattern_match_ampersand_not_variable() {
        // Standalone "&" should NOT be treated as a variable
        let pattern = MettaValue::Atom("&".to_string());
        let value = MettaValue::Long(42);
        // Should NOT match - "&" is a literal, not a variable
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_none());
    }

    #[test]
    fn test_pattern_match_ampersand_variable_prefix() {
        // "&foo" (variable starting with &) SHOULD be treated as a variable
        let pattern = MettaValue::Atom("&foo".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.unwrap();
        assert_eq!(bindings.get("&foo").map(|v| v.as_long()), Some(Some(42)));
    }

    #[test]
    fn test_pattern_match_unit_unit() {
        // Unit pattern matches Unit
        let pattern = MettaValue::Unit();
        let value = MettaValue::Unit();
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_unit_empty_sexpr() {
        // Unit pattern matches empty S-expression (SExpr([]) normalizes to Unit)
        let pattern = MettaValue::Unit();
        let value = MettaValue::SExpr(vec![]);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_empty_sexpr_unit() {
        // Empty S-expression pattern matches Unit (SExpr([]) normalizes to Unit)
        let pattern = MettaValue::SExpr(vec![]);
        let value = MettaValue::Unit();
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_unit_empty_atom() {
        // Unit pattern matches Atom("Empty")
        let pattern = MettaValue::Unit();
        let value = MettaValue::Atom("Empty".to_string());
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_empty_atom_unit() {
        // Atom("Empty") does NOT match Unit -- they are different values.
        let pattern = MettaValue::Atom("Empty".to_string());
        let value = MettaValue::Unit();
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_none());
    }

    #[test]
    fn test_pattern_match_float_direct_equality() {
        // Float comparison uses direct equality (not epsilon)
        let pattern = MettaValue::Float(1.0);
        let value = MettaValue::Float(1.0);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_some());

        // Different floats should not match
        let pattern = MettaValue::Float(1.0);
        let value = MettaValue::Float(1.0 + f64::EPSILON * 2.0);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(bindings.is_none());
    }

    #[test]
    fn test_apply_bindings_ampersand_not_variable() {
        // Standalone "&" should NOT be substituted as a variable
        let factory = GcFactory::default();
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("&", MettaValue::Long(42));

        let template = MettaValue::Atom("&".to_string());
        let result = apply_bindings_generic(&template, &bindings, &factory);
        // Should remain as "&", not substituted
        assert_eq!(result.as_atom(), Some("&"));
    }

    #[test]
    fn test_apply_bindings_type_no_recursion() {
        // Type variants should NOT have bindings applied to their contents
        let factory = GcFactory::default();
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x", MettaValue::Long(42));

        // Type wrapping a variable - should not substitute
        let template = MettaValue::Type(MettaValue::Atom("$x".to_string()));
        let result = apply_bindings_generic(&template, &bindings, &factory);

        // Result should still be a Type with $x inside (not substituted)
        assert!(result.is_type());
        let inner = result.as_type().expect("should be type");
        assert_eq!(inner.as_atom(), Some("$x"));
    }
}
