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
//! - `pattern_specificity_generic` - Compute pattern specificity for rule ordering
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

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

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

    // Type variants do NOT recurse (matching heap behavior in helpers.rs)
    // Types are returned as-is without substitution
    if value.is_type() {
        return value.clone();
    }

    // Handle S-expressions - recursively apply bindings
    if let Some(items) = value.as_sexpr() {
        let new_items: Vec<V> = items
            .iter()
            .map(|item| apply_bindings_generic(item, bindings, factory))
            .collect();
        return factory.sexpr(new_items);
    }

    // Handle conjunctions
    if let Some(goals) = value.as_conjunction() {
        let new_goals: Vec<V> = goals
            .iter()
            .map(|goal| apply_bindings_generic(goal, bindings, factory))
            .collect();
        return factory.conjunction(new_goals);
    }

    // Handle errors
    if let Some((msg, details)) = value.as_error() {
        let new_details = apply_bindings_generic(details, bindings, factory);
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
            bindings.insert(pattern_name.to_string(), value.clone());
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

/// Compute the specificity of a generic pattern (lower is more specific).
///
/// More specific patterns have fewer variables. This enables prioritizing
/// more specific rule matches over general ones.
///
/// # Specificity Scoring
///
/// - Variables ($x, &y, 'z, _): +1000 each (except standalone "&")
/// - Literals (atoms, numbers, bools, strings): +0 each
/// - Compound types: sum of children's specificities
///
/// # Example
///
/// ```ignore
/// // Pattern ($x $y) has specificity 2000
/// // Pattern (foo $x) has specificity 1000
/// // Pattern (foo bar) has specificity 0 (most specific)
/// ```
pub fn pattern_specificity_generic<V: MettaValueTrait>(pattern: &V) -> usize {
    let mut work_stack: Vec<&V> = Vec::with_capacity(16);
    work_stack.push(pattern);
    let mut total: usize = 0;

    while let Some(val) = work_stack.pop() {
        // Check if it's an atom variable
        if let Some(name) = val.as_atom() {
            // Variables are least specific
            // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
            if (name.starts_with('$')
                || name.starts_with('&')
                || name.starts_with('\'')
                || name == "_")
                && name != "&"
            {
                total += 1000;
            }
            // Literals contribute 0 (most specific)
            continue;
        }

        // Ground types contribute 0
        if val.is_bool() || val.is_long() || val.is_float() || val.is_string() || val.is_unit() {
            continue;
        }

        // Compound types: push children onto work stack
        if let Some(items) = val.as_sexpr() {
            for item in items {
                work_stack.push(item);
            }
            continue;
        }

        if let Some(goals) = val.as_conjunction() {
            for goal in goals {
                work_stack.push(goal);
            }
            continue;
        }

        if let Some((_, details)) = val.as_error() {
            work_stack.push(details);
            continue;
        }

        if let Some(inner) = val.as_type() {
            work_stack.push(inner);
            continue;
        }

        // Other types (space, state, unit, memo, empty) contribute 0
    }

    total
}

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
pub fn try_match_all_rules_generic<V, F>(
    expr: &V,
    env: &GenericEnvironment<V, F>,
    _factory: F,
) -> Vec<(V, GenericBindings<V>)>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    // Use native byte-level matching via RuleIndex + extract_data.
    // This replaces the old pipeline of:
    //   get_matching_rules_for_expr → pattern_match_generic → apply_bindings_generic
    let results = env.match_rules_native(expr, apply_bindings_generic);
    results
        .into_iter()
        .map(|r| (r.instantiated_rhs, r.bindings))
        .collect()
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
        bindings.insert("$x".to_string(), MettaValue::Long(42));

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
        bindings.insert("&".to_string(), MettaValue::Long(42));

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
        bindings.insert("$x".to_string(), MettaValue::Long(42));

        // Type wrapping a variable - should not substitute
        let template = MettaValue::Type(MettaValue::Atom("$x".to_string()));
        let result = apply_bindings_generic(&template, &bindings, &factory);

        // Result should still be a Type with $x inside (not substituted)
        assert!(result.is_type());
        let inner = result.as_type().expect("should be type");
        assert_eq!(inner.as_atom(), Some("$x"));
    }
}
