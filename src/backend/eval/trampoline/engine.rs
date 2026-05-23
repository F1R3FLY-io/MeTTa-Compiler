//! Monomorphized Evaluation Helpers for MettaValue
//!
//! This module provides helper functions for evaluation that operate on
//! concrete `MettaValue` and `GcFactory` types.
//!
//! MettaValue is Copy (8-byte tagged pointer), so clone() is a no-op memcpy.
//! GcFactory is Copy + Clone, used as a global allocator.
//!
//! ## Key Functions
//!
//! - `apply_bindings` - Apply bindings to a value (Copy-optimized)
//! - `pattern_match` - Pattern matching returning bindings
//! - `try_match_all_rules` - Match all rules against an expression
//! - `try_deterministic_chain` - Phase F tight deterministic eval loop
//! - `try_deferred_deterministic_chain` - Binding-aware deterministic chain
//! - `eval_switch` - Switch/case evaluation
//! - `is_boolean_check_pattern` - Detect boolean check optimization patterns

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use smallvec::SmallVec;

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{
    GcFactory, GenericBindings, MettaValue, MettaValueFactory, MettaValueTrait,
};

use super::dispatch_hints::{match_result_get, match_result_put};

/// Concrete type aliases.
pub type Environment = GenericEnvironment<MettaValue, GcFactory>;
pub type Bindings = GenericBindings<MettaValue>;
pub use super::types::{Continuation, EvalResult, WorkItem};

// ============================================================================
// Binding Application (Copy-optimized)
// ============================================================================

/// S0d.1: Apply class-aware bindings to a MettaValue.
///
/// Fast path: if the [`BindingsWithClasses`] has no class table, delegates
/// straight to the entries-only [`apply_bindings`]. Only the user-facing
/// `(unify ...)` form is expected to populate the class table.
///
/// When at least one class is present, lookup follows the
/// [`crate::backend::eval::bindings::apply_bindings_with_classes_generic`]
/// resolution order:
/// 1. Ordinary entry takes precedence (HE invariant: one slot per name).
/// 2. Class value: substituted in place.
/// 3. Value-less class member: returned as the ORIGINAL atom name
///    (preserves T03/004 strict alpha-distinct output).
/// 4. Unbound: returned as-is.
#[inline]
pub fn apply_bindings_with_classes(
    value: &MettaValue,
    bindings: &crate::backend::models::BindingsWithClasses<MettaValue>,
    factory: &GcFactory,
) -> MettaValue {
    crate::backend::eval::bindings::apply_bindings_with_classes_generic(value, bindings, factory)
}

/// Apply bindings to a MettaValue, substituting variables with bound values.
///
/// Monomorphized for MettaValue (Copy, 8-byte tagged pointer).
/// All `*value` dereferences are zero-cost copies.
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

/// Inner implementation after the top-level Spanned wrapper is peeled.
///
/// **Iterative trampoline / pushdown automaton.** Uses an explicit work stack
/// instead of Rust call-stack recursion. This guarantees:
///
/// 1. **No stack overflow on deep nesting**: an S-expression nested 10,000
///    levels deep walks the heap-allocated work_stack, not the Rust stack.
///
/// 2. **Transitive substitution**: when looking up a variable yields a bound
///    value that itself contains variables, those inner variables are also
///    resolved by the same single pass. The bound value is pushed back onto
///    the work stack as a `Process` item, so the loop re-enters the variable
///    branch on the next iteration. This is critical for the bidirectional
///    unification case where bindings look like:
///
///        {$B → (Inheritance $1 (IntSet cancerous)), $1 → Anna}
///
///    Substituting `$B` must yield `(Inheritance Anna (IntSet cancerous))`,
///    not the unreduced `(Inheritance $1 (IntSet cancerous))`.
///
/// **Cycle prevention**: `bidirectional_unify_generic` enforces an occurs
/// check at unification time, so cyclic bindings (e.g., `$a → (... $a ...)`)
/// can never enter the bindings map. Without cycles, the transitive walk is
/// bounded by the binding chain length and terminates.
///
/// **Lazy allocation preserved**: each `BuildSExpr` / `BuildConjunction` /
/// `BuildError` arm performs an identity-equality check against the original
/// value's children. If every child is unchanged, the original value is
/// reused verbatim — no allocation, matching the previous recursive
/// implementation's hot path.
fn apply_bindings_inner(
    value: &MettaValue,
    bindings: &Bindings,
    factory: &GcFactory,
) -> MettaValue {
    use crate::ir::Span;

    /// Work-stack item describing a pending operation.
    enum Work {
        /// Process a value: dispatch to the appropriate handler.
        Process(MettaValue),
        /// After processing `count` children, build a new S-expression.
        /// `original` is used for the identity-equality lazy-allocation check.
        BuildSExpr { count: usize, original: MettaValue },
        /// After processing `count` goals, build a new Conjunction.
        BuildConjunction { count: usize, original: MettaValue },
        /// After processing 2 children (offending, detail) in that order on
        /// result_stack, build a new Error. HE-bisimilar shape.
        BuildError { original: MettaValue },
        /// After processing 1 inner value, re-wrap with the saved Span.
        BuildSpanned { span: Span, original: MettaValue },
        /// After processing 1 inner value, re-wrap as `(quote inner)`.
        /// HE-faithful: `(quote $x)` with `$x → foo` becomes `(quote foo)`.
        BuildQuoted { original: MettaValue },
    }

    let mut work_stack: Vec<Work> = Vec::with_capacity(32);
    let mut result_stack: Vec<MettaValue> = Vec::with_capacity(32);

    work_stack.push(Work::Process(*value));

    while let Some(w) = work_stack.pop() {
        match w {
            Work::Process(v) => {
                // Handle Spanned wrapper around an inner value.
                // Peel one layer, process the inner value, then re-wrap.
                // This is iterative: we push BuildSpanned + Process(inner).
                if v.span().is_some() {
                    let (inner, span_opt) = v.peel_span();
                    if let Some(span) = span_opt {
                        work_stack.push(Work::BuildSpanned {
                            span: *span,
                            original: v,
                        });
                        work_stack.push(Work::Process(inner));
                        continue;
                    }
                    // Fallthrough: peel_span returned None for span (shouldn't
                    // happen since we just checked span().is_some(), but be safe).
                    result_stack.push(v);
                    continue;
                }

                // Atom (variable or non-variable)
                if let Some(var_name) = v.as_atom() {
                    if (var_name.starts_with('$')
                        || var_name.starts_with('&')
                        || var_name.starts_with('\''))
                        && var_name != "&"
                    {
                        if let Some(bound_value) = bindings.get(var_name) {
                            // Guard: if bound value is the same variable atom,
                            // emit directly to prevent infinite self-referential
                            // transitive resolution (e.g., $a → $a from
                            // call-site/rule variable name collision).
                            if bound_value.as_atom() == Some(var_name) {
                                result_stack.push(*bound_value);
                                continue;
                            }
                            // Transitive substitution: re-process the bound
                            // value via the work stack so any inner variables
                            // also get resolved. NO Rust call-stack growth.
                            //
                            // Note: HE's M-VAR-VAR-DISTINCT (spec §4.3.1) would
                            // return the ORIGINAL variable when the chain ends
                            // at an unbound variable. That requires an
                            // Equivalence variant on `Bindings` to distinguish
                            // unify-distinct (return original) from pattern-match
                            // var-var (substitute chain terminus). Deferred to S11.
                            work_stack.push(Work::Process(*bound_value));
                            continue;
                        }
                    }
                    result_stack.push(v);
                    continue;
                }

                // Fast path: no variables anywhere in this subtree.
                if !v.has_variables_fast() {
                    result_stack.push(v);
                    continue;
                }

                // Types are returned as-is (no substitution)
                if v.is_type() {
                    result_stack.push(v);
                    continue;
                }

                // S-expression: push BuildSExpr continuation, then push children
                // in reverse so they're processed left-to-right.
                if let Some(items) = v.as_sexpr() {
                    let count = items.len();
                    if count == 0 {
                        result_stack.push(v);
                        continue;
                    }
                    work_stack.push(Work::BuildSExpr { count, original: v });
                    for item in items.iter().rev() {
                        work_stack.push(Work::Process(*item));
                    }
                    continue;
                }

                // Conjunction: same pattern as S-expression.
                if let Some(goals) = v.as_conjunction() {
                    let count = goals.len();
                    if count == 0 {
                        result_stack.push(v);
                        continue;
                    }
                    work_stack.push(Work::BuildConjunction { count, original: v });
                    for goal in goals.iter().rev() {
                        work_stack.push(Work::Process(*goal));
                    }
                    continue;
                }

                // Error: process both offending and detail children, rebuild.
                if let Some((offending, detail)) = v.as_error() {
                    work_stack.push(Work::BuildError { original: v });
                    // Pop order is detail-first (top), then offending second.
                    // We want result_stack to contain [offending, detail] in
                    // that order, so push offending last (LIFO).
                    work_stack.push(Work::Process(detail));
                    work_stack.push(Work::Process(offending));
                    continue;
                }

                // Quoted: substitute bindings inside the quoted body, then
                // rebuild the wrapper. HE-faithful (T06/129 noreduce-eq):
                // `(quote $x)` with `$x → foo` becomes `(quote foo)`.
                if let Some(inner) = v.as_quoted() {
                    work_stack.push(Work::BuildQuoted { original: v });
                    work_stack.push(Work::Process(inner));
                    continue;
                }

                // Ground / unknown values: return as-is.
                result_stack.push(v);
            }
            Work::BuildSExpr { count, original } => {
                let start = result_stack.len() - count;
                // Identity-equality lazy-allocation check: if every new child
                // is the same MettaValue as the corresponding original child,
                // reuse the original verbatim (no allocation).
                let items = original
                    .as_sexpr()
                    .expect("BuildSExpr original must be sexpr");
                debug_assert_eq!(items.len(), count);
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&items[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                } else {
                    let new_val = factory.sexpr_from_slice(&result_stack[start..]);
                    result_stack.truncate(start);
                    result_stack.push(new_val);
                }
            }
            Work::BuildConjunction { count, original } => {
                let start = result_stack.len() - count;
                let goals = original
                    .as_conjunction()
                    .expect("BuildConjunction original must be conjunction");
                debug_assert_eq!(goals.len(), count);
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&goals[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                } else {
                    let new_val = factory.conjunction_from_slice(&result_stack[start..]);
                    result_stack.truncate(start);
                    result_stack.push(new_val);
                }
            }
            Work::BuildError { original } => {
                let new_detail = result_stack.pop().expect("BuildError needs detail");
                let new_offending = result_stack.pop().expect("BuildError needs offending");
                let (orig_offending, orig_detail) = original
                    .as_error()
                    .expect("BuildError original must be error");
                if new_offending.identity_eq(&orig_offending)
                    && new_detail.identity_eq(&orig_detail)
                {
                    result_stack.push(original);
                } else {
                    result_stack.push(factory.error( new_detail,new_offending));
                }
            }
            Work::BuildSpanned { span, original } => {
                let inner = result_stack.pop().expect("BuildSpanned needs inner");
                // Match the existing top-level peel logic at engine.rs:53-55:
                // if the result already carries a span, don't double-wrap.
                if inner.span().is_some() {
                    result_stack.push(inner);
                } else if inner.identity_eq(&original.peel_span().0) {
                    // Inner unchanged: reuse original Spanned wrapper.
                    result_stack.push(original);
                } else {
                    result_stack.push(factory.spanned(inner, span));
                }
            }
            Work::BuildQuoted { original } => {
                let new_inner = result_stack.pop().expect("BuildQuoted needs inner");
                let original_inner = original
                    .as_quoted()
                    .expect("BuildQuoted original must be Quoted");
                if new_inner.identity_eq(&original_inner) {
                    result_stack.push(original);
                } else {
                    result_stack.push(factory.quote(new_inner));
                }
            }
        }
    }

    debug_assert_eq!(
        result_stack.len(),
        1,
        "result stack should have exactly 1 value"
    );
    result_stack
        .pop()
        .expect("apply_bindings_inner: result stack empty")
}

