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
        assert_eq!(
            get_metatype(&MettaValue::Atom("$x".to_string())),
            "Variable"
        );
        assert_eq!(
            get_metatype(&MettaValue::Atom("&ref".to_string())),
            "Variable"
        );
        assert_eq!(
            get_metatype(&MettaValue::Atom("'quoted".to_string())),
            "Variable"
        );
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
    #[allow(clippy::approx_constant)]
    fn test_get_metatype_grounded() {
        assert_eq!(get_metatype(&MettaValue::Long(42)), "Grounded");
        assert_eq!(get_metatype(&MettaValue::Float(3.14)), "Grounded");
        assert_eq!(get_metatype(&MettaValue::Bool(true)), "Grounded");
        assert_eq!(
            get_metatype(&MettaValue::String("hello".to_string())),
            "Grounded"
        );
    }

    // ============================================================
    // empty tests (additional)
    // ============================================================

    #[test]
    fn test_empty_with_arguments() {
        let env = Environment::new();

        // (empty 1 2 3) - arguments should be ignored
        let items = vec![
            MettaValue::Atom("empty".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ];
        let (results, _) = eval_empty(items, env);

        // Still returns empty
        assert!(results.is_empty());
    }

    #[test]
    fn test_empty_environment_unchanged() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("empty".to_string())];
        let (results, _) = eval_empty(items, env);

        // empty returns no values and doesn't modify the environment
        assert!(results.is_empty());
    }

    // ============================================================
    // get-metatype tests (additional)
    // ============================================================

    #[test]
    fn test_eval_get_metatype_symbol() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("get-metatype".to_string()),
            MettaValue::Atom("my-symbol".to_string()),
        ];
        let (results, _) = eval_get_metatype(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("Symbol".to_string()));
    }

    #[test]
    fn test_eval_get_metatype_variable() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("get-metatype".to_string()),
            MettaValue::Atom("$x".to_string()),
        ];
        let (results, _) = eval_get_metatype(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("Variable".to_string()));
    }

    #[test]
    fn test_eval_get_metatype_grounded() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("get-metatype".to_string()),
            MettaValue::Long(42),
        ];
        let (results, _) = eval_get_metatype(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("Grounded".to_string()));
    }

    #[test]
    fn test_eval_get_metatype_expression() {
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);
        let items = vec![MettaValue::Atom("get-metatype".to_string()), expr];
        let (results, _) = eval_get_metatype(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("Expression".to_string()));
    }

    #[test]
    fn test_eval_get_metatype_missing_args() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("get-metatype".to_string())];
        let (results, _) = eval_get_metatype(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_get_metatype_nil() {
        assert_eq!(get_metatype(&MettaValue::Nil), "Symbol");
    }

    #[test]
    fn test_get_metatype_type() {
        let typ = MettaValue::Type(std::sync::Arc::new(MettaValue::Atom("Int".to_string())));
        assert_eq!(get_metatype(&typ), "Expression");
    }

    #[test]
    fn test_get_metatype_conjunction() {
        let conj = MettaValue::Conjunction(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        assert_eq!(get_metatype(&conj), "Expression");
    }

    #[test]
    fn test_get_metatype_error() {
        let err = MettaValue::Error(
            "test error".to_string(),
            std::sync::Arc::new(MettaValue::Nil),
        );
        assert_eq!(get_metatype(&err), "Expression");
    }

    #[test]
    fn test_get_metatype_state() {
        let state = MettaValue::State(42);
        assert_eq!(get_metatype(&state), "Grounded");
    }

    #[test]
    fn test_get_metatype_bool_false() {
        assert_eq!(get_metatype(&MettaValue::Bool(false)), "Grounded");
    }

    #[test]
    fn test_get_metatype_string_empty() {
        assert_eq!(
            get_metatype(&MettaValue::String("".to_string())),
            "Grounded"
        );
    }

    #[test]
    fn test_get_metatype_variable_patterns() {
        // All variable prefixes should return "Variable"
        assert_eq!(
            get_metatype(&MettaValue::Atom("$var".to_string())),
            "Variable"
        );
        assert_eq!(
            get_metatype(&MettaValue::Atom("&space".to_string())),
            "Variable"
        );
        assert_eq!(
            get_metatype(&MettaValue::Atom("'quote".to_string())),
            "Variable"
        );
    }

    #[test]
    fn test_get_metatype_underscore_is_symbol() {
        // Underscore should be a symbol (wildcard pattern, but still a symbol)
        assert_eq!(get_metatype(&MettaValue::Atom("_".to_string())), "Symbol");
    }
}
