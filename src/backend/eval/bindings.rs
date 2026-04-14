//! Generic Binding Operations
//!
//! This module provides generic implementations of binding operations that work
//! with any value type implementing `MettaValueTrait`. These are used by the
//! generic evaluation engine to avoid conversions between value types.
//!
//! ## Operations
//!
//! - `sealed` - Create locally scoped variables
//! - `atom-subst` - Variable substitution through pattern matching

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueInner, MettaValueTrait};

/// Global counter for generating unique variable IDs in `sealed`
static SEALED_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Collect all variable names from an expression (generic version)
///
/// Variables are atoms starting with '$'.
pub fn collect_variables_generic<V: MettaValueTrait>(expr: &V) -> HashSet<String> {
    let mut vars = HashSet::new();
    let mut work_stack: Vec<&V> = Vec::with_capacity(16);
    work_stack.push(expr);

    while let Some(val) = work_stack.pop() {
        if let Some(name) = val.as_atom() {
            if name.starts_with('$') {
                vars.insert(name.to_string());
            }
        } else if let Some(items) = val.as_sexpr() {
            for item in items.iter().rev() {
                work_stack.push(item);
            }
        } else if let Some(goals) = val.as_conjunction() {
            for goal in goals.iter().rev() {
                work_stack.push(goal);
            }
        }
    }

    vars
}

/// Seal variables in an expression (generic version)
///
/// Replaces variables NOT in the ignore set with unique versions.
pub fn seal_variables_generic<V, F>(expr: &V, ignore: &HashSet<String>, unique_id: u64, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Fast path for simple cases
    if let Some(name) = expr.as_atom() {
        if name.starts_with('$') && !ignore.contains(name) {
            return factory.atom(&format!("{}_{}", name, unique_id));
        }
        return expr.clone();
    }

    // Ground types pass through unchanged
    if matches!(expr.inner_raw(),
        MettaValueInner::Bool(_) | MettaValueInner::Long(_) | MettaValueInner::Float(_)
        | MettaValueInner::String(_) | MettaValueInner::Unit | MettaValueInner::Space(_)
        | MettaValueInner::State(_) | MettaValueInner::Type(_) | MettaValueInner::Memo(_)
        | MettaValueInner::Empty | MettaValueInner::Error(..))
    {
        return expr.clone();
    }

    // Compound types need iterative processing
    seal_variables_iterative_generic(expr, ignore, unique_id, factory)
}

/// Work item for iterative seal_variables
enum SealWork<'a, V> {
    Process(&'a V),
    BuildSExpr(usize),
    BuildConjunction(usize),
}

