//! Generic Type Operations
//!
//! This module provides generic implementations of type operations that work
//! with any value type implementing `MettaValueTrait`. These are used by the
//! generic evaluation engine to avoid conversions between value types.
//!
//! ## Design
//!
//! Each operation:
//! - Takes generic `&[V]` items and a factory for constructing results
//! - Returns `Vec<V>` results
//! - Uses `MettaValueTrait` methods for type checking
//! - Uses `MettaValueFactory` for constructing new values

use crate::backend::builtin_signatures::{get_return_type, get_signature, TypeExpr};
use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Infer the type of an expression (generic version)
///
/// Uses the `MettaValueTrait` to inspect values and returns a type atom.
/// This is equivalent to `infer_type` but works with any value type.
pub fn infer_type_generic<V, F>(expr: &V, factory: &F, env: &GenericEnvironment<V, F>) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // Ground types have built-in types
    if expr.is_bool() {
        return factory.atom("Bool");
    }
    if expr.is_long() {
        return factory.atom("Number");
    }
    if expr.is_float() {
        return factory.atom("Number");
    }
    if expr.is_string() {
        return factory.atom("String");
    }
    if expr.is_unit() {
        return factory.atom("Expression");
    }

    // Type values have type Type
    if expr.is_type() {
        return factory.atom("Type");
    }

    // Errors have Error type
    if expr.is_error() {
        return factory.atom("Error");
    }

    // Space references have Space type
    if expr.is_space() {
        return factory.atom("Space");
    }

    // State references have State type
    if expr.is_state() {
        return factory.atom("State");
    }

    // Memo tables have Memo type
    if expr.is_memo() {
        return factory.atom("Memo");
    }

    // Empty sentinel has Empty type
    if expr.is_empty() {
        return factory.atom("Empty");
    }

    // For atoms, look up in environment or return Undefined
    if let Some(name) = expr.as_atom() {
        // Check if it's a variable (starts with $, &, or ')
        if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
            // Type variable - return as-is wrapped in Type
            return factory.type_value(factory.atom(name));
        }

        // Look up type in environment - get_type returns Option<V> directly
        if let Some(typ) = env.get_type_generic(name) {
            return typ;
        }

        return factory.atom("Undefined");
    }

    // For s-expressions, try to infer from function application
    if let Some(items) = expr.as_sexpr() {
        if items.is_empty() {
            return factory.atom("Expression");
        }

        // Get the operator/function
        if let Some(op) = items.first().and_then(|v| v.as_atom()) {
            // Check the built-in signature registry
            if let Some(sig) = get_signature(op) {
                if let Some(ret_type) = get_return_type(&sig.type_sig) {
                    return type_expr_to_generic(ret_type, factory);
                }
            }

            // Special case for arrow type constructor
            if op == "->" {
                return factory.atom("Type");
            }

            // Look up function type in environment (user-defined types)
            // get_type returns Option<V> directly
            if let Some(generic_type) = env.get_type_generic(op) {
                // Extract return type from arrow type
                if let Some(type_items) = generic_type.as_sexpr() {
                    if let Some(arrow) = type_items.first().and_then(|v| v.as_atom()) {
                        if arrow == "->" && type_items.len() > 1 {
                            // Return type is last element
                            if let Some(last) = type_items.last() {
                                return last.clone();
                            }
                        }
                    }
                }
                return generic_type;
            }
        }

        // Can't infer type
        return factory.atom("Undefined");
    }

    // For conjunctions, type is the type of the last goal
    if let Some(goals) = expr.as_conjunction() {
        if goals.is_empty() {
            return factory.atom("Expression");
        }
        if let Some(last) = goals.last() {
            return infer_type_generic(last, factory, env);
        }
    }

    // Default: Undefined
    factory.atom("Undefined")
}

/// Convert a TypeExpr from the signature registry to a generic value
fn type_expr_to_generic<V, F>(type_expr: &TypeExpr, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    match type_expr {
        TypeExpr::Number => factory.atom("Number"),
        TypeExpr::Bool => factory.atom("Bool"),
        TypeExpr::String => factory.atom("String"),
        TypeExpr::Atom => factory.atom("Atom"),
        TypeExpr::Space => factory.atom("Space"),
        TypeExpr::State => factory.atom("State"),
        TypeExpr::Unit => factory.atom("Unit"),
        TypeExpr::Error => factory.atom("Error"),
        TypeExpr::Type => factory.atom("Type"),
        TypeExpr::Any => factory.atom("Any"),
        TypeExpr::Pattern => factory.atom("Pattern"),
        TypeExpr::Bindings => factory.atom("Bindings"),
        TypeExpr::Expr => factory.atom("Expr"),
        TypeExpr::Var(name) => factory.atom(&format!("${}", name)),
        TypeExpr::List(elem) => {
            let elem_type = type_expr_to_generic(elem, factory);
            factory.sexpr(vec![factory.atom("List"), elem_type])
        }
        TypeExpr::Arrow(args, ret) => {
            let mut items = vec![factory.atom("->")];
            for arg in args {
                items.push(type_expr_to_generic(arg, factory));
            }
            items.push(type_expr_to_generic(ret, factory));
            factory.sexpr(items)
        }
    }
}

