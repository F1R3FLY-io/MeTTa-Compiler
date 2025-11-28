use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::sync::Arc;

/// Built-in type names with correct capitalization for "Did you mean?" suggestions
const TYPE_NAME_MAPPINGS: &[(&str, &str)] = &[
    ("bool", "Bool"),
    ("boolean", "Bool"),
    ("string", "String"),
    ("str", "String"),
    ("int", "Long"),
    ("integer", "Long"),
    ("long", "Long"),
    ("float", "Float"),
    ("double", "Float"),
    ("number", "Number"),
    ("num", "Number"),
    ("nil", "Nil"),
    ("null", "Nil"),
    ("none", "Nil"),
    ("atom", "Atom"),
    ("symbol", "Atom"),
    ("type", "Type"),
    ("error", "Error"),
];

/// Suggest correct type name capitalization if the given name is a common variant
pub(crate) fn suggest_type_name(name: &str) -> Option<String> {
    let lower = name.to_lowercase();

    for (incorrect, correct) in TYPE_NAME_MAPPINGS {
        if lower == *incorrect && name != *correct {
            return Some(format!(
                "Did you mean: {}? (type names are capitalized)",
                correct
            ));
        }
    }

    None
}

/// Type assertion: (: expr type)
/// Adds a type assertion to the environment
pub(super) fn eval_type_assertion(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!(":", items, 2, env, "(: expr type)");

    let expr = &items[1];
    let typ = items[2].clone();

    // Extract name from expression
    let name = match expr {
        MettaValue::Atom(s) => s.clone(),
        MettaValue::SExpr(expr_items) if !expr_items.is_empty() => {
            if let MettaValue::Atom(s) = &expr_items[0] {
                s.clone()
            } else {
                format!("{:?}", expr)
            }
        }
        _ => format!("{:?}", expr),
    };

    let mut new_env = env.clone();
    new_env.add_type(name, typ);

    // Add the type assertion to MORK Space
    let type_expr = MettaValue::SExpr(items);
    new_env.add_to_space(&type_expr);

    (vec![], new_env)
}

/// get-type: return the type of an expression
/// (get-type expr) -> Type
pub(super) fn eval_get_type(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("get-type", items, 1, env, "(get-type expr)");

    let expr = &items[1];
    let typ = infer_type(expr, &env);
    (vec![typ], env)
}

/// check-type: check if expression has expected type
/// (check-type expr expected-type) -> Bool
pub(super) fn eval_check_type(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("check-type", items, 2, env, "(check-type expr type)");

    let expr = &items[1];
    let expected = &items[2];

    let actual = infer_type(expr, &env);
    let matches = types_match(&actual, expected);

    (vec![MettaValue::Bool(matches)], env)
}

/// Infer the type of an expression
/// Returns a MettaValue representing the type
fn infer_type(expr: &MettaValue, env: &Environment) -> MettaValue {
    match expr {
        // Ground types have built-in types
        MettaValue::Bool(_) => MettaValue::Atom("Bool".to_string()),
        MettaValue::Long(_) => MettaValue::Atom("Number".to_string()),
        MettaValue::Float(_) => MettaValue::Atom("Number".to_string()),
        MettaValue::String(_) => MettaValue::Atom("String".to_string()),
        MettaValue::Nil => MettaValue::Atom("Nil".to_string()),

        // Type values have type Type
        MettaValue::Type(_) => MettaValue::Atom("Type".to_string()),

        // Errors have Error type
        MettaValue::Error(_, _) => MettaValue::Atom("Error".to_string()),

        // For atoms, look up in environment
        MettaValue::Atom(name) => {
            // Check if it's a variable (starts with $, &, or ')
            if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
                // Type variable - return as-is wrapped in Type
                return MettaValue::Type(Arc::new(MettaValue::Atom(name.clone())));
            }

            // Look up type in environment
            match env.get_type(name) {
                Some(typ) => typ,
                None => {
                    // Check if it looks like a mis-capitalized type name
                    if let Some(_suggestion) = suggest_type_name(name) {
                        // Return the suggestion hint in a special format
                        // Note: We could return a more detailed error here,
                        // but keeping Undefined for backward compatibility
                    }
                    MettaValue::Atom("Undefined".to_string())
                }
            }
        }

        // For s-expressions, try to infer from function application
        MettaValue::SExpr(items) => {
            if items.is_empty() {
                return MettaValue::Atom("Nil".to_string());
            }

            // Get the operator/function
            if let Some(MettaValue::Atom(op)) = items.first() {
                // Check for built-in operators (using symbols, not normalized names)
                match op.as_str() {
                    "+" | "-" | "*" | "/" => {
                        return MettaValue::Atom("Number".to_string());
                    }
                    "<" | "<=" | ">" | ">=" | "==" | "!=" => {
                        return MettaValue::Atom("Bool".to_string());
                    }
                    "->" => {
                        // Arrow type constructor
                        return MettaValue::Atom("Type".to_string());
                    }
                    _ => {
                        // Look up function type in environment
                        if let Some(func_type) = env.get_type(op) {
                            // If it's an arrow type, extract return type
                            if let MettaValue::SExpr(ref type_items) = func_type {
                                if let Some(MettaValue::Atom(arrow)) = type_items.first() {
                                    if arrow == "->" && type_items.len() > 1 {
                                        // Return type is last element
                                        return type_items.last().cloned().unwrap();
                                    }
                                }
                            }
                            return func_type;
                        }
                    }
                }
            }

            // Can't infer type
            MettaValue::Atom("Undefined".to_string())
        }

        // Conjunctions have Conjunction type
        // The type is the type of the last goal (final result)
        MettaValue::Conjunction(goals) => {
            if goals.is_empty() {
                // Empty conjunction has Nil type
                MettaValue::Atom("Nil".to_string())
            } else {
                // Type is the type of the last goal
                infer_type(goals.last().unwrap(), env)
            }
        }

        // Space references have Space type
        MettaValue::Space(_, _) => MettaValue::Atom("Space".to_string()),

        // Unit has Unit type
        MettaValue::Unit => MettaValue::Atom("Unit".to_string()),
    }
}

