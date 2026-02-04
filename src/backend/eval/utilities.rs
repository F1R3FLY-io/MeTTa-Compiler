use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

#[allow(unused_imports)]
use super::eval;
use super::EvalStep;

// ============================================================
// Utility Operations (empty, get-metatype)
// ============================================================

/// Step version of eval_get_metatype - defers evaluation to trampoline.
/// Usage: (get-metatype atom)
pub(crate) fn eval_get_metatype_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            format!(
                "get-metatype requires exactly 1 argument, got {}. Usage: (get-metatype atom)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let atom = items[1].clone();

    EvalStep::StartGetMetatype { atom, env, depth }
}

/// empty: Returns the Empty sentinel atom
/// Usage: (empty)
/// Returns Empty sentinel - will be filtered at result collection (HE-compatible).
/// This is distinct from:
/// - Empty result set (vec![]) - no alternatives exist, evaluation branch is dead
/// - Unit (()) - a valid result representing "success with no value"
pub(super) fn eval_empty(_items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    // Return Empty sentinel - will be filtered at result collection
    (vec![MettaValue::Empty()], env)
}

/// get-metatype: Returns the meta-type of an atom
/// Usage: (get-metatype atom)
/// Returns: Symbol, Variable, Expression, or Grounded
///
/// DEPRECATED: Use eval_get_metatype_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(super) fn eval_get_metatype(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
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
    match value.inner() {
        // Atoms (symbols) are the basic named entities
        MettaValueInner::Atom(s) => {
            if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') {
                "Variable"
            } else {
                "Symbol"
            }
        }
        // S-expressions are compound expressions
        MettaValueInner::SExpr(_) => "Expression",
        // All grounded values (numbers, strings, bools, etc.)
        MettaValueInner::Long(_)
        | MettaValueInner::Float(_)
        | MettaValueInner::Bool(_)
        | MettaValueInner::String(_) => "Grounded",
        // Special types
        MettaValueInner::Nil => "Symbol",
        MettaValueInner::Unit => "Expression", // () is an empty expression
        MettaValueInner::Type(_) => "Expression",
        MettaValueInner::Conjunction(_) => "Expression",
        MettaValueInner::Space(_) => "Grounded",
        MettaValueInner::State(_) => "Grounded",
        MettaValueInner::Error(_, _) => "Expression",
        MettaValueInner::Memo(_) => "Grounded",
        MettaValueInner::Empty => "Symbol", // Empty is treated as a symbol for meta-type purposes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_returns_empty_sentinel() {
        let env = HeapEnvironment::default();

        let items = vec![MettaValue::Atom("empty".to_string())];
        let (results, _) = eval_empty(items, env);

        // empty returns Empty sentinel (HE-compatible)
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].inner(), MettaValueInner::Empty));
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
        assert_eq!(get_metatype(&MettaValue::Unit()), "Expression");
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
        let env = HeapEnvironment::default();

        // (empty 1 2 3) - arguments should be ignored
        let items = vec![
            MettaValue::Atom("empty".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ];
        let (results, _) = eval_empty(items, env);

        // Still returns Empty sentinel (arguments ignored)
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].inner(), MettaValueInner::Empty));
    }

    #[test]
    fn test_empty_environment_unchanged() {
        let env = HeapEnvironment::default();

        let items = vec![MettaValue::Atom("empty".to_string())];
        let (results, _) = eval_empty(items, env);

        // empty returns Empty sentinel and doesn't modify the environment
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].inner(), MettaValueInner::Empty));
    }

    // ============================================================
    // get-metatype tests (additional)
    // ============================================================

    #[test]
    fn test_eval_get_metatype_symbol() {
        let env = HeapEnvironment::default();

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
        let env = HeapEnvironment::default();

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
        let env = HeapEnvironment::default();

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
        let env = HeapEnvironment::default();

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
        let env = HeapEnvironment::default();

        let items = vec![MettaValue::Atom("get-metatype".to_string())];
        let (results, _) = eval_get_metatype(items, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_get_metatype_nil() {
        assert_eq!(get_metatype(&MettaValue::Nil()), "Symbol");
    }

    #[test]
    fn test_get_metatype_type() {
        let typ = MettaValue::Type(MettaValue::Atom("Int".to_string()));
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
        let err = MettaValue::Error("test error".to_string(), MettaValue::Nil());
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
