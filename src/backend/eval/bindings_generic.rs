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
        Process(&'a V),
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
                            result_stack.push(bound.clone());
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
}