/// Check if two types match
/// Handles type variables and structural equality
fn types_match(actual: &MettaValue, expected: &MettaValue) -> bool {
    match (actual, expected) {
        // Type variables match anything
        (_, MettaValue::Atom(e)) if e.starts_with('$') => true,
        (MettaValue::Atom(a), _) if a.starts_with('$') => true,

        // Type variables in Type wrapper
        (_, MettaValue::Type(e)) => {
            if let MettaValue::Atom(name) = e.as_ref() {
                if name.starts_with('$') {
                    return true;
                }
            }
            // Otherwise, unwrap and compare
            if let MettaValue::Type(a) = actual {
                types_match(a, e)
            } else {
                false
            }
        }

        // Exact atom matches
        (MettaValue::Atom(a), MettaValue::Atom(e)) => a == e,

        // Bool matches
        (MettaValue::Bool(a), MettaValue::Bool(e)) => a == e,

        // Long matches
        (MettaValue::Long(a), MettaValue::Long(e)) => a == e,

        // String matches
        (MettaValue::String(a), MettaValue::String(e)) => a == e,

        // S-expression matches (structural equality)
        (MettaValue::SExpr(a_items), MettaValue::SExpr(e_items)) => {
            if a_items.len() != e_items.len() {
                return false;
            }
            a_items
                .iter()
                .zip(e_items.iter())
                .all(|(a, e)| types_match(a, e))
        }

        // Nil matches Nil
        (MettaValue::Nil, MettaValue::Nil) => true,

        // Default: no match
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::Rule;
    use crate::eval;

    #[test]
    fn test_type_assertion_missing_arguments() {
        let env = Environment::new();

        // (:) - missing both arguments
        let value = MettaValue::SExpr(vec![MettaValue::Atom(":".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains(":"));
                assert!(msg.contains("requires exactly 2 arguments")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_get_type_missing_argument() {
        let env = Environment::new();

        // (get-type) - missing argument
        let value = MettaValue::SExpr(vec![MettaValue::Atom("get-type".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("get-type"));
                assert!(msg.contains("requires exactly 1 argument")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_check_type_missing_arguments() {
        let env = Environment::new();

        // (check-type x) - missing type argument
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("check-type"));
                assert!(msg.contains("requires exactly 2 arguments")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_type_assertion() {
        let env = Environment::new();

        // (: x Number)
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);

        let (result, new_env) = eval(type_assertion, env);

        // Type assertion should return empty list
        assert!(result.is_empty());

        // Environment should have the type assertion
        assert_eq!(
            new_env.get_type("x"),
            Some(MettaValue::Atom("Number".to_string()))
        );
    }

    #[test]
    fn test_get_type_ground_types() {
        let env = Environment::new();

        // (get-type 42) -> Number
        let get_type_long = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Long(42),
        ]);
        let (result, _) = eval(get_type_long, env.clone());
        assert_eq!(result[0], MettaValue::Atom("Number".to_string()));

        // (get-type true) -> Bool
        let get_type_bool = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Bool(true),
        ]);
        let (result, _) = eval(get_type_bool, env.clone());
        assert_eq!(result[0], MettaValue::Atom("Bool".to_string()));

        // (get-type "hello") -> String
        let get_type_string = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::String("hello".to_string()),
        ]);
        let (result, _) = eval(get_type_string, env);
        assert_eq!(result[0], MettaValue::Atom("String".to_string()));
    }

    #[test]
    fn test_get_type_with_assertion() {
        let mut env = Environment::new();

        // Add type assertion: (: foo Number)
        env.add_type("foo".to_string(), MettaValue::Atom("Number".to_string()));

        // (get-type foo) -> Number
        let get_type = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Atom("foo".to_string()),
        ]);

        let (result, _) = eval(get_type, env);
        assert_eq!(result[0], MettaValue::Atom("Number".to_string()));
    }

    #[test]
    fn test_get_type_builtin_operations() {
        let env = Environment::new();

        // (get-type (add 1 2)) -> Number
        let get_type_add = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);
        let (result, _) = eval(get_type_add, env.clone());
        assert_eq!(result[0], MettaValue::Atom("Number".to_string()));

        // (get-type (lt 1 2)) -> Bool
        let get_type_lt = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("<".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);
        let (result, _) = eval(get_type_lt, env);
        assert_eq!(result[0], MettaValue::Atom("Bool".to_string()));
    }

    #[test]
    fn test_check_type() {
        let mut env = Environment::new();

        // Add type assertion: (: x Number)
        env.add_type("x".to_string(), MettaValue::Atom("Number".to_string()));

        // (check-type x Number) -> true
        let check_type_match = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);
        let (result, _) = eval(check_type_match, env.clone());
        assert_eq!(result[0], MettaValue::Bool(true));

        // (check-type x String) -> false
        let check_type_mismatch = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("String".to_string()),
        ]);
        let (result, _) = eval(check_type_mismatch, env);
        assert_eq!(result[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_check_type_with_type_variables() {
        let env = Environment::new();

        // (check-type 42 $t) -> true (type variable matches anything)
        let check_type_var = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$t".to_string()),
        ]);
        let (result, _) = eval(check_type_var, env);
        assert_eq!(result[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_arrow_type_assertion() {
        let mut env = Environment::new();

        // (: add (-> Number Number Number))
        // Using a user-defined function name instead of builtin "+"
        let arrow_type = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("add".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("->".to_string()),
                MettaValue::Atom("Number".to_string()),
                MettaValue::Atom("Number".to_string()),
                MettaValue::Atom("Number".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(arrow_type, env);
        env = new_env;

        // Should return empty list
        assert!(result.is_empty());

        // Get the type back
        let arrow_type_expected = MettaValue::SExpr(vec![
            MettaValue::Atom("->".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);
        assert_eq!(env.get_type("add"), Some(arrow_type_expected));
    }

    #[test]
    fn test_integration_with_rules_and_types() {
        let mut env = Environment::new();

        // Add type assertion: (: double (-> Number Number))
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("double".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("->".to_string()),
                MettaValue::Atom("Number".to_string()),
                MettaValue::Atom("Number".to_string()),
            ]),
        ]);
        let (_, new_env) = eval(type_assertion, env);
        env = new_env;

        // Add rule: (= (double $x) (mul $x 2))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        };
        env.add_rule(rule);

        // Check type of double
        let get_type = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Atom("double".to_string()),
        ]);
        let (result, _) = eval(get_type, env.clone());

        let expected_type = MettaValue::SExpr(vec![
            MettaValue::Atom("->".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);
        assert_eq!(result[0], expected_type);

        // Evaluate (double 5) -> 10
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Long(5),
        ]);
        let (result, _) = eval(value, env);
        assert_eq!(result[0], MettaValue::Long(10));
    }

    #[test]
    fn test_type_assertion_added_to_fact_database() {
        let env = Environment::new();

        // Define a type assertion: (: x Number)
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);

        let (result, new_env) = eval(type_assertion.clone(), env);

        // Type assertion should return empty list
        assert!(result.is_empty());

        // Type should be in the type database
        assert_eq!(
            new_env.get_type("x"),
            Some(MettaValue::Atom("Number".to_string()))
        );

        // Type assertion should also be in the fact database
        assert!(new_env.has_sexpr_fact(&type_assertion));
    }

    #[test]
    fn test_type_error_propagation() {
        let env = Environment::new();

        // Test: !(+ 1 (+ 2 "bad")) - error should propagate from inner expression
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::String("bad".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // The error from the inner expression should propagate
        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}