/// Iterative implementation of seal_variables using explicit work stack.
fn seal_variables_iterative_generic<V, F>(
    expr: &V,
    ignore: &HashSet<String>,
    unique_id: u64,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let mut work_stack: Vec<SealWork<V>> = Vec::with_capacity(32);
    let mut result_stack: Vec<V> = Vec::with_capacity(32);

    work_stack.push(SealWork::Process(expr));

    while let Some(work) = work_stack.pop() {
        match work {
            SealWork::Process(val) => {
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') && !ignore.contains(name) {
                        result_stack.push(factory.atom(&format!("{}_{}", name, unique_id)));
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(SealWork::BuildSExpr(items.len()));
                        for item in items.iter().rev() {
                            work_stack.push(SealWork::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(SealWork::BuildConjunction(goals.len()));
                        for goal in goals.iter().rev() {
                            work_stack.push(SealWork::Process(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            SealWork::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.sexpr(children));
            }
            SealWork::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.conjunction(children));
            }
        }
    }

    result_stack.pop().expect("Result stack should not be empty")
}

/// sealed: Create locally scoped variables (generic version)
/// Usage: (sealed ignore-vars expr)
pub fn eval_sealed_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() < 3 {
        return vec![factory.error(
            &format!(
                "sealed requires 2 arguments, got {}. Usage: (sealed ignore-vars expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let ignore_vars = &items[1];
    let expr = &items[2];

    // Collect variables to ignore
    let ignore_set = collect_variables_generic(ignore_vars);

    // Generate unique variable ID
    let unique_id = SEALED_COUNTER.fetch_add(1, Ordering::SeqCst);

    // Seal variables
    let sealed_expr = seal_variables_generic(expr, &ignore_set, unique_id, factory);

    vec![sealed_expr]
}

/// Generic pattern matching for simple variable binding
///
/// This is a simplified version that handles the common case of matching
/// a single variable against a value.
pub fn pattern_match_simple_generic<V: MettaValueTrait + Clone>(
    pattern: &V,
    value: &V,
) -> Option<GenericBindings<V>> {
    // If pattern is a variable, bind it to value
    if let Some(name) = pattern.as_atom() {
        if name.starts_with('$') {
            let mut bindings = GenericBindings::new();
            bindings.insert(name, value.clone());
            return Some(bindings);
        }
    }

    // If pattern equals value exactly, return empty bindings
    if pattern == value {
        return Some(GenericBindings::new());
    }

    // For more complex patterns, we would need full pattern matching
    // For atom-subst, we typically just have simple variable patterns
    None
}

/// Full generic pattern matching for any MettaValueTrait value.
///
/// This function performs pattern matching between a pattern and a value,
/// returning variable bindings if successful. Uses `MettaValueTrait` methods
/// instead of `MettaValueInner` pattern matching, enabling zero-conversion
/// operations for MettaValue.
///
/// ## Supported Pattern Types
///
/// - **Wildcards**: `_` matches any value, no binding created
/// - **Variables**: `$x`, `&y`, `'z` bind to the matched value
/// - **Atoms**: Must match exactly (except space references like `&self`)
/// - **Ground types**: Bool, Long, Float, String must match exactly
/// - **S-expressions**: Structural matching with recursive pattern matching
/// - **Conjunctions**: Structural matching for conjunction goals
/// - **Unit**: Matches empty S-expressions
///
/// ## Performance
///
/// Uses iterative work-stack approach to avoid stack overflow on deeply
/// nested expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks)
/// - Deeply nested data structures common in knowledge graphs
pub fn pattern_match_generic<V: MettaValueTrait + Clone>(
    pattern: &V,
    value: &V,
) -> Option<GenericBindings<V>> {
    let mut bindings = GenericBindings::new();
    if pattern_match_generic_impl(pattern, value, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

/// Internal implementation of generic pattern matching.
///
/// Uses an explicit work stack instead of recursion to handle deeply nested
/// structures without stack overflow.
fn pattern_match_generic_impl<V: MettaValueTrait + Clone>(
    pattern: &V,
    value: &V,
    bindings: &mut GenericBindings<V>,
) -> bool {
    // Work stack: (pattern, value) pairs to match
    let mut work_stack: Vec<(&V, &V)> = Vec::with_capacity(16);
    work_stack.push((pattern, value));

    while let Some((pat, val)) = work_stack.pop() {
        // Check if pattern is an atom (handles variables, wildcards, and literal atoms)
        if let Some(p_name) = pat.as_atom() {
            // Wildcard matches anything
            if p_name == "_" {
                continue;
            }

            // Check if it's a variable (starts with $, &, or ')
            // EXCEPT: standalone "&" is a literal operator, not a variable
            // EXCEPT: space references like &self, &kb, &stack are NOT variables
            let is_variable = (p_name.starts_with('$')
                || p_name.starts_with('&')
                || p_name.starts_with('\''))
                && p_name != "&"
                && p_name != "&self"
                && p_name != "&kb"
                && p_name != "&stack";

            if is_variable {
                // Check if variable is already bound
                if let Some(existing) = bindings.get(p_name) {
                    // Variable already bound - must match existing value
                    if existing != val {
                        return false;
                    }
                } else {
                    // New variable - bind to value
                    bindings.insert(p_name, val.clone());
                }
                continue;
            }

            // Literal atom - must match exactly
            if let Some(v_name) = val.as_atom() {
                if p_name == v_name {
                    continue;
                }
            }
            // Atom pattern "Empty" matches Empty sentinel
            if p_name == "Empty" && val.is_empty() {
                continue;
            }
            return false;
        }

        // Check ground types
        if let Some(p_bool) = pat.as_bool() {
            if let Some(v_bool) = val.as_bool() {
                if p_bool == v_bool {
                    continue;
                }
            }
            return false;
        }

        if let Some(p_long) = pat.as_long() {
            if let Some(v_long) = val.as_long() {
                if p_long == v_long {
                    continue;
                }
            }
            return false;
        }

        if let Some(p_float) = pat.as_float() {
            if let Some(v_float) = val.as_float() {
                if (p_float - v_float).abs() < f64::EPSILON {
                    continue;
                }
            }
            return false;
        }

        if let Some(p_str) = pat.as_string() {
            if let Some(v_str) = val.as_string() {
                if p_str == v_str {
                    continue;
                }
            }
            return false;
        }

        // Unit matches Unit
        if pat.is_unit() {
            if val.is_unit() {
                continue;
            }
            return false;
        }

        // S-expressions: structural matching
        if let Some(p_items) = pat.as_sexpr() {
            // Empty S-expr pattern matches empty values
            if p_items.is_empty() {
                if val.is_unit() {
                    continue;
                }
                if let Some(v_items) = val.as_sexpr() {
                    if v_items.is_empty() {
                        continue;
                    }
                }
                if let Some(name) = val.as_atom() {
                    if name == "Empty" {
                        continue;
                    }
                }
                return false;
            }

            // Non-empty S-expr must match non-empty S-expr
            if let Some(v_items) = val.as_sexpr() {
                if p_items.len() != v_items.len() {
                    return false;
                }
                // Push children in reverse order (LIFO)
                for (p, v) in p_items.iter().zip(v_items.iter()).rev() {
                    work_stack.push((p, v));
                }
                continue;
            }
            return false;
        }

        // Conjunctions: structural matching
        if let Some(p_goals) = pat.as_conjunction() {
            if let Some(v_goals) = val.as_conjunction() {
                if p_goals.len() != v_goals.len() {
                    return false;
                }
                // Push children in reverse order (LIFO)
                for (p, v) in p_goals.iter().zip(v_goals.iter()).rev() {
                    work_stack.push((p, v));
                }
                continue;
            }
            return false;
        }

        // Errors: check message match, push details
        if let Some((p_msg, p_details)) = pat.as_error() {
            if let Some((v_msg, v_details)) = val.as_error() {
                if p_msg != v_msg {
                    return false;
                }
                work_stack.push((p_details, v_details));
                continue;
            }
            return false;
        }

        // Space handles: must match by id
        if let Some(p_handle) = pat.as_space() {
            if let Some(v_handle) = val.as_space() {
                if p_handle.id == v_handle.id {
                    continue;
                }
            }
            return false;
        }

        // State handles: must match by id
        if let Some(p_id) = pat.as_state() {
            if let Some(v_id) = val.as_state() {
                if p_id == v_id {
                    continue;
                }
            }
            return false;
        }

        // Type wrappers: match inner values
        if let Some(p_inner) = pat.as_type() {
            if let Some(v_inner) = val.as_type() {
                work_stack.push((p_inner, v_inner));
                continue;
            }
            return false;
        }

        // Empty sentinel matches empty sentinel
        if pat.is_empty() && val.is_empty() {
            continue;
        }

        // Default: no match
        return false;
    }

    true // All pairs matched successfully
}

/// Apply bindings to a template (generic version)
///
/// Replaces variables in template with their bound values.
/// Preserves Spanned wrappers: if the template has a span, the result
/// will be wrapped in Spanned with the same span.
pub fn apply_bindings_generic<V, F>(template: &V, bindings: &GenericBindings<V>, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Fast path: if no bindings, return as-is
    if bindings.is_empty() {
        return template.clone();
    }

    // Peel Spanned: process inner, re-wrap with same span
    if let Some(span) = template.span() {
        let span = *span; // Copy
        let stripped = template.strip_one_span();
        let result = apply_bindings_generic(&stripped, bindings, factory);
        // Skip wrapping if result already carries a span
        if result.span().is_some() {
            return result;
        }
        return factory.spanned(result, span);
    }

    apply_bindings_iterative_generic(template, bindings, factory)
}

/// Iterative implementation of apply_bindings.
///
/// Uses an explicit work stack (pushdown automaton) instead of recursion to
/// avoid Rust call-stack overflow on deeply nested expressions or long
/// transitive binding chains.
///
/// **Transitive substitution**: when a variable lookup finds a bound value
/// that itself contains variables, those inner variables are also resolved.
/// This is critical for bidirectional unification cases where one rule
/// variable is bound to an expression containing a free input variable that
/// has been bound by unification of a sibling occurrence:
///
///     bindings: {$B → (Inheritance $1 (IntSet cancerous)), $1 → Anna}
///     template: $B
///     result:   (Inheritance Anna (IntSet cancerous))
///
/// Without transitive substitution, the result would be the unreduced
/// (Inheritance $1 (IntSet cancerous)). The transitive walk happens via the
/// existing work stack (using `Work::ProcessOwned`), so there is NO call-stack
/// growth — this is safe for arbitrarily deep transitive chains.
///
/// Cycle prevention: bidirectional_unify_generic enforces an occurs check
/// at unification time, so cyclic bindings (e.g., $a → (... $a ...)) cannot
/// enter the bindings map. Without cycles, the transitive walk terminates.
///
/// Precondition: `template` is not Spanned (caller peels it).
fn apply_bindings_iterative_generic<V, F>(
    template: &V,
    bindings: &GenericBindings<V>,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    enum Work<'a, V> {
        /// Process a borrowed value from the original template.
        Process(&'a V),
        /// Process an owned value popped from the bindings map. Owned because
        /// the bound value's lifetime is tied to `bindings`, not `template`.
        ProcessOwned(V),
        BuildSExpr(usize),
        BuildConjunction(usize),
    }

    let mut work_stack: Vec<Work<V>> = Vec::with_capacity(32);
    let mut result_stack: Vec<V> = Vec::with_capacity(32);

    work_stack.push(Work::Process(template));

    while let Some(work) = work_stack.pop() {
        match work {
            Work::Process(val) => {
                // Handle Spanned children by peeling span, processing, re-wrapping.
                // This calls apply_bindings_generic which peels one Spanned layer,
                // then calls apply_bindings_iterative_generic on the stripped value.
                // Safe because MettaValue has at most one Spanned layer.
                if val.is_spanned() {
                    let result = apply_bindings_generic(val, bindings, factory);
                    result_stack.push(result);
                    continue;
                }

                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') {
                        if let Some(bound) = bindings.get(name) {
                            // Guard: self-referential binding ($a → $a) — emit
                            // directly to prevent infinite transitive loop.
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound.clone());
                                continue;
                            }
                            // Transitive substitution: push the bound value
                            // back to the work stack so any inner variables
                            // also get substituted.
                            work_stack.push(Work::ProcessOwned(bound.clone()));
                        } else {
                            result_stack.push(val.clone());
                        }
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildSExpr(items.len()));
                        for item in items.iter().rev() {
                            work_stack.push(Work::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildConjunction(goals.len()));
                        for goal in goals.iter().rev() {
                            work_stack.push(Work::Process(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            Work::ProcessOwned(val) => {
                // Same logic as `Process` but operating on an owned value
                // (the value was popped from the bindings map, so its
                // lifetime is no longer tied to the input template).
                if val.is_spanned() {
                    let result = apply_bindings_generic(&val, bindings, factory);
                    result_stack.push(result);
                    continue;
                }

                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') {
                        if let Some(bound) = bindings.get(name) {
                            // Guard: self-referential binding ($a → $a) — emit
                            // directly to prevent infinite transitive loop.
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound.clone());
                                continue;
                            }
                            // Transitive: re-process the (newly) bound value.
                            work_stack.push(Work::ProcessOwned(bound.clone()));
                        } else {
                            result_stack.push(val);
                        }
                    } else {
                        result_stack.push(val);
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val);
                    } else {
                        // Children are borrowed from `val`; clone each before
                        // pushing because `val` is consumed when this branch ends.
                        let len = items.len();
                        let owned_children: Vec<V> = items.iter().cloned().collect();
                        work_stack.push(Work::BuildSExpr(len));
                        for item in owned_children.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = goals.len();
                        let owned_goals: Vec<V> = goals.iter().cloned().collect();
                        work_stack.push(Work::BuildConjunction(len));
                        for goal in owned_goals.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(goal));
                        }
                    }
                } else {
                    result_stack.push(val);
                }
            }
            Work::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.sexpr(children));
            }
            Work::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.conjunction(children));
            }
        }
    }

    result_stack.pop().expect("Result stack should not be empty")
}

