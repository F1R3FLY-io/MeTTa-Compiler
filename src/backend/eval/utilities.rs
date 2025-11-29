use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::eval;

// ============================================================
// Utility Operations (empty, get-metatype)
// ============================================================

/// empty: Returns an empty result (no values)
/// Usage: (empty)
/// Returns no values - used for operations that should produce no output
pub(super) fn eval_empty(_items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // Return empty result - no values
    (vec![], env)
}

/// get-metatype: Returns the meta-type of an atom
/// Usage: (get-metatype atom)
/// Returns: Symbol, Variable, Expression, or Grounded
pub(super) fn eval_get_metatype(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("get-metatype", items, 1, env, "(get-metatype atom)");

    let atom = &items[1];

    // Evaluate the argument
    let (results, env1) = eval(atom.clone(), env);
    if results.is_empty() {
        // If evaluation returns empty, that's valid - return empty
        return (vec![], env1);
    }

    // Get the meta-type of the first result
    let value = &results[0];
    let meta_type = get_metatype(value);
    (vec![MettaValue::Atom(meta_type.to_string())], env1)
}

/// Get the meta-type of a MettaValue
fn get_metatype(value: &MettaValue) -> &'static str {
    match value {
        // Atoms (symbols) are the basic named entities
        MettaValue::Atom(s) => {
            if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') {
                "Variable"
            } else {
                "Symbol"
            }
        }
        // S-expressions are compound expressions
        MettaValue::SExpr(_) => "Expression",
        // All grounded values (numbers, strings, bools, etc.)
        MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::Bool(_)
        | MettaValue::String(_) => "Grounded",
        // Special types
        MettaValue::Nil => "Symbol",
        MettaValue::Unit => "Expression", // () is an empty expression
        MettaValue::Type(_) => "Expression",
        MettaValue::Conjunction(_) => "Expression",
        MettaValue::Space(_) => "Grounded",
        MettaValue::State(_) => "Grounded",
        MettaValue::Error(_, _) => "Expression",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_returns_no_values() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("empty".to_string())];
        let (results, _) = eval_empty(items, env);

        assert!(results.is_empty());
    }

    #[test]
    fn test_get_metatype_symbol() {
        assert_eq!(get_metatype(&MettaValue::Atom("foo".to_string())), "Symbol");
        assert_eq!(get_metatype(&MettaValue::Atom("bar".to_string())), "Symbol");
    }

    #[test]
    fn test_get_metatype_variable() {
        assert_eq!(get_metatype(&MettaValue::Atom("$x".to_string())), "Variable");
        assert_eq!(get_metatype(&MettaValue::Atom("&ref".to_string())), "Variable");
        assert_eq!(get_metatype(&MettaValue::Atom("'quoted".to_string())), "Variable");
    }

    #[test]
    fn test_get_metatype_expression() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert_eq!(get_metatype(&expr), "Expression");
        assert_eq!(get_metatype(&MettaValue::Unit), "Expression");
    }

    #[test]
    fn test_get_metatype_grounded() {
        assert_eq!(get_metatype(&MettaValue::Long(42)), "Grounded");
        assert_eq!(get_metatype(&MettaValue::Float(3.14)), "Grounded");
        assert_eq!(get_metatype(&MettaValue::Bool(true)), "Grounded");
        assert_eq!(get_metatype(&MettaValue::String("hello".to_string())), "Grounded");
    }
}