/// Check if two types match (generic version)
///
/// Handles type variables and structural equality.
fn types_match_generic<V: MettaValueTrait>(actual: &V, expected: &V) -> bool {
    // Type variables match anything
    if let Some(name) = expected.as_atom() {
        if name.starts_with('$') {
            return true;
        }
    }
    if let Some(name) = actual.as_atom() {
        if name.starts_with('$') {
            return true;
        }
    }

    // Type variables in Type wrapper
    if expected.is_type() {
        if let Some(inner) = expected.as_type() {
            if let Some(name) = inner.as_atom() {
                if name.starts_with('$') {
                    return true;
                }
            }
            // Otherwise, unwrap and compare
            if actual.is_type() {
                if let Some(actual_inner) = actual.as_type() {
                    return types_match_generic(actual_inner, inner);
                }
            }
        }
        return false;
    }

    // Exact atom matches
    if let (Some(a), Some(e)) = (actual.as_atom(), expected.as_atom()) {
        return a == e;
    }

    // Bool matches
    if let (Some(a), Some(e)) = (actual.as_bool(), expected.as_bool()) {
        return a == e;
    }

    // Long matches
    if let (Some(a), Some(e)) = (actual.as_long(), expected.as_long()) {
        return a == e;
    }

    // String matches
    if let (Some(a), Some(e)) = (actual.as_string(), expected.as_string()) {
        return a == e;
    }

    // S-expression matches (structural equality)
    if let (Some(a_items), Some(e_items)) = (actual.as_sexpr(), expected.as_sexpr()) {
        if a_items.len() != e_items.len() {
            return false;
        }
        return a_items
            .iter()
            .zip(e_items.iter())
            .all(|(a, e)| types_match_generic(a, e));
    }

    // Unit matches Unit
    if actual.is_unit() && expected.is_unit() {
        return true;
    }

    // Default: no match
    false
}

/// get-type: Return the type of an expression (generic version)
/// (get-type expr) -> Type
pub fn eval_get_type_generic<V, F>(items: &[V], factory: &F, env: &GenericEnvironment<V, F>) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() != 2 {
        return vec![factory.error(
            &format!(
                "get-type requires exactly 1 argument, got {}. Usage: (get-type expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];
    let typ = infer_type_generic(expr, factory, env);
    vec![typ]
}

/// check-type: Check if expression has expected type (generic version)
/// (check-type expr expected-type) -> Bool
pub fn eval_check_type_generic<V, F>(items: &[V], factory: &F, env: &GenericEnvironment<V, F>) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() != 3 {
        return vec![factory.error(
            &format!(
                "check-type requires exactly 2 arguments, got {}. Usage: (check-type expr type)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];
    let expected = &items[2];

    let actual = infer_type_generic(expr, factory, env);
    let matches = types_match_generic(&actual, expected);

    vec![factory.bool(matches)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::HeapEnvironment;
    use crate::backend::models::{HeapMettaValueFactory, MettaValue};

    #[test]
    fn test_infer_type_generic_ground_types() {
        let factory = HeapMettaValueFactory;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        // Bool
        let value = MettaValue::Bool(true);
        let typ = infer_type_generic(&value, &factory, &env);
        assert_eq!(typ.as_atom(), Some("Bool"));

        // Long
        let value = MettaValue::Long(42);
        let typ = infer_type_generic(&value, &factory, &env);
        assert_eq!(typ.as_atom(), Some("Number"));

        // String
        let value = MettaValue::String("hello".to_string());
        let typ = infer_type_generic(&value, &factory, &env);
        assert_eq!(typ.as_atom(), Some("String"));
    }

    #[test]
    fn test_get_type_generic() {
        let factory = HeapMettaValueFactory;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        let items = vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Long(42),
        ];
        let result = eval_get_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_atom(), Some("Number"));
    }

    #[test]
    fn test_check_type_generic() {
        let factory = HeapMettaValueFactory;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        // (check-type 42 Number) -> true
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("Number".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));

        // (check-type 42 String) -> false
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("String".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(false));
    }

    #[test]
    fn test_check_type_with_type_variable() {
        let factory = HeapMettaValueFactory;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        // (check-type 42 $t) -> true (type variable matches anything)
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$t".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }
}