// =============================================================================
// Bidirectional Unification (Martelli-Montanari)
// =============================================================================

/// Bidirectional unification using the Martelli-Montanari algorithm.
///
/// Unlike one-directional `pattern_match_generic` (which only binds variables
/// on the pattern side), this handles variables on **both sides** simultaneously.
///
/// ## Algorithm
///
/// Maintains a work stack of `(V, V)` equation pairs. For each pair:
/// 1. **Deref** both sides through existing bindings (transitive)
/// 2. **Variable vs anything**: Occurs check, then bind (or verify consistency)
/// 3. **S-expression decomposition**: Arity check, then push child pairs
/// 4. **Ground term comparison**: Structural equality
///
/// Based on: Martelli & Montanari (1982), "An Efficient Unification Algorithm"
/// Reference implementation with Rocq proofs: mettail-rust/prattail/src/unification.rs
///
/// ## Complexity
///
/// O(n * k) where n = total term size and k = number of variables (for consistency
/// checks). Occurs check adds O(|term|) per binding. For typical MeTTa patterns
/// with 3-10 variables, this is effectively linear.
///
/// ## Examples
///
/// ```text
/// unify((a $x), ($y b))         => Some({$x → b, $y → a})
/// unify(($x $x), (a a))         => Some({$x → a})
/// unify(($x $x), (a b))         => None  (variable consistency)
/// unify($x, (f $x))             => None  (occurs check)
/// unify((f _ $y), (f anything z)) => Some({$y → z})
/// ```
pub fn bidirectional_unify_generic<V: MettaValueTrait + Clone>(
    a: &V,
    b: &V,
) -> Option<GenericBindings<V>> {
    let mut bindings = GenericBindings::new();
    if bidirectional_unify_generic_impl(a, b, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

/// Check if `name` refers to a MeTTa variable (starts with `$`, `&`, or `'`).
///
/// Excludes standalone `&` (literal operator) and space references (`&self`, `&kb`, `&stack`).
#[inline]
fn is_unification_variable(name: &str) -> bool {
    (name.starts_with('$') || name.starts_with('&') || name.starts_with('\''))
        && name != "&"
        && name != "&self"
        && name != "&kb"
        && name != "&stack"
}

/// Iterative occurs check: does variable `var_name` appear anywhere in `term`?
///
/// Uses an explicit work stack to avoid stack overflow on deeply nested terms.
/// Returns `true` if `var_name` occurs in `term` (unification would create an
/// infinite term).
/// Public wrapper for occurs check (used by VM opcodes).
pub fn occurs_in_generic_pub<V: MettaValueTrait + Clone>(
    var_name: &str,
    term: &V,
    bindings: &GenericBindings<V>,
) -> bool {
    occurs_in_generic(var_name, term, bindings)
}

fn occurs_in_generic<V: MettaValueTrait + Clone>(
    var_name: &str,
    term: &V,
    bindings: &GenericBindings<V>,
) -> bool {
    // Owned work stack (V is Copy for MettaValue, so cloning is zero-cost)
    let mut work_stack: Vec<V> = Vec::with_capacity(16);
    work_stack.push(term.clone());

    while let Some(raw) = work_stack.pop() {
        // Deref through bindings
        let current = deref_value_owned(&raw, bindings);

        if let Some(name) = current.as_atom() {
            if name == var_name {
                return true;
            }
            // Don't descend into atoms further
            continue;
        }

        if let Some(items) = current.as_sexpr() {
            for item in items.iter() {
                work_stack.push(item.clone());
            }
            continue;
        }

        if let Some(goals) = current.as_conjunction() {
            for goal in goals.iter() {
                work_stack.push(goal.clone());
            }
            continue;
        }

        if let Some((_msg, details)) = current.as_error() {
            work_stack.push(details.clone());
            continue;
        }

        if let Some(inner) = current.as_type() {
            work_stack.push(inner.clone());
        }

        // Ground types (Long, Bool, Float, String, Unit, etc.) cannot contain variables
    }

    false
}

/// Internal implementation of Martelli-Montanari bidirectional unification.
///
/// Uses an owned work stack of equation pairs `(lhs, rhs)`. Since `V` is
/// typically Copy (MettaValue is 8 bytes), cloning is zero-cost.
/// Bindings are accumulated in the `bindings` parameter.
fn bidirectional_unify_generic_impl<V: MettaValueTrait + Clone>(
    a: &V,
    b: &V,
    bindings: &mut GenericBindings<V>,
) -> bool {
    // Owned work stack — V is Copy for MettaValue so cloning is zero-cost.
    let mut work_stack: Vec<(V, V)> = Vec::with_capacity(16);
    work_stack.push((a.clone(), b.clone()));

    while let Some((lhs_raw, rhs_raw)) = work_stack.pop() {
        // Step 1: Dereference both sides through existing bindings (transitive)
        let lhs = deref_value_owned(&lhs_raw, bindings);
        let rhs = deref_value_owned(&rhs_raw, bindings);

        // Step 2: Trivial identity
        if lhs == rhs {
            continue;
        }

        // Step 3: Wildcards (match anything without binding)
        if let Some(name) = lhs.as_atom() {
            if name == "_" {
                continue;
            }
        }
        if let Some(name) = rhs.as_atom() {
            if name == "_" {
                continue;
            }
        }

        // Step 4: Variable on LHS
        if let Some(l_name) = lhs.as_atom() {
            if is_unification_variable(l_name) {
                // Already bound? Push (existing_value, rhs) for consistency check
                if let Some(existing) = bindings.get(l_name) {
                    let existing = existing.clone();
                    work_stack.push((existing, rhs));
                    continue;
                }

                // Occurs check: prevent $x = f($x) → infinite terms
                if occurs_in_generic(l_name, &rhs, bindings) {
                    return false;
                }

                // Bind: l_name → rhs
                bindings.insert(l_name, rhs);
                continue;
            }
        }

        // Step 5: Variable on RHS
        if let Some(r_name) = rhs.as_atom() {
            if is_unification_variable(r_name) {
                // Already bound? Push (existing_value, lhs) for consistency check
                if let Some(existing) = bindings.get(r_name) {
                    let existing = existing.clone();
                    work_stack.push((existing, lhs));
                    continue;
                }

                // Occurs check
                if occurs_in_generic(r_name, &lhs, bindings) {
                    return false;
                }

                // Bind: r_name → lhs
                bindings.insert(r_name, lhs);
                continue;
            }
        }

        // Step 6: Both are non-variable — structural comparison

        // Atoms: must match exactly
        if let Some(l_name) = lhs.as_atom() {
            if let Some(r_name) = rhs.as_atom() {
                if l_name == r_name {
                    continue;
                }
            }
            // Atom "Empty" matches Empty sentinel
            if l_name == "Empty" && rhs.is_empty() {
                continue;
            }
            return false;
        }
        // RHS atom "Empty" matching LHS Empty sentinel
        if let Some(r_name) = rhs.as_atom() {
            if r_name == "Empty" && lhs.is_empty() {
                continue;
            }
            return false;
        }

        // Ground types
        if let Some(l_bool) = lhs.as_bool() {
            if let Some(r_bool) = rhs.as_bool() {
                if l_bool == r_bool {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_long) = lhs.as_long() {
            if let Some(r_long) = rhs.as_long() {
                if l_long == r_long {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_float) = lhs.as_float() {
            if let Some(r_float) = rhs.as_float() {
                if l_float.to_bits() == r_float.to_bits() {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_str) = lhs.as_string() {
            if let Some(r_str) = rhs.as_string() {
                if l_str == r_str {
                    continue;
                }
            }
            return false;
        }

        // Unit matches Unit
        if lhs.is_unit() {
            if rhs.is_unit() {
                continue;
            }
            // Unit matches empty S-expr
            if let Some(r_items) = rhs.as_sexpr() {
                if r_items.is_empty() {
                    continue;
                }
            }
            return false;
        }
        if rhs.is_unit() {
            // Empty S-expr matches Unit (symmetric)
            if let Some(l_items) = lhs.as_sexpr() {
                if l_items.is_empty() {
                    continue;
                }
            }
            return false;
        }

        // S-expressions: decompose element-wise
        if let Some(l_items) = lhs.as_sexpr() {
            if let Some(r_items) = rhs.as_sexpr() {
                if l_items.len() != r_items.len() {
                    return false;
                }
                if l_items.is_empty() {
                    continue;
                }
                // Push child pairs in reverse order (LIFO → left-to-right processing)
                for (l, r) in l_items.iter().zip(r_items.iter()).rev() {
                    work_stack.push((l.clone(), r.clone()));
                }
                continue;
            }
            return false;
        }

        // Conjunctions: structural matching
        if let Some(l_goals) = lhs.as_conjunction() {
            if let Some(r_goals) = rhs.as_conjunction() {
                if l_goals.len() != r_goals.len() {
                    return false;
                }
                for (l, r) in l_goals.iter().zip(r_goals.iter()).rev() {
                    work_stack.push((l.clone(), r.clone()));
                }
                continue;
            }
            return false;
        }

        // Errors: structural matching
        if let Some((l_msg, l_details)) = lhs.as_error() {
            if let Some((r_msg, r_details)) = rhs.as_error() {
                if l_msg != r_msg {
                    return false;
                }
                work_stack.push((l_details.clone(), r_details.clone()));
                continue;
            }
            return false;
        }

        // Space handles
        if let Some(l_handle) = lhs.as_space() {
            if let Some(r_handle) = rhs.as_space() {
                if l_handle.id == r_handle.id {
                    continue;
                }
            }
            return false;
        }

        // State handles
        if let Some(l_id) = lhs.as_state() {
            if let Some(r_id) = rhs.as_state() {
                if l_id == r_id {
                    continue;
                }
            }
            return false;
        }

        // Type wrappers
        if let Some(l_inner) = lhs.as_type() {
            if let Some(r_inner) = rhs.as_type() {
                work_stack.push((l_inner.clone(), r_inner.clone()));
                continue;
            }
            return false;
        }

        // Empty sentinel
        if lhs.is_empty() && rhs.is_empty() {
            continue;
        }

        // Default: no match
        return false;
    }

    true // All equation pairs unified successfully
}

/// Dereference a value through existing bindings transitively (owned version).
///
/// Follows binding chains until reaching a non-variable or unbound variable.
/// Returns an owned value (Clone is zero-cost for Copy types like MettaValue).
fn deref_value_owned<V: MettaValueTrait + Clone>(
    val: &V,
    bindings: &GenericBindings<V>,
) -> V {
    let mut current = val.clone();
    // Limit chain length to prevent infinite loops from buggy bindings
    for _ in 0..64 {
        if let Some(name) = current.as_atom() {
            if is_unification_variable(name) {
                if let Some(bound) = bindings.get(name) {
                    current = bound.clone();
                    continue;
                }
            }
        }
        break;
    }
    current
}

/// atom-subst: Variable substitution (generic version)
/// Usage: (atom-subst value $var template)
pub fn eval_atom_subst_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() < 4 {
        return vec![factory.error(
            &format!(
                "atom-subst requires 3 arguments, got {}. Usage: (atom-subst value $var template)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let value = &items[1];
    let var = &items[2];
    let template = &items[3];

    // Use pattern matching to bind value to var
    if let Some(bindings) = pattern_match_simple_generic(var, value) {
        let instantiated = apply_bindings_generic(template, &bindings, factory);
        vec![instantiated]
    } else {
        // Pattern didn't match - return empty
        vec![]
    }
}

// ============================================================================
// Per-branch binding propagation helpers (Stage 1c+)
// ============================================================================
//
// These helpers support MeTTa-HE-faithful per-branch binding propagation
// through the trampoline evaluator. They operate on `GenericBindings<V>`
// and are used at rule-match composition sites and at `collapse-bind` scope
// boundaries. Bindings here encode the variable-substitution context of
// a specific nondeterministic branch — each `BoundValue`'s `.1` field.

/// Detect whether a `MettaValueTrait` value is a variable atom (starts with
/// `$`). Variables that appear as VALUES in a binding indicate a chain —
/// e.g., `{$a: $who}` means $a is aliased to $who (set up by bidirectional
/// unification when a rule variable met a template variable).
#[inline]
pub fn is_variable_value<V: MettaValueTrait>(val: &V) -> bool {
    val.as_atom().map_or(false, |s| s.starts_with('$'))
}

/// Unification-style composition of an outer binding set with an inner one.
///
/// When a rule-match produces `outer = {$a_ruleX: $template_var}` (the rule
/// variable is bound to the caller's template variable via bidirectional
/// unification) and a sub-evaluation of the RHS produces `inner = {$a_ruleX:
/// ground_value}` (the rule variable gets concretized deep inside the RHS),
/// the chain `$template_var ↔ $a_ruleX ↔ ground_value` must be resolved so
/// the final binding set contains `$template_var → ground_value`.
///
/// Algorithm (per shared key `k`):
/// - If only `outer` has `k`: result[k] = outer[k]
/// - If only `inner` has `k`: result[k] = inner[k]
/// - If both have `k`:
///   - If outer[k] is a variable atom `$X`: `$X` is an alias for the value
///     inner binds to `k`, so add `$X → inner[k]` AND keep `k → inner[k]`.
///     (Symmetric case for `inner[k]` being a variable alias.)
///   - If both are equal: use that value.
///   - Otherwise: ground-value conflict — this branch is inconsistent.
///     We emit an `Empty` bindings set (the caller treats this as the
///     branch's bindings being degenerate; the branch's RESULT is still
///     retained, but with no per-branch bindings recorded). This matches
///     the pragmatic "preserve the fact that a result was produced"
///     semantics since MeTTaTron's rule evaluation already performs the
///     actual substitution up-front via apply_bindings.
///
/// After composition, call `apply_chain_generic` to transitively resolve
/// any newly-introduced chain entries.
pub fn compose_outer_inner_generic<V, F>(
    outer: &GenericBindings<V>,
    inner: &GenericBindings<V>,
    factory: &F,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if outer.is_empty() {
        return inner.clone();
    }
    if inner.is_empty() {
        return outer.clone();
    }
    let _ = factory;
    let mut result = GenericBindings::new();
    // First pass: for keys in both, unify their values.
    for (name, outer_val) in outer.iter() {
        if let Some(inner_val) = inner.get(name) {
            // Both bind `name`. Unify.
            if outer_val == inner_val {
                result.insert_or_replace(name, outer_val.clone());
            } else if is_variable_value(outer_val) {
                // outer: name → $X, inner: name → inner_val. So $X aliases
                // to inner_val. Record both.
                if let Some(x_name) = outer_val.as_atom() {
                    // MettaValueTrait::as_atom returns `&'static str` (atoms
                    // are interned in the global atom pool).
                    if let Some(existing) = result.get(x_name) {
                        if existing != inner_val {
                            // Conflict on the alias — inconsistent branch,
                            // drop to empty.
                            return GenericBindings::new();
                        }
                    } else {
                        result.insert_or_replace(x_name, inner_val.clone());
                    }
                }
                result.insert_or_replace(name, inner_val.clone());
            } else if is_variable_value(inner_val) {
                // Symmetric: inner: name → $Y, outer: name → outer_val.
                if let Some(y_name) = inner_val.as_atom() {
                    if let Some(existing) = result.get(y_name) {
                        if existing != outer_val {
                            return GenericBindings::new();
                        }
                    } else {
                        result.insert_or_replace(y_name, outer_val.clone());
                    }
                }
                result.insert_or_replace(name, outer_val.clone());
            } else {
                // Both ground but unequal — inconsistent.
                return GenericBindings::new();
            }
        } else {
            // Only outer has it.
            result.insert_or_replace(name, outer_val.clone());
        }
    }
    // Second pass: add inner-only keys.
    for (name, inner_val) in inner.iter() {
        if result.get(name).is_none() {
            result.insert_or_replace(name, inner_val.clone());
        }
    }
    result
}

/// Transitive chain resolution: for every (v, val) in the bindings, rewrite
/// `val` via `apply_bindings(val, self)` until a fixed point.
///
/// Handles chains like `{$a: $who, $who: a}` → `{$a: a, $who: a}`.
/// Uses iteration with a max-depth guard (16) to handle pathological cycles
/// without infinite looping. `apply_bindings_generic` itself handles
/// transitive substitution, so typically one pass suffices; the loop is a
/// safeguard for value-level rewrites that don't settle on the first pass.
pub fn apply_chain_generic<V, F>(bindings: &mut GenericBindings<V>, factory: &F)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if bindings.is_empty() || bindings.len() == 1 {
        // Single entry can't have a chain within this binding set.
        return;
    }
    const MAX_PASSES: usize = 16;
    let snapshot_keys: Vec<&'static str> = bindings.iter().map(|(k, _)| k).collect();
    let mut pass = 0;
    loop {
        let mut changed = false;
        for &name in &snapshot_keys {
            let val = match bindings.get(name) {
                Some(v) => v.clone(),
                None => continue,
            };
            let resolved = apply_bindings_generic(&val, bindings, factory);
            if !val.identity_eq(&resolved) && val != resolved {
                bindings.insert_or_replace(name, resolved);
                changed = true;
            }
        }
        pass += 1;
        if !changed || pass >= MAX_PASSES {
            break;
        }
    }
}

/// Project a binding set to just the tracked-var keys. For each tracked
/// var, look it up in `self` and copy the resolved value. Tracked vars
/// that don't appear in `self` are omitted.
///
/// Assumes `apply_chain_generic` has already been called on `bindings`,
/// so values in `bindings` are already transitively resolved. This is
/// the projection step used at `ProcessCollapseBind` output to emit only
/// the variables the caller cares about in the `(Bindings …)` sidecar.
pub fn project_bindings_generic<V: MettaValueTrait + Clone>(
    bindings: &GenericBindings<V>,
    keep: &[&'static str],
) -> GenericBindings<V> {
    if bindings.is_empty() || keep.is_empty() {
        return GenericBindings::new();
    }
    let mut result = GenericBindings::new();
    for &v in keep {
        if let Some(val) = bindings.get(v) {
            result.insert_or_replace(v, val.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_collect_variables_generic() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let vars = collect_variables_generic(&expr);
        assert!(vars.contains("$x"));
        assert!(vars.contains("$y"));
        assert!(!vars.contains("foo"));
    }

    #[test]
    fn test_seal_variables_generic() {
        let factory = GcFactory::default();
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let mut ignore = HashSet::new();
        ignore.insert("$x".to_string());

        let sealed = seal_variables_generic(&expr, &ignore, 42, &factory);

        if let Some(items) = sealed.as_sexpr() {
            assert_eq!(items[0].as_atom(), Some("foo"));
            assert_eq!(items[1].as_atom(), Some("$x")); // preserved
            assert_eq!(items[2].as_atom(), Some("$y_42")); // sealed
        } else {
            panic!("Expected sexpr");
        }
    }

    #[test]
    fn test_eval_sealed_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("sealed".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("$x".to_string())]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ];
        let result = eval_sealed_generic(&items, &factory);
        assert_eq!(result.len(), 1);

        if let Some(items) = result[0].as_sexpr() {
            assert_eq!(items[0].as_atom(), Some("foo"));
            assert_eq!(items[1].as_atom(), Some("$x")); // preserved
            // $y should be sealed with some unique ID
            assert!(items[2].as_atom().map(|s| s.starts_with("$y_")).unwrap_or(false));
        }
    }

    #[test]
    fn test_eval_atom_subst_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("atom-subst".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        ];
        let result = eval_atom_subst_generic(&items, &factory);
        assert_eq!(result.len(), 1);

        if let Some(items) = result[0].as_sexpr() {
            assert_eq!(items[0].as_atom(), Some("+"));
            assert_eq!(items[1].as_long(), Some(42));
            assert_eq!(items[2].as_long(), Some(1));
        }
    }

    // =========================================================================
    // Bidirectional Unification (Martelli-Montanari) Tests
    // =========================================================================

    /// Helper: create an S-expression from a slice of MettaValues
    fn sexpr(items: Vec<MettaValue>) -> MettaValue {
        MettaValue::SExpr(items)
    }

    #[test]
    fn test_unify_identical_atoms() {
        let a = MettaValue::sym("foo");
        let b = MettaValue::sym("foo");
        let result = bidirectional_unify_generic(&a, &b);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_different_atoms() {
        let a = MettaValue::sym("foo");
        let b = MettaValue::sym("bar");
        let result = bidirectional_unify_generic(&a, &b);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_variable_lhs() {
        let var = MettaValue::var("x");
        let val = MettaValue::sym("hello");
        let result = bidirectional_unify_generic(&var, &val);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("hello"));
    }

    #[test]
    fn test_unify_variable_rhs() {
        let val = MettaValue::sym("hello");
        let var = MettaValue::var("x");
        let result = bidirectional_unify_generic(&val, &var);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("hello"));
    }

    #[test]
    fn test_unify_bidirectional() {
        // (a $x) unified with ($y b) => {$x -> b, $y -> a}
        let lhs = sexpr(vec![MettaValue::sym("a"), MettaValue::var("x")]);
        let rhs = sexpr(vec![MettaValue::var("y"), MettaValue::sym("b")]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("b"));
        assert_eq!(bindings.get("$y").unwrap().as_atom(), Some("a"));
    }

    #[test]
    fn test_unify_variable_consistency_success() {
        // ($x $x) unified with (a a) => Some({$x -> a})
        let pattern = sexpr(vec![MettaValue::var("x"), MettaValue::var("x")]);
        let value = sexpr(vec![MettaValue::sym("a"), MettaValue::sym("a")]);
        let result = bidirectional_unify_generic(&pattern, &value);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("a"));
    }

    #[test]
    fn test_unify_variable_consistency_failure() {
        // ($x $x) unified with (a b) => None
        let pattern = sexpr(vec![MettaValue::var("x"), MettaValue::var("x")]);
        let value = sexpr(vec![MettaValue::sym("a"), MettaValue::sym("b")]);
        let result = bidirectional_unify_generic(&pattern, &value);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_occurs_check_simple() {
        // $x unified with (f $x) => None (infinite term)
        let var = MettaValue::var("x");
        let term = sexpr(vec![MettaValue::sym("f"), MettaValue::var("x")]);
        let result = bidirectional_unify_generic(&var, &term);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_occurs_check_nested() {
        // $x unified with (f (g $x)) => None
        let var = MettaValue::var("x");
        let term = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("x")]),
        ]);
        let result = bidirectional_unify_generic(&var, &term);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_occurs_check_success() {
        // $x unified with (f $y) => Some({$x -> (f $y)})
        let var = MettaValue::var("x");
        let term = sexpr(vec![MettaValue::sym("f"), MettaValue::var("y")]);
        let result = bidirectional_unify_generic(&var, &term);
        assert!(result.is_some());
    }

    #[test]
    fn test_unify_all_variable_prefixes() {
        // (&x 'y $z) vs (a b c) => Some({&x->a, 'y->b, $z->c})
        let factory = GcFactory::default();
        let lhs = sexpr(vec![
            factory.atom("&x"),
            factory.atom("'y"),
            MettaValue::var("z"),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("a"),
            MettaValue::sym("b"),
            MettaValue::sym("c"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("&x").unwrap().as_atom(), Some("a"));
        assert_eq!(bindings.get("'y").unwrap().as_atom(), Some("b"));
        assert_eq!(bindings.get("$z").unwrap().as_atom(), Some("c"));
    }

    #[test]
    fn test_unify_standalone_ampersand_not_variable() {
        // (& $x) vs (& 5) => Some({$x -> 5})
        let factory = GcFactory::default();
        let lhs = sexpr(vec![factory.atom("&"), MettaValue::var("x")]);
        let rhs = sexpr(vec![factory.atom("&"), MettaValue::Long(5)]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_long(), Some(5));
    }

    #[test]
    fn test_unify_wildcard_lhs() {
        let wildcard = MettaValue::sym("_");
        let val = MettaValue::Long(42);
        let result = bidirectional_unify_generic(&wildcard, &val);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_wildcard_rhs() {
        let val = MettaValue::Long(42);
        let wildcard = MettaValue::sym("_");
        let result = bidirectional_unify_generic(&val, &wildcard);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_wildcard_both_sides() {
        // (_ $x) vs ($y _) => Some({$x -> _, $y -> _})
        let lhs = sexpr(vec![MettaValue::sym("_"), MettaValue::var("x")]);
        let rhs = sexpr(vec![MettaValue::var("y"), MettaValue::sym("_")]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
    }

    #[test]
    fn test_unify_nested_sexpr() {
        // (f (g $x) $y) vs (f (g a) b) => Some({$x -> a, $y -> b})
        let lhs = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("x")]),
            MettaValue::var("y"),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::sym("a")]),
            MettaValue::sym("b"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("a"));
        assert_eq!(bindings.get("$y").unwrap().as_atom(), Some("b"));
    }

    #[test]
    fn test_unify_arity_mismatch() {
        let lhs = sexpr(vec![MettaValue::sym("a"), MettaValue::sym("b")]);
        let rhs = sexpr(vec![
            MettaValue::sym("a"),
            MettaValue::sym("b"),
            MettaValue::sym("c"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_empty_sexpr() {
        let lhs = sexpr(vec![]);
        let rhs = sexpr(vec![]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_ground_types() {
        // Long
        assert!(bidirectional_unify_generic(&MettaValue::Long(42), &MettaValue::Long(42)).is_some());
        assert!(bidirectional_unify_generic(&MettaValue::Long(42), &MettaValue::Long(43)).is_none());

        // Bool
        assert!(bidirectional_unify_generic(&MettaValue::Bool(true), &MettaValue::Bool(true)).is_some());
        assert!(bidirectional_unify_generic(&MettaValue::Bool(true), &MettaValue::Bool(false)).is_none());

        // String
        assert!(bidirectional_unify_generic(
            &MettaValue::String("hi".to_string()),
            &MettaValue::String("hi".to_string()),
        ).is_some());
        assert!(bidirectional_unify_generic(
            &MettaValue::String("hi".to_string()),
            &MettaValue::String("bye".to_string()),
        ).is_none());
    }

    #[test]
    fn test_unify_type_mismatch() {
        // Long vs Bool
        assert!(bidirectional_unify_generic(&MettaValue::Long(1), &MettaValue::Bool(true)).is_none());
        // Atom vs Long
        assert!(bidirectional_unify_generic(&MettaValue::sym("a"), &MettaValue::Long(1)).is_none());
    }

    #[test]
    fn test_unify_transitive_deref() {
        // ($x $y $z) vs (a $x $y) => {$x->a, $y->a, $z->a}
        let lhs = sexpr(vec![
            MettaValue::var("x"),
            MettaValue::var("y"),
            MettaValue::var("z"),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("a"),
            MettaValue::var("x"),
            MettaValue::var("y"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some(), "Transitive deref unification should succeed");
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("a"));
        // $y is bound to $x, which deref's to a — consistency check should pass
    }

    #[test]
    fn test_unify_deep_nesting() {
        // Build a deeply nested term: (f (f (f ... (f $x) ...)))
        let mut lhs = MettaValue::var("x");
        let mut rhs = MettaValue::sym("leaf");
        for _ in 0..100 {
            lhs = sexpr(vec![MettaValue::sym("f"), lhs]);
            rhs = sexpr(vec![MettaValue::sym("f"), rhs]);
        }
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some(), "Deep nesting should not overflow stack");
        assert_eq!(result.unwrap().get("$x").unwrap().as_atom(), Some("leaf"));
    }

    #[test]
    fn test_unify_unit_empty_sexpr_cross_type() {
        let unit = MettaValue::Unit();
        let empty = sexpr(vec![]);
        assert!(bidirectional_unify_generic(&unit, &empty).is_some());
        assert!(bidirectional_unify_generic(&empty, &unit).is_some());
    }

    #[test]
    fn test_unify_variable_to_variable() {
        // $x vs $y => Some({$x -> $y}) or Some({$y -> $x})
        let x = MettaValue::var("x");
        let y = MettaValue::var("y");
        let result = bidirectional_unify_generic(&x, &y);
        assert!(result.is_some());
        let bindings = result.unwrap();
        // One of them should be bound
        assert_eq!(bindings.len(), 1);
    }

    #[test]
    fn test_unify_same_variable_both_sides() {
        // $x vs $x => Some({}) (trivially equal after deref)
        let x = MettaValue::var("x");
        let result = bidirectional_unify_generic(&x, &x);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_space_ref_not_variable() {
        // &self is a space reference, not a variable
        let factory = GcFactory::default();
        let lhs = factory.atom("&self");
        let rhs = MettaValue::sym("foo");
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_none(), "&self should not be treated as a variable");
    }

    #[test]
    fn test_unify_complex_bidirectional() {
        // (f $x (g $y)) vs (f (h $y) (g a))
        // => {$x -> (h a), $y -> a}  (with transitive deref)
        let lhs = sexpr(vec![
            MettaValue::sym("f"),
            MettaValue::var("x"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("y")]),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("h"), MettaValue::var("y")]),
            sexpr(vec![MettaValue::sym("g"), MettaValue::sym("a")]),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$y").unwrap().as_atom(), Some("a"));
        // $x should be bound to (h $y) where $y -> a
        // The binding stores the value at bind-time: (h $y)
        // Deref'd, it represents (h a)
        assert!(bindings.get("$x").is_some());
    }

    #[test]
    fn test_is_unification_variable() {
        assert!(is_unification_variable("$x"));
        assert!(is_unification_variable("$foo"));
        assert!(is_unification_variable("&x"));
        assert!(is_unification_variable("'y"));
        assert!(!is_unification_variable("&"));
        assert!(!is_unification_variable("&self"));
        assert!(!is_unification_variable("&kb"));
        assert!(!is_unification_variable("&stack"));
        assert!(!is_unification_variable("foo"));
        assert!(!is_unification_variable("_"));
        assert!(!is_unification_variable("42"));
    }

    #[test]
    fn test_occurs_in_simple() {
        let term = sexpr(vec![MettaValue::sym("f"), MettaValue::var("x")]);
        let bindings = GenericBindings::new();
        assert!(occurs_in_generic("$x", &term, &bindings));
        assert!(!occurs_in_generic("$y", &term, &bindings));
    }

    #[test]
    fn test_occurs_in_through_bindings() {
        // $y is bound to (g $x). Does $x occur in $y?
        let mut bindings = GenericBindings::new();
        let factory = GcFactory::default();
        bindings.insert(
            factory.atom("$y").as_atom().expect("atom"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("x")]),
        );
        let term = MettaValue::var("y");
        assert!(occurs_in_generic("$x", &term, &bindings));
    }

    #[test]
    fn test_deref_value_owned_chain() {
        // $x -> $y -> $z -> 42
        let factory = GcFactory::default();
        let mut bindings = GenericBindings::new();
        bindings.insert(factory.atom("$x").as_atom().expect("atom"), MettaValue::var("y"));
        bindings.insert(factory.atom("$y").as_atom().expect("atom"), MettaValue::var("z"));
        bindings.insert(factory.atom("$z").as_atom().expect("atom"), MettaValue::Long(42));

        let result = deref_value_owned(&MettaValue::var("x"), &bindings);
        assert_eq!(result.as_long(), Some(42));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::backend::models::MettaValue;
    use proptest::prelude::*;

    // =========================================================================
    // Strategy Generators
    // =========================================================================

    /// Generate arbitrary Long values
    fn arb_long() -> impl Strategy<Value = MettaValue> {
        (-10000i64..10000i64).prop_map(MettaValue::Long)
    }

    /// Generate arbitrary Bool values
    fn arb_bool() -> impl Strategy<Value = MettaValue> {
        prop::bool::ANY.prop_map(MettaValue::Bool)
    }

    /// Generate arbitrary Symbol values (non-variable atoms)
    fn arb_symbol() -> impl Strategy<Value = MettaValue> {
        "[a-z]{1,8}".prop_map(|s| MettaValue::sym(&s))
    }

    /// Generate arbitrary String values
    fn arb_string() -> impl Strategy<Value = MettaValue> {
        ".{0,20}".prop_map(MettaValue::String)
    }

    /// Generate simple ground values (no variables, no S-expressions)
    fn arb_simple_ground() -> impl Strategy<Value = MettaValue> {
        prop_oneof![
            arb_long(),
            arb_bool(),
            arb_symbol(),
            arb_string(),
            Just(MettaValue::Unit()),
        ]
    }

    /// Generate ground MettaValue (no variables) with bounded depth
    fn arb_ground_value(depth: usize) -> BoxedStrategy<MettaValue> {
        if depth == 0 {
            arb_simple_ground().boxed()
        } else {
            prop_oneof![
                arb_simple_ground(),
                prop::collection::vec(arb_ground_value(depth - 1), 0..4)
                    .prop_map(MettaValue::SExpr),
            ]
            .boxed()
        }
    }

    /// Generate arbitrary MettaValue including variables, with bounded depth
    fn arb_metta_value(depth: usize) -> BoxedStrategy<MettaValue> {
        if depth == 0 {
            prop_oneof![
                arb_simple_ground(),
                "[a-e]{1,3}".prop_map(|s| MettaValue::var(&s)),
            ]
            .boxed()
        } else {
            prop_oneof![
                arb_simple_ground(),
                "[a-e]{1,3}".prop_map(|s| MettaValue::var(&s)),
                prop::collection::vec(arb_metta_value(depth - 1), 0..4)
                    .prop_map(MettaValue::SExpr),
            ]
            .boxed()
        }
    }

    /// Check if a MettaValue contains any variables
    fn is_ground(v: &MettaValue) -> bool {
        let mut stack: Vec<&MettaValue> = vec![v];
        while let Some(current) = stack.pop() {
            if let Some(name) = current.as_atom() {
                if is_unification_variable(name) {
                    return false;
                }
            }
            if let Some(items) = current.as_sexpr() {
                for item in items.iter() {
                    stack.push(item);
                }
            }
        }
        true
    }

    // =========================================================================
    // Property Tests
    // =========================================================================

    proptest! {
        /// Reflexivity: any term unifies with itself
        #[test]
        fn unify_reflexive(v in arb_metta_value(3)) {
            let result = bidirectional_unify_generic(&v, &v);
            prop_assert!(result.is_some(), "Any term should unify with itself: {:?}", v);
        }

        /// Symmetry: unify(a,b) succeeds iff unify(b,a) succeeds
        #[test]
        fn unify_symmetric(a in arb_metta_value(2), b in arb_metta_value(2)) {
            let ab = bidirectional_unify_generic(&a, &b);
            let ba = bidirectional_unify_generic(&b, &a);
            prop_assert_eq!(
                ab.is_some(), ba.is_some(),
                "Unification should be symmetric: a={:?}, b={:?}", a, b
            );
        }

        /// Ground terms: unify(ground_a, ground_b) = Some({}) iff a == b structurally
        #[test]
        fn unify_ground_terms(a in arb_ground_value(2), b in arb_ground_value(2)) {
            // Only test values we know are ground
            prop_assume!(is_ground(&a) && is_ground(&b));
            let result = bidirectional_unify_generic(&a, &b);
            if a == b {
                prop_assert!(result.is_some(), "Equal ground terms should unify");
                prop_assert!(result.unwrap().is_empty(), "Ground unification should produce no bindings");
            } else {
                prop_assert!(result.is_none(), "Unequal ground terms should not unify: {:?} vs {:?}", a, b);
            }
        }

        /// Variable consistency: ($x $x) only unifies with (a b) when a == b
        #[test]
        fn variable_consistency(a in arb_ground_value(2), b in arb_ground_value(2)) {
            prop_assume!(is_ground(&a) && is_ground(&b));
            let pattern = MettaValue::SExpr(vec![MettaValue::var("x"), MettaValue::var("x")]);
            let value = MettaValue::SExpr(vec![a.clone(), b.clone()]);
            let result = bidirectional_unify_generic(&pattern, &value);
            if a == b {
                prop_assert!(result.is_some(), "($x $x) should unify with ({:?} {:?})", a, b);
            } else {
                prop_assert!(result.is_none(), "($x $x) should NOT unify with ({:?} {:?})", a, b);
            }
        }

        /// Wildcard universality: _ unifies with any term (from either side)
        #[test]
        fn wildcard_matches_anything(v in arb_metta_value(3)) {
            let wildcard = MettaValue::sym("_");
            prop_assert!(
                bidirectional_unify_generic(&wildcard, &v).is_some(),
                "Wildcard should match anything on LHS"
            );
            prop_assert!(
                bidirectional_unify_generic(&v, &wildcard).is_some(),
                "Wildcard should match anything on RHS"
            );
        }

        /// Idempotence: unifying a term with itself produces empty or identity bindings
        #[test]
        fn unify_self_produces_no_new_info(v in arb_ground_value(3)) {
            prop_assume!(is_ground(&v));
            let result = bidirectional_unify_generic(&v, &v);
            prop_assert!(result.is_some());
            prop_assert!(result.unwrap().is_empty(), "Self-unification of ground term should produce no bindings");
        }

        /// Occurs check: $x never unifies with S-expr containing $x
        #[test]
        fn occurs_check_prevents_infinite_terms(
            wrapper_head in "[a-z]{1,3}".prop_map(|s| MettaValue::sym(&s)),
            extra_args in prop::collection::vec(arb_ground_value(1), 0..3),
        ) {
            let var = MettaValue::var("occ");
            let mut children = vec![wrapper_head];
            children.extend(extra_args);
            children.push(MettaValue::var("occ"));
            let term = MettaValue::SExpr(children);

            let result = bidirectional_unify_generic(&var, &term);
            prop_assert!(result.is_none(), "Occurs check should prevent $occ = {:?}", term);
        }

        /// Single variable binding: $x vs ground_value always succeeds
        #[test]
        fn single_var_binds_to_ground(v in arb_ground_value(2)) {
            prop_assume!(is_ground(&v));
            let var = MettaValue::var("single");
            let result = bidirectional_unify_generic(&var, &v);
            prop_assert!(result.is_some());
            let bindings = result.unwrap();
            prop_assert_eq!(bindings.len(), 1);
            prop_assert_eq!(bindings.get("$single").unwrap(), &v);
        }
    }
}
