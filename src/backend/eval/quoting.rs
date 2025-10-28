use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::EvalOutput;

/// Quote: return argument unevaluated
pub(super) fn eval_quote(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("quote", items, env);
    return (vec![items[1].clone()], env);
}

mod tests {
    use super::*;
    use crate::eval;

    #[test]
    fn test_quote_missing_argument() {
        let env = Environment::new();

        // (quote) - missing argument
        let value = MettaValue::SExpr(vec![MettaValue::Atom("quote".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("quote"));
                assert!(msg.contains("argument")); // Just check for "argument" - flexible
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_prevents_evaluation() {
        let env = Environment::new();

        // (quote (+ 1 2))
        // Should return the expression unevaluated
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // Should be the unevaluated s-expression with "+" not evaluated to 3
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::Atom("+".to_string()));
            }
            _ => panic!("Expected SExpr"),
        }
    }

    #[test]
    fn test_quote_with_variable() {
        let env = Environment::new();

        // (quote $x)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("$x".to_string()));
    }
}