// ============================================================================
// Pattern Matching
// ============================================================================

/// Pattern match two values.
///
/// Returns bindings if the pattern matches the value, None otherwise.
///
/// This implementation matches the heap-based `pattern_match` in `pattern.rs`,
/// including cross-type matching for Nil/Unit/Empty and the "&" exclusion.
///
/// `MettaValue` is `Copy` (8-byte tagged pointer), so cloning is free.
pub fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
    // Helper to check if a name is a variable.
    // IMPORTANT: standalone "&" is a literal operator (used in match), not a variable.
    // `$_` is the wildcard, not a variable.
    fn is_variable(name: &str) -> bool {
        (name.starts_with('$') || name.starts_with('&') || name.starts_with('\''))
            && name != "&"
            && name != "$_"
    }

    // Helper to check if a name is a wildcard.
    // Both `_` and `$_` are wildcards — each occurrence matches anything
    // without producing a binding.
    fn is_wildcard(name: &str) -> bool {
        name == "_" || name == "$_"
    }

    // Handle pattern atom
    if let Some(pattern_name) = pattern.as_atom() {
        // Wildcard matches anything
        if is_wildcard(pattern_name) {
            return Some(Bindings::new());
        }

        // Variable binds to value (excluding standalone "&")
        if is_variable(pattern_name) {
            let mut bindings = Bindings::new();
            // MettaValue is Copy — O(1) bitwise copy
            bindings.insert(pattern_name, *value);
            return Some(bindings);
        }

        // Empty atom pattern matches Empty sentinel
        if pattern_name == "Empty" && value.is_empty() {
            return Some(Bindings::new());
        }

        // Non-variable atom must match exactly
        if let Some(value_name) = value.as_atom() {
            if pattern_name == value_name {
                return Some(Bindings::new());
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
                    return Some(Bindings::new());
                }
            }
            // Empty S-expr matches Unit
            if value.is_unit() {
                return Some(Bindings::new());
            }
            // Empty S-expr matches Atom("Empty")
            if let Some(name) = value.as_atom() {
                if name == "Empty" {
                    return Some(Bindings::new());
                }
            }
            return None;
        }

        if let Some(value_items) = value.as_sexpr() {
            if pattern_items.len() != value_items.len() {
                return None;
            }

            let mut combined_bindings = Bindings::new();
            for (p, v) in pattern_items.iter().zip(value_items.iter()) {
                match pattern_match(p, v) {
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

        // S3 (ERROR-MATCH cross-shape): HE represents errors as the 3-element
        // `(Error <offending> <detail>)` SExpr; MeTTaTron stores them as a
        // dedicated `Error(offending, detail)` variant. When the value is an
        // Error variant, project it into the pseudo-SExpr shape and recurse.
        // See hyperon-experimental/lib/src/metta/mod.rs:54-74 and
        // metta-specification/spec/C-errors.md:7-14 for the HE shape.
        if let Some((value_off, value_detail)) = value.as_error() {
            if pattern_items.len() == 3 && pattern_items[0].as_atom() == Some("Error") {
                let mut combined = Bindings::new();
                if let Some(sub) = pattern_match(&pattern_items[1], &value_off) {
                    if !combined.merge(&sub) {
                        return None;
                    }
                } else {
                    return None;
                }
                if let Some(sub) = pattern_match(&pattern_items[2], &value_detail) {
                    if !combined.merge(&sub) {
                        return None;
                    }
                } else {
                    return None;
                }
                return Some(combined);
            }
            return None;
        }

        return None;
    }

    // Handle Conjunction pattern
    if let Some(pattern_goals) = pattern.as_conjunction() {
        if let Some(value_goals) = value.as_conjunction() {
            if pattern_goals.len() != value_goals.len() {
                return None;
            }

            let mut combined_bindings = Bindings::new();
            for (p, v) in pattern_goals.iter().zip(value_goals.iter()) {
                match pattern_match(p, v) {
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

    // Handle Error pattern.
    // HE-bisimilar: match both offending and detail sub-values structurally.
    // NOTE: inherent as_error() returns (offending, detail) as MettaValue by
    // value (Copy).
    if let Some((pattern_offending, pattern_detail)) = pattern.as_error() {
        if let Some((value_offending, value_detail)) = value.as_error() {
            let mut combined = Bindings::new();
            match pattern_match(&pattern_offending, &value_offending) {
                Some(sub) => {
                    if !combined.merge(&sub) {
                        return None;
                    }
                }
                None => return None,
            }
            match pattern_match(&pattern_detail, &value_detail) {
                Some(sub) => {
                    if !combined.merge(&sub) {
                        return None;
                    }
                }
                None => return None,
            }
            return Some(combined);
        }
        // S3 symmetric cross-shape: Error-variant pattern vs SExpr-shaped
        // value `(Error <offending> <detail>)`. Project SExpr into variant.
        if let Some(value_items) = value.as_sexpr() {
            if value_items.len() == 3 && value_items[0].as_atom() == Some("Error") {
                let mut combined = Bindings::new();
                if let Some(sub) = pattern_match(&pattern_offending, &value_items[1]) {
                    if !combined.merge(&sub) {
                        return None;
                    }
                } else {
                    return None;
                }
                if let Some(sub) = pattern_match(&pattern_detail, &value_items[2]) {
                    if !combined.merge(&sub) {
                        return None;
                    }
                } else {
                    return None;
                }
                return Some(combined);
            }
        }
        return None;
    }

    // Handle ground types - must match exactly (use direct equality, not epsilon)
    if let (Some(p), Some(v)) = (pattern.as_bool(), value.as_bool()) {
        return if p == v { Some(Bindings::new()) } else { None };
    }

    if let (Some(p), Some(v)) = (pattern.as_long(), value.as_long()) {
        return if p == v { Some(Bindings::new()) } else { None };
    }

    // Float comparison uses direct equality (matching heap behavior)
    if let (Some(p), Some(v)) = (pattern.as_float(), value.as_float()) {
        return if p == v { Some(Bindings::new()) } else { None };
    }

    if let (Some(p), Some(v)) = (pattern.as_string(), value.as_string()) {
        return if p == v { Some(Bindings::new()) } else { None };
    }

    // Unit pattern matches Unit and empty S-expr
    if pattern.is_unit() {
        if value.is_unit() {
            return Some(Bindings::new());
        }
        if let Some(items) = value.as_sexpr() {
            if items.is_empty() {
                return Some(Bindings::new());
            }
        }
        if let Some(name) = value.as_atom() {
            if name == "Empty" {
                return Some(Bindings::new());
            }
        }
        return None;
    }

    // Empty matches empty
    if pattern.is_empty() && value.is_empty() {
        return Some(Bindings::new());
    }

    None
}

// ============================================================================
// Rule Matching
// ============================================================================

// DEAD CODE: pattern_specificity_generic was removed because MeTTa HE has no
// specificity filter — all matching rules fire nondeterministically. The specificity
// filter in rule_management.rs was the only consumer, and it has been removed.
// The function incorrectly dropped structurally-more-specific rules when a variable-only
// rule happened to have fewer NewVar tags (e.g. PLN's `(f ($c $tv) $y)` with 3 vars
// beat `(f ((Implication $A $B) $TV) $Y)` with 4 vars despite the latter being more
// specific due to the `(Implication ...)` constructor constraint).

/// Try to match all rules against an expression.
///
/// Retrieves rules from the environment using `get_matching_rules_for_expr`
/// and performs pattern matching without any value type conversions.
///
/// # Returns
///
/// A vector of (rhs, bindings) pairs for all matching rules, sorted by specificity
/// and expanded by rule multiplicity.
/// Phase 8.7: Return type includes `rhs_type` for branch pruning.
/// The third element is the cached RHS type from the rule entry (if available).
pub fn try_match_all_rules(
    expr: &MettaValue,
    env: &Environment,
    _factory: GcFactory,
) -> Vec<(MettaValue, Bindings, Option<MettaValue>)> {
    // Default entry — no caller-side outer bindings available. Forwards
    // to the with_outer variant with empty outer_carrying.
    let empty_outer = Bindings::new();
    try_match_all_rules_with_outer(expr, env, _factory, &empty_outer)
}

/// Phase 5 (Bug 1): Variant of `try_match_all_rules` that threads the
/// caller's ambient bindings into `match_rules_native`. The `outer_carrying`
/// is consulted by `apply_bindings_with_rename_scoped` to resolve caller-
/// side variables that appear inside captured rule body values (e.g.
/// `$C`'s value `(uncle $a $b)` substituted into the `=>` rule body),
/// preventing those variables from being freshened to `$__fr_*` and
/// producing wildcard-LHS rules on `add-atom`.
pub fn try_match_all_rules_with_outer(
    expr: &MettaValue,
    env: &Environment,
    _factory: GcFactory,
    outer_carrying: &Bindings,
) -> Vec<(MettaValue, Bindings, Option<MettaValue>)> {
    let expr_arity = expr.get_arity();

    // Phase E: For single-candidate all-structural operators, skip hash computation
    // and match_result_cache entirely. The cache rarely hits for these (different args
    // each call), so the hash overhead (~700ns) exceeds any cache benefit.
    if let Some(head) = expr
        .as_sexpr()
        .and_then(|items| items.first())
        .and_then(|h| h.as_atom())
    {
        if let Some(cache_entry) = operator_cache_get(head, expr_arity) {
            if cache_entry.all_structural && cache_entry.candidate_count == 1 {
                // Fast path: skip hash, skip match_result_cache, go straight to structural match.
                // Phase 3.2-A: the trampoline downstream applies bindings to
                // the template via `EvalWithBindings`. Under per-match
                // freshening, `match_rules_native` has ALREADY substituted
                // bindings into a per-match-freshened RHS (stored in
                // `instantiated_rhs`). Returning `(instantiated_rhs,
                // empty_bindings)` lets the downstream apply-bindings
                // pass be a no-op — preserving the per-match freshened
                // names (essential for branch isolation) without double-
                // substituting against original-keyed bindings.
                let results = env.match_rules_native(
                    expr,
                    |v: &MettaValue, _: &Bindings, _: &GcFactory| *v,
                    outer_carrying,
                );
                return results
                    .into_iter()
                    .map(|r| (r.instantiated_rhs, Bindings::new(), r.rhs_type))
                    .collect();
            }
        }
    }

    // Standard path: compute hash and use match_result_cache. Skip the
    // cache when outer_carrying is non-empty — cached results were
    // computed without caller-side resolution, so they may contain
    // freshened variables that the threaded path would have resolved.
    let expr_hash = expr.hash_value();

    if outer_carrying.is_empty() {
        if let Some(cached) = match_result_get(expr_hash, expr_arity) {
            return cached;
        }
    }

    // Use native byte-level matching via RuleIndex + extract_data.
    // match_rules_native also populates the operator cache for Phase E.
    let results = env.match_rules_native(
        expr,
        |v: &MettaValue, _: &Bindings, _: &GcFactory| *v,
        outer_carrying,
    );
    let result_vec: Vec<(MettaValue, Bindings, Option<MettaValue>)> = results
        .into_iter()
        .map(|r| (r.instantiated_rhs, Bindings::new(), r.rhs_type))
        .collect();

    // Store in match result cache only when outer_carrying was empty —
    // otherwise the result is conditional on the caller's bindings and
    // cannot be reused for queries with different (or empty) outer.
    if outer_carrying.is_empty() && !result_vec.is_empty() {
        match_result_put(expr_hash, expr_arity, &result_vec);
    }

    result_vec
}

/// Detailed result from bidirectional rule unification.
#[derive(Debug, Clone)]
pub struct UnificationRuleMatch {
    /// RHS with match bindings already applied.
    pub instantiated_rhs: MettaValue,
    /// Caller-visible bindings that may escape as branch provenance.
    pub exported_bindings: Bindings,
    /// Rule-local bindings keyed by original rule variable names for compiled RHS frames.
    pub original_bindings: Bindings,
    /// Per-dispatch scope used while instantiating this RHS.
    pub rule_scope: crate::backend::models::generic_bindings::ScopeId,
    /// Cached RHS type from the matching rule entry.
    pub rhs_type: Option<MettaValue>,
}

/// Enumerate rule matches via bidirectional unification.
pub fn enumerate_rules_via_unification(
    query: &MettaValue,
    env: &Environment,
    factory: &GcFactory,
) -> Vec<(MettaValue, Bindings, Option<MettaValue>)> {
    enumerate_rules_via_unification_detailed(query, env, factory)
        .into_iter()
        .map(|m| (m.instantiated_rhs, m.exported_bindings, m.rhs_type))
        .collect()
}

/// Enumerate rule matches via bidirectional unification, preserving VM/JIT
/// frame bindings separately from caller-visible branch bindings.
///
/// This is the HE-conformant fallback used when `try_match_all_rules` returns
/// no matches AND the query expression contains free variables. It iterates
/// over all candidate rules in the rule index for the query's `(head, arity)`
/// and runs `bidirectional_unify_generic` against each candidate's LHS. On
/// success, the candidate's RHS is instantiated with the unified bindings.
///
/// Why this is needed: `try_match_all_rules` uses `StructuralMatcher` which
/// performs literal atom comparison. When the query has a free variable in a
/// position where the rule LHS has a concrete atom (e.g. query `(father a $b)`
/// vs rule `(father a b)`), the literal check fails because `"$b" != "b"`.
/// PeTTa / MeTTa HE handle this case via Prolog-style unification —
/// the query variable `$b` should bind to `b` and the rule's RHS should be
/// produced as a result. This function provides exactly that semantic.
///
/// The returned `exported_bindings` include only caller/query variables.
/// Internal rule-frame bindings are retained separately in `original_bindings`
/// for compiled RHS execution.
///
/// Returns an empty vec when no rules match, never `None`. Caller decides
/// whether the result is meaningful (empty → fall through to data
/// constructor / tuple path).
pub fn enumerate_rules_via_unification_detailed(
    query: &MettaValue,
    env: &Environment,
    factory: &GcFactory,
) -> Vec<UnificationRuleMatch> {
    use crate::backend::environment::rule_management::get_first_arg_head;
    use crate::backend::eval::bindings::{
        bidirectional_unify_generic, export_query_bindings_generic,
    };
    use crate::backend::eval::cesk::continuation_compression::is_rule_live;

    let head = match query.get_head_symbol() {
        Some(h) => h,
        None => return Vec::new(),
    };
    let arity = query.get_arity();
    let first_arg_head = get_first_arg_head(query);

    let rule_index = env.shared.rule_index.read();
    let candidates = rule_index.get_candidates_filtered(head, arity, first_arg_head, query);

    let mut out: Vec<UnificationRuleMatch> = Vec::with_capacity(candidates.len());

    for entry in candidates.iter() {
        if !is_rule_live(entry.global_rule_index) {
            continue;
        }
        // P2 simultaneous flip with bidirectional unify.
        //
        // bidirectional_unify is name-equality based and would conflict if
        // rule and query share a variable name, so we still freshen the
        // LHS *internally* (the freshened LHS is consumed by unify, not
        // propagated). The freshening prefix `$__fr_{epoch}_` is stable
        // per epoch, so we can identify rule-side keys on the resulting
        // bindings by prefix-match and retag them to `dispatch_scope`.
        // Query-side keys (no prefix) stay at ROOT_SCOPE.
        use crate::backend::eval::freshening::{allocate_epoch, freshen_variables_with_epoch};
        use crate::backend::models::generic_bindings::{allocate_scope_id, ROOT_SCOPE};
        let epoch = allocate_epoch();
        let lhs_freshened = freshen_variables_with_epoch(&entry.lhs, epoch, factory);
        if let Some(bindings) = bidirectional_unify_generic(&lhs_freshened, query) {
            let dispatch_scope = allocate_scope_id();
            let prefix = format!("$__fr_{}_", epoch);

            // Partial-unification guard.
            //
            // If `bidirectional_unify` produced a binding `(query_var → value)`
            // whose value still references a freshened rule-side variable
            // (`$__fr_{epoch}_*`), the rule match is "partial": the rule LHS
            // could not bottom out against concrete query structure. Applying
            // the rule body would produce an instantiated_rhs with body-local
            // fresh vars left free, which would re-trigger non-deterministic
            // rule lookup at the next recursion step and never terminate.
            //
            // Concretely: `(append $unbound (Cons "x" Nil))` against rule LHS
            // `(append (Cons $head $tail) $list)` produces
            // `{$unbound → (Cons $__fr_E_head $__fr_E_tail), $list → (Cons "x" Nil)}`.
            // The body `(Cons $head (append $tail $list))` references
            // `$__fr_E_tail`, which is unbound after substitution — re-running
            // the recursive append rule with this unbound tail diverges.
            //
            // Skipping such rules restricts free-variable unification matches
            // to those where the rule LHS structure can be FULLY unified
            // against concrete query parts (i.e., the rule terminates in one
            // step). This matches HE / spec semantics for non-ground queries
            // against recursive functions: the Nil base case must apply, the
            // recursive case is only viable when the query's first-arg is a
            // concrete `(Cons head tail)` cell.
            use crate::backend::eval::bindings::value_contains_var_with_prefix;
            let has_partial_binding = bindings.iter_full().any(|(_, name, val)| {
                !name.starts_with(&prefix) && value_contains_var_with_prefix(val, &prefix)
            });
            if has_partial_binding {
                continue;
            }

            let mut scoped_bindings = crate::backend::models::GenericBindings::new();
            for (s, name, val) in bindings.iter_full() {
                let target = if name.starts_with(&prefix) {
                    dispatch_scope
                } else {
                    s
                };
                scoped_bindings.insert_scoped(target, name, val.clone());
            }

            let mut original_bindings = Bindings::new();
            for (s, name, val) in scoped_bindings.iter_full() {
                if let Some(bare) = name.strip_prefix(&prefix) {
                    original_bindings.insert_or_replace(format!("${bare}"), val.clone());
                } else if s == ROOT_SCOPE {
                    original_bindings.insert_or_replace(name, val.clone());
                }
            }

            let Some(exported_bindings) = export_query_bindings_generic(
                &scoped_bindings,
                query,
                &prefix,
                &[dispatch_scope, ROOT_SCOPE],
                factory,
            ) else {
                continue;
            };

            let instantiated = if entry.rhs_has_variables {
                let rhs_freshened = freshen_variables_with_epoch(&entry.rhs, epoch, factory);
                // Phase 1: outer_carrying empty here — `enumerate_rules_via_unification`
                // takes no outer-bindings parameter. Caller-side variable
                // resolution flows in via `scoped_bindings` (ROOT_SCOPE
                // entries from bidirectional_unify), which the lookup arm
                // already consults.
                crate::backend::eval::bindings::apply_bindings_with_rename_scoped(
                    &rhs_freshened,
                    &scoped_bindings,
                    &[dispatch_scope, ROOT_SCOPE],
                    dispatch_scope,
                    &crate::backend::models::GenericBindings::new(),
                    factory,
                )
            } else {
                entry.rhs.clone()
            };
            out.push(UnificationRuleMatch {
                instantiated_rhs: instantiated,
                exported_bindings,
                original_bindings,
                rule_scope: dispatch_scope,
                rhs_type: entry.rhs_type.clone(),
            });
        }
    }

    out
}

// ============================================================================
// SG1: Binding-Aware Rule Matching (WAM-style lazy variable resolution)
// ============================================================================
//
// Instead of materializing `apply_bindings(template, outer_bindings)` and
// then calling `try_match_all_rules` on the result, this function
// matches rules directly against the unresolved template by resolving
// variables on-the-fly through `outer_bindings` inside the structural matcher.
//
// This eliminates the O(tree) allocation from `apply_bindings` for
// the common case where all rule candidates have structural matchers.
//
// Returns `Some(matches)` if binding-aware matching was possible (even if
// no rules matched — empty vec means "no match, self-evaluate").
// Returns `None` if binding-aware matching was not possible (caller must
// fall back to materialization).

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
fn resolve_match_bindings_through(
    match_bindings: &mut Bindings,
    outer_bindings: &Bindings,
    factory: &GcFactory,
) {
    match match_bindings {
        GenericBindings::Empty => {}
        GenericBindings::Single((_, _, ref mut val)) => {
            if val.has_variables_fast() {
                *val = apply_bindings(val, outer_bindings, factory);
            }
        }
        GenericBindings::Small(ref mut vec) => {
            for (_, _, val) in vec.iter_mut() {
                if val.has_variables_fast() {
                    *val = apply_bindings(val, outer_bindings, factory);
                }
            }
        }
    }
}

/// Try to match rules against a template + bindings without materializing.
///
/// # Returns
/// - `Some(matches)` — binding-aware matching succeeded.  `matches` may be empty
///   (no rule matched — the expression is self-evaluating).
/// - `None` — cannot use binding-aware path (e.g. some candidate lacks a
///   structural matcher, or the head can't be resolved).  Caller should
///   fall back to `apply_bindings + Eval`.
pub fn try_match_rules_with_bindings(
    template: &MettaValue,
    outer_bindings: &Bindings,
    resolved_head: &str,
    arity: usize,
    env: &Environment,
    factory: &GcFactory,
) -> Option<Vec<(MettaValue, Bindings)>> {
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
                    outer_bindings
                        .get(var_name)
                        .and_then(|v| get_first_arg_head(v))
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

    // Read rule index and collect candidates with disc tree pruning applied.
    // get_candidates_filtered applies disc tree to group entries only,
    // always including wildcard rules (which are not in any group's disc tree).
    let rule_index = env.shared.rule_index.read();
    let candidates: SmallVec<
        [&crate::backend::environment::rule_management::RuleEntry<MettaValue>; 16],
    > = rule_index.get_candidates_filtered(resolved_head, arity, first_arg_head, template);

    if candidates.is_empty() {
        return Some(Vec::new()); // No candidates — self-evaluating
    }

    // ALL candidates must have a compiled matcher (structural or enhanced)
    // for the binding-aware path. If any lacks both, bail to materialization.
    if !candidates
        .iter()
        .all(|e| e.structural_matcher.is_some() || e.enhanced_matcher.is_some())
    {
        return None;
    }

    // Match each candidate against the template + outer_bindings without
    // materializing. Both StructuralMatcher and EnhancedMatcher support
    // try_match_with_bindings, resolving variables on the fly.
    let mut matches: Vec<(MettaValue, Bindings)> = Vec::new();

    // eval-trace filter check: cheap atomic load when the env var is unset.
    // When admitted, every (call_site, candidate) pair emits a
    // `RuleMatchAttempt` event so the analyzer can answer "why did rule R
    // not match at this call site?".
    #[cfg(feature = "trace")]
    let trace_match_attempts = crate::backend::trace::rule_match::should_trace_match(resolved_head);

    for (rule_idx, entry) in candidates.iter().enumerate() {
        let _rule_idx_u32 = rule_idx as u32;
        let match_result = if let Some(ref matcher) = entry.structural_matcher {
            matcher.try_match_with_bindings(template, outer_bindings)
        } else if let Some(ref matcher) = entry.enhanced_matcher {
            matcher.try_match_with_bindings(template, outer_bindings)
        } else {
            None
        };

        // eval-trace: emit a RuleMatchAttempt event for this candidate.
        // Cheap when the filter is disabled (single atomic load + None
        // check above). When enabled, the event captures the call site,
        // rule LHS, and outcome (Success or generic failure).
        #[cfg(feature = "trace")]
        if trace_match_attempts {
            let outcome = if let Some(ref b) = match_result {
                trace_format::RuleMatchOutcome::Success {
                    bindings: b
                        .iter()
                        .map(|(k, v)| {
                            (k.to_string(), crate::backend::trace::trace_value_generic(v))
                        })
                        .collect(),
                }
            } else {
                // Without a `try_match_with_bindings_with_detail` variant we
                // can't pinpoint *which* check failed; emit a generic
                // path-navigate-failed outcome with an empty path. The next
                // refinement is to thread detailed failures through this
                // call site as well.
                trace_format::RuleMatchOutcome::PathNavigateFailed {
                    path: Vec::new(),
                    var: None,
                }
            };
            crate::backend::trace::rule_match::emit_outcome::<MettaValue>(
                "structural-with-bindings",
                resolved_head,
                arity as u32,
                template,
                &entry.lhs,
                None,
                _rule_idx_u32,
                outcome,
                None,
                0,
            );
        }

        if let Some(mut match_bindings) = match_result {
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

            // Per-invocation scope-tagged binding substitution.
            //
            // Each dispatch allocates a fresh `dispatch_scope` so recursive
            // invocations of the same rule use disjoint binding namespaces
            // (mirrors HE's `CachingMapper` per-query freshening, but at the
            // bindings layer instead of allocating new RHS atoms). We:
            //   1. Retag the matcher's `match_bindings` from `ROOT_SCOPE` to
            //      the new `dispatch_scope`. Caller-side bindings (e.g. query
            //      vars `$who`) remain at `ROOT_SCOPE` if any; the matcher
            //      typically only emits rule-LHS bindings at this point.
            //   2. Walk `entry.rhs` (the env-resident original template — no
            //      slab allocation for variable atoms) with the chain
            //      `[dispatch_scope, ROOT_SCOPE]`. Rule-LHS-bound atoms hit
            //      `dispatch_scope`; caller-level atoms embedded via
            //      bidirectional unify miss `dispatch_scope` and fall back
            //      to `ROOT_SCOPE`.
            //   3. Emit `(substituted_rhs, empty)`. Empty matches the prior
            //      Edit A invariant: substitution is baked into the RHS, so
            //      downstream `compose_outer_inner_strict_generic` cannot
            //      produce a same-key conflict between distinct invocations.
            //
            // Net effect vs. Edit A: identical observed result, zero atom
            // allocations per match (substituted values are copies of bound
            // pointers, not freshly-renamed atoms). This is the change that
            // restores GC quiescence for Robot.metta — no transient atoms
            // per rule dispatch means no stack-only roots churning the
            // allocator past the cron's `gc_threshold`.
            // P2 simultaneous flip: scope-tagged bindings + body-local
            // rename. The matcher emits bindings keyed on `entry.var_names`
            // (rule-side, original LHS names); we retag those into a fresh
            // `dispatch_scope`. Caller-side keys (introduced by the
            // bidirectional-unify fallback in EnhancedMatcher slot equality
            // checks at `enhanced_matcher.rs:398-407`) stay at `ROOT_SCOPE`.
            // `apply_bindings_with_rename_scoped` walks `entry.rhs` with
            // chain `[dispatch_scope, ROOT_SCOPE]`: rule-LHS atoms hit
            // `dispatch_scope`; body-local atoms (let*-pattern intros)
            // are renamed using `dispatch_scope` as the epoch.
            //
            // Empty `out_bindings` matches the prior Edit-A invariant:
            // substitution is baked into the RHS, downstream compose sees
            // empty inner — no same-key conflict between distinct invocations.
            // Option D: HE-style stored-side rename then scope tag.
            use crate::backend::eval::freshening::{
                allocate_epoch, freshen_bindings_keys_with_epoch, freshen_variables_with_epoch,
                intern_fresh_name,
            };
            use crate::backend::models::generic_bindings::{allocate_scope_id, ROOT_SCOPE};
            let body_local_epoch = allocate_epoch();
            let dispatch_scope = allocate_scope_id();
            let prefix = format!("$__fr_{}_", body_local_epoch);
            let match_bindings = freshen_bindings_keys_with_epoch(
                match_bindings,
                body_local_epoch,
                &entry.var_names,
            );
            let renamed_var_names: Vec<&'static str> = entry
                .var_names
                .iter()
                .map(|n| intern_fresh_name(body_local_epoch, &n[1..]))
                .collect();
            let scoped_bindings = crate::backend::eval::bindings::retag_rule_keys_at_scope(
                match_bindings,
                &renamed_var_names,
                ROOT_SCOPE,
                dispatch_scope,
            );
            let Some(out_bindings) = crate::backend::eval::bindings::export_query_bindings_generic(
                &scoped_bindings,
                template,
                &prefix,
                &[dispatch_scope, ROOT_SCOPE],
                factory,
            ) else {
                continue;
            };
            let out_rhs = if entry.rhs_has_variables {
                let rhs_freshened =
                    freshen_variables_with_epoch(&entry.rhs, body_local_epoch, factory);
                // Phase 1 (Bug 1): caller-side variables in captured values
                // (e.g. `$C`'s value `(uncle $a $b)`) must resolve through
                // outer_bindings BEFORE the body-local rename freshens them
                // and produces a wildcard-LHS rule on add-atom.
                let instantiated_rhs =
                    crate::backend::eval::bindings::apply_bindings_with_rename_scoped(
                        &rhs_freshened,
                        &scoped_bindings,
                        &[dispatch_scope, ROOT_SCOPE],
                        dispatch_scope,
                        outer_bindings,
                        factory,
                    );
                instantiated_rhs
            } else {
                entry.rhs
            };

            let multiplicity = entry.multiplicity.max(1);
            if multiplicity == 1 {
                matches.push((out_rhs, out_bindings));
            } else {
                for _ in 0..multiplicity {
                    matches.push((out_rhs.clone(), out_bindings.clone()));
                }
            }
        }
    }

    Some(matches)
}

// ============================================================================
// Phase F: Tight Deterministic Eval Loop
// ============================================================================

use super::dispatch_hints::{is_normal_form_bounded, is_reducible_head, operator_cache_get};

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
/// The trampoline alternative costs ~3-5 us per step (push/pop work item,
/// dispatch, push continuation, collect results). For PLN deterministic
/// operators (kbstatic, kbdynamic, PLNcategorizeObject), this saves ~80%
/// of per-step overhead.
#[inline]
pub fn try_deterministic_chain(
    expr: &MettaValue,
    env: &Environment,
    factory: &GcFactory,
) -> Option<MettaValue> {
    // Task #6 Phase 5 (2026-05-18): the historic `MAX_CHAIN_LENGTH = 512`
    // band-aid is replaced by a principled hash-cycle bound. For
    // self-referential rules like `(= (rec) (rec))` the chain previously
    // ran 512 inline iterations PER outer trampoline tick before
    // returning the unchanged `(rec)` — every tick burning chain budget
    // without making progress. Now we track seen content-hashes and
    // terminate as soon as we'd revisit a state. Worst-case bound is
    // `MAX_CHAIN_LENGTH` only as a safety net for pathological hash
    // collisions; the genuine termination criterion is the hash cycle,
    // not a magic 512.
    const MAX_CHAIN_LENGTH: usize = 512;

    let items = expr.as_sexpr()?;
    if items.is_empty() {
        return None;
    }

    let head = items[0].as_atom()?;
    if head.starts_with('$') {
        return None;
    } // Variable head
    if is_reducible_head(head) {
        return None;
    } // Special form / grounded op

    let arity = items.len() - 1;
    let cache_entry = operator_cache_get(head, arity)?;
    if !cache_entry.all_structural || cache_entry.candidate_count != 1 {
        return None;
    }

    // Seed the seen-set with the entry expression's hash so a self-
    // recursive RHS that returns the same form (e.g. `(rec) → (rec)`)
    // terminates after exactly one step.
    let mut seen: smallvec::SmallVec<[u64; 8]> = smallvec::SmallVec::new();
    seen.push(expr.hash_value());

    // First step: structural match against the single candidate
    let mut current = try_deterministic_step(expr, head, arity, env, factory)?;

    // Chain subsequent steps
    for _ in 1..MAX_CHAIN_LENGTH {
        // Hash-cycle termination: if `current` is a state we've already
        // been at (including the entry), the chain has reached a fixed
        // point and any further steps would loop. Return immediately;
        // the trampoline's cycle-detection / memoization layer handles
        // the outer recursion.
        let h = current.hash_value();
        if seen.iter().any(|&prev| prev == h) {
            return Some(current);
        }
        seen.push(h);

        let next_items = match current.as_sexpr() {
            Some(items) if !items.is_empty() => items,
            _ => return Some(current), // Not an S-expr or empty -> done
        };

        let next_head = match next_items[0].as_atom() {
            Some(h) => h,
            None => return Some(current), // Non-atom head -> done
        };

        if next_head.starts_with('$') {
            return Some(current);
        }
        if is_reducible_head(next_head) {
            return None;
        } // Need trampoline for special forms

        let next_arity = next_items.len() - 1;
        let next_cache = match operator_cache_get(next_head, next_arity) {
            Some(c) if c.all_structural && c.candidate_count == 1 => c,
            _ => return Some(current), // Non-deterministic or no cache -> return current for trampoline
        };
        let _ = next_cache;

        match try_deterministic_step(&current, next_head, next_arity, env, factory) {
            Some(result) => current = result,
            None => return Some(current), // Match failed -> return current for trampoline
        }
    }

    // Chain reached the safety-net cap without cycling — unlikely with
    // hash-cycle detection in place but preserved for pathological cases.
    Some(current)
}

/// Execute a single deterministic step: structural match + apply bindings.
///
/// Reads the rule index to get the single candidate, runs its structural
/// matcher, and applies bindings if variables exist in the RHS.
#[inline]
fn try_deterministic_step(
    expr: &MettaValue,
    head: &str,
    arity: usize,
    env: &Environment,
    factory: &GcFactory,
) -> Option<MettaValue> {
    use crate::backend::environment::rule_management::get_first_arg_head;

    let first_arg_head = get_first_arg_head(expr);
    let rule_index = env.shared.rule_index.read();
    let mut candidates = rule_index.get_candidates(head, arity, first_arg_head);

    let entry = candidates.next()?;
    // Verify it's actually a single candidate (no wildcard extras, etc.)
    if candidates.next().is_some() {
        return None;
    }

    let matcher = entry.structural_matcher.as_ref()?;
    let bindings = matcher.try_match(expr)?;

    let result = if entry.rhs_has_variables {
        apply_bindings(&entry.rhs, &bindings, factory)
    } else {
        entry.rhs
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

/// Result of a deferred deterministic chain.
pub enum DeferredChainResult {
    /// Chain terminated with a deferred (template, bindings) pair.
    /// Push EvalWithBindings to continue evaluation.
    Deferred {
        template: MettaValue,
        bindings: Bindings,
    },
    /// Chain terminated with a concrete value needing further evaluation.
    /// Push Eval to continue.
    Concrete(MettaValue),
    /// Chain terminated with a normal-form value. Push Resume directly.
    Done(MettaValue),
}

/// Attempt to chain deterministic rule applications from an EvalWithBindings context.
///
/// Given a `(template, bindings)` pair where the template is an S-expression:
/// 1. Materializes the expression via `apply_bindings`
/// 2. Checks if the head is a deterministic single-rule operator
/// 3. If so, performs structural matching and chains into the next step
/// 4. Returns `Some((final_rhs_template, composed_bindings))` for further
///    EvalWithBindings dispatch, or `Some((materialized_value, EMPTY_BINDINGS))`
///    if the chain terminates in a concrete value.
/// 5. Returns `None` if the first step isn't chainable (caller falls through).
///
/// # Performance
///
/// For a deterministic chain of length N, this replaces N x (materialize + Eval +
/// eval_step + eval_sexpr_step + try_match_all_rules + dispatch_rule_matches)
/// with N x (materialize + structural_match) + 1 x EvalWithBindings. Saves ~60%
/// of trampoline overhead for each chain step.
#[inline]
pub fn try_deferred_deterministic_chain(
    template: &MettaValue,
    bindings: &Bindings,
    env: &Environment,
    factory: &GcFactory,
) -> Option<DeferredChainResult> {
    const MAX_CHAIN_LENGTH: usize = 512;

    // Template must be an S-expr with a resolvable head
    let items = template.as_sexpr()?;
    if items.is_empty() {
        return None;
    }

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
    if head.starts_with('$') {
        return None;
    }
    if is_reducible_head(head) {
        return None;
    }

    let arity = items.len() - 1;
    let cache_entry = operator_cache_get(head, arity)?;
    if !cache_entry.all_structural || cache_entry.candidate_count != 1 {
        return None;
    }

    // First step: materialize and match
    let materialized = apply_bindings(template, bindings, factory);
    let (rhs_template, match_bindings) = try_deterministic_match(&materialized, head, arity, env)?;

    // If RHS has variables, we can defer materialization by composing bindings
    if rhs_template.has_variables_fast() {
        // Chain subsequent steps with composed bindings
        let mut current_template = rhs_template;
        let mut current_bindings = match_bindings;

        for _ in 1..MAX_CHAIN_LENGTH {
            // Check if current template is an S-expr with a chainable head
            let next_items = current_template.as_sexpr()?;
            if next_items.is_empty() {
                break;
            }

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

            if next_head.starts_with('$') {
                break;
            }
            if is_reducible_head(next_head) {
                break;
            }

            let next_arity = next_items.len() - 1;
            let next_cache = match operator_cache_get(next_head, next_arity) {
                Some(c) if c.all_structural && c.candidate_count == 1 => c,
                _ => break,
            };
            let _ = next_cache;

            // Materialize current template with current bindings for matching
            let next_materialized = apply_bindings(&current_template, &current_bindings, factory);
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

/// Execute a single deterministic match step, returning the RHS template and bindings.
///
/// Unlike `try_deterministic_step` which applies bindings immediately, this
/// returns the raw (rhs_template, match_bindings) for deferred binding composition.
#[inline]
fn try_deterministic_match(
    expr: &MettaValue,
    head: &str,
    arity: usize,
    env: &Environment,
) -> Option<(MettaValue, Bindings)> {
    use crate::backend::environment::rule_management::get_first_arg_head;

    let first_arg_head = get_first_arg_head(expr);
    let rule_index = env.shared.rule_index.read();
    let mut candidates = rule_index.get_candidates(head, arity, first_arg_head);

    let entry = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }

    let matcher = entry.structural_matcher.as_ref()?;
    let bindings = matcher.try_match(expr)?;

    Some((entry.rhs, bindings))
}

// ============================================================================
// Boolean Check Pattern Detection
// ============================================================================

/// Check if success/failure bodies represent a simple boolean check pattern.
///
/// Returns true if (success_body, failure_body) match:
/// - (Bool(true), Bool(false))
/// - (Atom("True"), Atom("False"))
///
/// This is used to optimize unify operations that are existence checks.
#[inline]
pub fn is_boolean_check_pattern(success_body: &MettaValue, failure_body: &MettaValue) -> bool {
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
// Switch/Case Evaluation
// ============================================================================

/// Result type for switch evaluation
pub enum SwitchResult {
    /// Match found - return the instantiated template and bindings
    Match(MettaValue, Bindings),
    /// No match found
    NoMatch,
    /// Error occurred
    Error(MettaValue),
}

/// Switch/case evaluation.
///
/// This function evaluates a switch/case expression by matching the atom against
/// the pattern in each case, and returns the instantiated template if a match is found.
///
/// # Arguments
///
/// - `atom`: The value to match against patterns
/// - `cases`: The cases s-expression: ((pattern1 template1) (pattern2 template2) ...)
/// - `factory`: The factory for constructing new values
///
/// # Returns
/// Check if a value contains a grounded sub-expression at any depth
/// that needs pre-evaluation before rule dispatch (e.g., `(+ 1 1)`,
/// `((+ 1 1) $x)`, `(f (collapse ...))`).
///
/// Used by the `EvalWithBindings` handler and tiered dispatch guard to
/// detect when SG1/deferred-chain fast paths must be bypassed in favor
/// of the materialization path (Step 2 pre-evaluation).
pub fn binding_value_needs_eval(value: &MettaValue) -> bool {
    use crate::backend::eval::helpers::{is_eager_special_form, is_grounded_op};
    if let Some(items) = value.as_sexpr() {
        if let Some(first) = items.first() {
            if let Some(head) = first.as_atom() {
                if is_grounded_op(head) || is_eager_special_form(head) {
                    return true;
                }
            }
        }
        // Shallow check: only inspect direct children (not fully recursive).
        // Deeply nested grounded sub-expressions like (f (g (+ 1 1))) will be
        // pre-evaluated when (g (+ 1 1)) is itself evaluated through Step 2.
        for item in items.iter().skip(1) {
            if let Some(sub_items) = item.as_sexpr() {
                if let Some(head) = sub_items.first().and_then(|h| h.as_atom()) {
                    if is_grounded_op(head) || is_eager_special_form(head) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check if a template S-expression has direct arguments whose heads are
/// grounded ops or eager special forms (e.g., `(f (+ 1 $x) ...)` where
/// `(+ 1 $x)` has grounded head `+`). These need pre-evaluation before
/// the expression is dispatched to rule matching.
///
/// Only checks user-defined function templates — special forms (if, let,
/// chain, case, etc.) evaluate their arguments through their own handlers.
pub fn template_has_grounded_arg_heads(template: &MettaValue) -> bool {
    use crate::backend::eval::helpers::{is_eager_special_form, is_grounded_op};

    if let Some(items) = template.as_sexpr() {
        // Check if the head is a user-defined function (not a special form)
        if let Some(first) = items.first() {
            if let Some(head) = first.as_atom() {
                // Skip special forms — they handle their own argument evaluation
                if matches!(
                    head,
                    "if" | "let"
                        | "let*"
                        | "chain"
                        | "case"
                        | "switch"
                        | "unify"
                        | "match"
                        | "match-or"
                        | "superpose"
                        | "collapse"
                        | "collapse-bind"
                        | "ground-with-bindings"
                        | "freeze-tuple"
                        | "map-atom"
                        | "filter-atom"
                        | "foldl-atom"
                        | "add-atom"
                        | "remove-atom"
                        | "get-atoms"
                        | "new-state"
                        | "get-state"
                        | "change-state!"
                        | "println!"
                        | "trace!"
                        | "nop"
                        | "quote"
                        | "unquote"
                        | "eval"
                        | "!"
                        | "sealed"
                        | "atom-subst"
                        | "new-space"
                        | "bind!"
                        | "import!"
                        | "git-import!"
                        | "include"
                        | "error"
                        | "is-error"
                        | "catch"
                        | "="
                        | ":"
                        | ":<"
                ) {
                    return false;
                }
            }
        }
        // Check arguments (skip head at index 0)
        for item in items.iter().skip(1) {
            if let Some(sub_items) = item.as_sexpr() {
                if let Some(first) = sub_items.first() {
                    if let Some(op) = first.as_atom() {
                        if is_grounded_op(op) || is_eager_special_form(op) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

///
/// - `SwitchResult::Match(template, bindings)` if a pattern matches
/// - `SwitchResult::NoMatch` if no pattern matches
/// - `SwitchResult::Error(err)` if there's an error (malformed case)
pub fn eval_switch(atom: &MettaValue, cases: &MettaValue, factory: &GcFactory) -> SwitchResult {
    // Cases must be an S-expression
    let Some(case_items) = cases.as_sexpr() else {
        let err = factory.error(
            factory.string(&format!(
                "switch-minimal expects expression as second argument, got: {}",
                cases.friendly_type_name()
            )),
            *cases,);
        return SwitchResult::Error(err);
    };

    // No cases - return NoMatch (caller should handle as NotReducible)
    if case_items.is_empty() {
        return SwitchResult::NoMatch;
    }

    // Iterate through cases looking for a match
    for case in case_items.iter() {
        // Each case must be an S-expression (pattern template)
        let Some(case_parts) = case.as_sexpr() else {
            let err = factory.error(
                factory.string("switch case should be an expression (pattern-template pair)"),
                *case,);
            return SwitchResult::Error(err);
        };

        // Each case must have exactly 2 elements: pattern and template
        if case_parts.len() != 2 {
            let err = factory.error(
                factory.string(&format!(
                    "switch case should be a pattern-template pair with exactly 2 elements, got {}. \
                    Usage: (switch expr (pattern1 result1) (pattern2 result2) ...)",
                    case_parts.len()
                )),
                *case,);
            return SwitchResult::Error(err);
        }

        let pattern = &case_parts[0];
        let template = &case_parts[1];

        // Try to match pattern against atom using pattern matching
        if let Some(bindings) = pattern_match(pattern, atom) {
            // Pattern matches - apply bindings to template
            let instantiated = apply_bindings(template, &bindings, factory);
            return SwitchResult::Match(instantiated, bindings);
        }
        // No match - continue to next case
    }

    // No case matched
    SwitchResult::NoMatch
}

// ============================================================================
// Iterative Binding Application
// ============================================================================

/// Apply bindings to a template using the iterative implementation.
///
/// This delegates to the iterative `apply_bindings` from
/// `bindings.rs`, which uses an explicit work stack instead of
/// recursion. Used by environment matching and space operations.
#[inline]
pub fn apply_bindings_iterative(
    template: &MettaValue,
    bindings: &Bindings,
    factory: &GcFactory,
) -> MettaValue {
    crate::backend::eval::bindings::apply_bindings_generic(template, bindings, factory)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_match_variable() {
        let pattern = MettaValue::Atom("$x".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.expect("bindings should be Some");
        assert_eq!(bindings.get("$x").map(|v| v.as_long()), Some(Some(42)));
    }

    #[test]
    fn test_pattern_match_wildcard() {
        let pattern = MettaValue::Atom("_".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        assert!(bindings.expect("bindings should be Some").is_empty());
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
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.expect("bindings should be Some");
        assert_eq!(bindings.get("$x").map(|v| v.as_long()), Some(Some(42)));
    }

    #[test]
    fn test_apply_bindings() {
        let factory = GcFactory::default();
        let mut bindings: Bindings = Bindings::new();
        bindings.insert("$x", MettaValue::Long(42));

        let template = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);

        let result = apply_bindings(&template, &bindings, &factory);
        assert!(result.is_sexpr());
        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items[1].as_long(), Some(42));
    }

    // Tests for semantic alignment with heap pattern_match

    #[test]
    fn test_pattern_match_ampersand_not_variable() {
        // Standalone "&" should NOT be treated as a variable
        let pattern = MettaValue::Atom("&".to_string());
        let value = MettaValue::Long(42);
        // Should NOT match - "&" is a literal, not a variable
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_none());
    }

    #[test]
    fn test_pattern_match_ampersand_variable_prefix() {
        // "&foo" (variable starting with &) SHOULD be treated as a variable
        let pattern = MettaValue::Atom("&foo".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.expect("bindings should be Some");
        assert_eq!(bindings.get("&foo").map(|v| v.as_long()), Some(Some(42)));
    }

    #[test]
    fn test_pattern_match_unit_unit() {
        // Unit pattern matches Unit
        let pattern = MettaValue::Unit();
        let value = MettaValue::Unit();
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_unit_empty_sexpr() {
        // Unit pattern matches empty S-expression (SExpr([]) normalizes to Unit)
        let pattern = MettaValue::Unit();
        let value = MettaValue::SExpr(vec![]);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_empty_sexpr_unit() {
        // Empty S-expression pattern matches Unit (SExpr([]) normalizes to Unit)
        let pattern = MettaValue::SExpr(vec![]);
        let value = MettaValue::Unit();
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_unit_empty_atom() {
        // Unit pattern matches Atom("Empty")
        let pattern = MettaValue::Unit();
        let value = MettaValue::Atom("Empty".to_string());
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_pattern_match_empty_atom_unit() {
        // Atom("Empty") does NOT match Unit -- they are different values.
        let pattern = MettaValue::Atom("Empty".to_string());
        let value = MettaValue::Unit();
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_none());
    }

    #[test]
    fn test_pattern_match_float_direct_equality() {
        // Float comparison uses direct equality (not epsilon)
        let pattern = MettaValue::Float(1.0);
        let value = MettaValue::Float(1.0);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());

        // Different floats should not match
        let pattern = MettaValue::Float(1.0);
        let value = MettaValue::Float(1.0 + f64::EPSILON * 2.0);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_none());
    }

    #[test]
    fn test_apply_bindings_ampersand_not_variable() {
        // Standalone "&" should NOT be substituted as a variable
        let factory = GcFactory::default();
        let mut bindings: Bindings = Bindings::new();
        bindings.insert("&", MettaValue::Long(42));

        let template = MettaValue::Atom("&".to_string());
        let result = apply_bindings(&template, &bindings, &factory);
        // Should remain as "&", not substituted
        assert_eq!(result.as_atom(), Some("&"));
    }

    #[test]
    fn test_apply_bindings_type_no_recursion() {
        // Type variants should NOT have bindings applied to their contents
        let factory = GcFactory::default();
        let mut bindings: Bindings = Bindings::new();
        bindings.insert("$x", MettaValue::Long(42));

        // Type wrapping a variable - should not substitute
        let template = MettaValue::Type(MettaValue::Atom("$x".to_string()));
        let result = apply_bindings(&template, &bindings, &factory);

        // Result should still be a Type with $x inside (not substituted)
        assert!(result.is_type());
        let inner = result.as_type().expect("should be type");
        assert_eq!(inner.as_atom(), Some("$x"));
    }
}
