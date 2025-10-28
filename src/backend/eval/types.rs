use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::EvalOutput;

/// Type assertion: (: expr type)
/// Adds a type assertion to the environment
pub(super) fn eval_type_assertion(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!(":", items, env);

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

    return (vec![], new_env);
}

/// get-type: return the type of an expression
/// (get-type expr) -> Type
pub(super) fn eval_get_type(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("get-type", items, env);

    let expr = &items[1];
    let typ = infer_type(expr, &env);
    return (vec![typ], env);
}

/// check-type: check if expression has expected type
/// (check-type expr expected-type) -> Bool
pub(super) fn eval_check_type(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!("check-type", items, env);

    let expr = &items[1];
    let expected = &items[2];

    let actual = infer_type(expr, &env);
    let matches = types_match(&actual, expected);

    return (vec![MettaValue::Bool(matches)], env);
}

/// Infer the type of an expression
/// Returns a MettaValue representing the type
fn infer_type(expr: &MettaValue, env: &Environment) -> MettaValue {
    match expr {
        // Ground types have built-in types
        MettaValue::Bool(_) => MettaValue::Atom("Bool".to_string()),
        MettaValue::Long(_) => MettaValue::Atom("Number".to_string()),
        MettaValue::String(_) => MettaValue::Atom("String".to_string()),
        MettaValue::Uri(_) => MettaValue::Atom("URI".to_string()),
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
                return MettaValue::Type(Box::new(MettaValue::Atom(name.clone())));
            }

            // Look up type in environment
            env.get_type(name)
                .unwrap_or_else(|| MettaValue::Atom("Undefined".to_string()))
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
