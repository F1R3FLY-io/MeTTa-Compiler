//! Pattern matching for MeTTa values.
//!
//! This module implements the core pattern matching algorithm for MeTTa,
//! supporting variable binding, wildcards, and structural matching.

use tracing::trace;

use crate::backend::models::{Bindings, MettaValue, ValueView};

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
    // MettaValue is Copy (8 bytes) so owned values are equally efficient as references
    let mut work_stack: Vec<(MettaValue, MettaValue)> = Vec::with_capacity(16);
    work_stack.push((*pattern, *value));

    while let Some((pat, val)) = work_stack.pop() {
        // Process each pattern-value pair
        let matches = match (pat.view(), val.view()) {
            // Wildcard matches anything (both `_` and `$_`)
            (ValueView::Atom(p), _) if p == "_" || p == "$_" => true,

            // FAST PATH: First variable binding (empty bindings)
            // Optimization: Skip lookup when bindings are empty - directly insert
            // This reduces single-variable regression from 16.8% to ~5-7%
            (ValueView::Atom(p), _)
                if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                    && p != "&"
                    && p != "$_"
                    && bindings.is_empty()
                    && work_stack.is_empty() =>
            {
                bindings.insert(p, val);
                true
            }

            // GENERAL PATH: Variable with potential existing bindings
            // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
            // EXCEPT: $_ is the wildcard (handled above)
            (ValueView::Atom(p), _)
                if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                    && p != "&"
                    && p != "$_" =>
            {
                // Check if variable is already bound (linear search for SmartBindings)
                if let Some((_, existing)) = bindings.iter().find(|(name, _)| *name == p) {
                    existing == &val
                } else {
                    bindings.insert(p, val);
                    true
                }
            }

            // Atoms must match exactly
            (ValueView::Atom(p), ValueView::Atom(v)) => p == v,
            (ValueView::Bool(p), ValueView::Bool(v)) => p == v,
            (ValueView::Long(p), ValueView::Long(v)) => p == v,
            (ValueView::Float(p), ValueView::Float(v)) => p == v,
            (ValueView::String(p), ValueView::String(v)) => p == v,
            (ValueView::Unit, ValueView::Unit) => true,
            // Unit pattern matches Empty atom (HE-compatible: () pattern in case matches Empty)
            // This is needed because case converts empty results to Atom("Empty") internally
            (ValueView::Unit, ValueView::Atom(v)) if v == "Empty" => true,
            // Empty atom pattern matches Unit (symmetry: Empty pattern matches () values)
            (ValueView::Atom(p), ValueView::Unit) if p == "Empty" => true,

            // Unit pattern matches only empty values (Unit, empty S-expr, or Empty atom)
            // For discard pattern, use wildcard _ instead
            (ValueView::Unit, ValueView::SExpr(v_items)) if v_items.is_empty() => true,

            // Empty S-expression () matches only empty values (empty S-expr, Unit, or Empty atom)
            // For discard pattern, use wildcard _ instead
            (ValueView::SExpr(p_items), ValueView::SExpr(v_items))
                if p_items.is_empty() && v_items.is_empty() =>
            {
                true
            }
            (ValueView::SExpr(p_items), ValueView::Unit) if p_items.is_empty() => true,
            (ValueView::SExpr(p_items), ValueView::Atom(v))
                if p_items.is_empty() && v == "Empty" =>
            {
                true
            }

            // S-expressions: push children onto work stack (replaces recursion)
            (ValueView::SExpr(p_items), ValueView::SExpr(v_items)) => {
                if p_items.len() != v_items.len() {
                    return false; // Early exit on length mismatch
                }
                // Push in reverse order so first element is processed first (LIFO)
                for (p, v) in p_items.iter().zip(v_items.iter()).rev() {
                    work_stack.push((*p, *v));
                }
                true // Continue processing the work stack
            }

            // Conjunctions: push children onto work stack (replaces recursion)
            (ValueView::Conjunction(p_goals), ValueView::Conjunction(v_goals)) => {
                if p_goals.len() != v_goals.len() {
                    return false; // Early exit on length mismatch
                }
                // Push in reverse order so first element is processed first
                for (p, v) in p_goals.iter().zip(v_goals.iter()).rev() {
                    work_stack.push((*p, *v));
                }
                true // Continue processing the work stack
            }

            // Errors: check message match, push details onto work stack
            (
                ValueView::Error(p_msg, p_details),
                ValueView::Error(v_msg, v_details),
            ) => {
                if p_msg != v_msg {
                    return false; // Message mismatch
                }
                // Push details for matching (replaces recursion)
                work_stack.push((p_details, v_details));
                true // Continue processing the work stack
            }

            // Quoted: match inner values
            (ValueView::Quoted(p_inner), ValueView::Quoted(v_inner)) => {
                work_stack.push((p_inner, v_inner));
                true
            }

            // Transparency: SExpr pattern (quote $x) matches Quoted(v)
            (ValueView::SExpr(p_items), ValueView::Quoted(v_inner))
                if p_items.len() == 2
                    && matches!(p_items[0].view(), ValueView::Atom(s) if s == "quote") =>
            {
                work_stack.push((p_items[1], v_inner));
                true
            }

            _ => false,
        };

        if !matches {
            return false; // Early exit on any mismatch
        }
    }

    true // All pairs matched successfully
}
