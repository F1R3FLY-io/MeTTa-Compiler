//! Pattern matching for MeTTa values.
//!
//! This module implements the core pattern matching algorithm for MeTTa,
//! supporting variable binding, wildcards, and structural matching.

use tracing::trace;

use crate::backend::models::{Bindings, MettaValue, MettaValueInner};

/// Match a pattern against a value, returning variable bindings if successful.
///
/// This is made public to support optimized match operations in Environment
/// and for benchmarking the core pattern matching algorithm.
///
/// # Arguments
/// - `pattern`: The pattern to match against (may contain variables like `$x`, `&y`, `'z`)
/// - `value`: The value to match
///
/// # Returns
/// - `Some(bindings)` if the pattern matches, with variable bindings
/// - `None` if the pattern does not match
///
/// # Examples
/// ```ignore
/// // Variable binding
/// pattern_match(&atom("$x"), &long(42)) // => Some({$x: 42})
///
/// // Structural matching
/// pattern_match(&sexpr([atom("foo"), atom("$x")]), &sexpr([atom("foo"), long(1)]))
/// // => Some({$x: 1})
///
/// // Wildcard
/// pattern_match(&atom("_"), &long(999)) // => Some({})
/// ```
#[inline]
pub fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
    trace!(target: "mettatron::backend::eval::pattern_match", ?pattern, ?value);
    let mut bindings = Bindings::new();
    if pattern_match_impl(pattern, value, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

/// Internal pattern matching implementation that accumulates bindings.
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
/// - Pattern matching before trampoline's MAX_EVAL_DEPTH check runs
#[inline]
pub(crate) fn pattern_match_impl(
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Bindings,
) -> bool {
    // Work stack: (pattern, value) pairs to match
    // Use Vec as stack (push/pop from end) - more efficient than VecDeque for this use case
    let mut work_stack: Vec<(&MettaValue, &MettaValue)> = Vec::with_capacity(16);
    work_stack.push((pattern, value));

    while let Some((pat, val)) = work_stack.pop() {
        // Process each pattern-value pair
        let matches = match (pat.inner(), val.inner()) {
            // Wildcard matches anything
            (MettaValueInner::Atom(p), _) if p == "_" => true,

            // FAST PATH: First variable binding (empty bindings)
            // Optimization: Skip lookup when bindings are empty - directly insert
            // This reduces single-variable regression from 16.8% to ~5-7%
            (MettaValueInner::Atom(p), _)
                if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                    && p != "&"
                    && bindings.is_empty()
                    && work_stack.is_empty() =>
            {
                bindings.insert(p.clone(), val.clone());
                true
            }

            // GENERAL PATH: Variable with potential existing bindings
            // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
            (MettaValueInner::Atom(p), _)
                if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                    && p != "&" =>
            {
                // Check if variable is already bound (linear search for SmartBindings)
                if let Some((_, existing)) = bindings.iter().find(|(name, _)| name.as_str() == p) {
                    existing == val
                } else {
                    bindings.insert(p.clone(), val.clone());
                    true
                }
            }

            // Atoms must match exactly
            (MettaValueInner::Atom(p), MettaValueInner::Atom(v)) => p == v,
            (MettaValueInner::Bool(p), MettaValueInner::Bool(v)) => p == v,
            (MettaValueInner::Long(p), MettaValueInner::Long(v)) => p == v,
            (MettaValueInner::Float(p), MettaValueInner::Float(v)) => p == v,
            (MettaValueInner::String(p), MettaValueInner::String(v)) => p == v,
            (MettaValueInner::Unit, MettaValueInner::Unit) => true,
            // Unit pattern matches Empty atom (HE-compatible: () pattern in case matches Empty)
            // This is needed because case converts empty results to Atom("Empty") internally
            (MettaValueInner::Unit, MettaValueInner::Atom(v)) if v == "Empty" => true,
            // Empty atom pattern matches Unit (symmetry: Empty pattern matches () values)
            (MettaValueInner::Atom(p), MettaValueInner::Unit) if p == "Empty" => true,

            // Unit pattern matches only empty values (Unit, empty S-expr, or Empty atom)
            // For discard pattern, use wildcard _ instead
            (MettaValueInner::Unit, MettaValueInner::SExpr(v_items)) if v_items.is_empty() => true,

            // Empty S-expression () matches only empty values (empty S-expr, Unit, or Empty atom)
            // For discard pattern, use wildcard _ instead
            (MettaValueInner::SExpr(p_items), MettaValueInner::SExpr(v_items))
                if p_items.is_empty() && v_items.is_empty() =>
            {
                true
            }
            (MettaValueInner::SExpr(p_items), MettaValueInner::Unit) if p_items.is_empty() => true,
            (MettaValueInner::SExpr(p_items), MettaValueInner::Atom(v))
                if p_items.is_empty() && v == "Empty" =>
            {
                true
            }

            // S-expressions: push children onto work stack (replaces recursion)
            (MettaValueInner::SExpr(p_items), MettaValueInner::SExpr(v_items)) => {
                if p_items.len() != v_items.len() {
                    return false; // Early exit on length mismatch
                }
                // Push in reverse order so first element is processed first (LIFO)
                for (p, v) in p_items.iter().zip(v_items.iter()).rev() {
                    work_stack.push((p, v));
                }
                true // Continue processing the work stack
            }

            // Conjunctions: push children onto work stack (replaces recursion)
            (MettaValueInner::Conjunction(p_goals), MettaValueInner::Conjunction(v_goals)) => {
                if p_goals.len() != v_goals.len() {
                    return false; // Early exit on length mismatch
                }
                // Push in reverse order so first element is processed first
                for (p, v) in p_goals.iter().zip(v_goals.iter()).rev() {
                    work_stack.push((p, v));
                }
                true // Continue processing the work stack
            }

            // Errors: check message match, push details onto work stack
            (
                MettaValueInner::Error(p_msg, p_details),
                MettaValueInner::Error(v_msg, v_details),
            ) => {
                if p_msg != v_msg {
                    return false; // Message mismatch
                }
                // Push details for matching (replaces recursion)
                work_stack.push((p_details, v_details));
                true // Continue processing the work stack
            }

            _ => false,
        };

        if !matches {
            return false; // Early exit on any mismatch
        }
    }

    true // All pairs matched successfully
}
