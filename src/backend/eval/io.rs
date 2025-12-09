use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::eval;

// ============================================================
// I/O Operations (println!, trace!, nop)
// ============================================================

/// println!: Print an atom to stdout
/// Usage: (println! atom)
/// Returns Unit after printing
pub(super) fn eval_println(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("println!", items, 1, env, "(println! atom)");

    let atom = &items[1];

    // Evaluate the argument
    let (results, env1) = eval(atom.clone(), env);
    if results.is_empty() {
        let err = MettaValue::Error(
            "println!: argument evaluated to empty".to_string(),
            std::sync::Arc::new(atom.clone()),
        );
        return (vec![err], env1);
    }

    // Print the first result to stdout
    let value = &results[0];
    println!("{}", atom_to_string(value));

    (vec![MettaValue::Unit], env1)
}

/// trace!: Debug trace - prints message to stderr and returns the value
/// Usage: (trace! message value)
/// Prints message to stderr, returns value unchanged
pub(super) fn eval_trace(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("trace!", items, 2, env, "(trace! message value)");

    let message = &items[1];
    let value_expr = &items[2];

    // Evaluate the message
    let (msg_results, env1) = eval(message.clone(), env);
    if msg_results.is_empty() {
        let err = MettaValue::Error(
            "trace!: message evaluated to empty".to_string(),
            std::sync::Arc::new(message.clone()),
        );
        return (vec![err], env1);
    }

    // Evaluate the value
    let (value_results, env2) = eval(value_expr.clone(), env1);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "trace!: value evaluated to empty".to_string(),
            std::sync::Arc::new(value_expr.clone()),
        );
        return (vec![err], env2);
    }

    // Print message to stderr
    let msg = &msg_results[0];
    eprintln!("{}", atom_to_string(msg));

    // Return the value (first result)
    (vec![value_results[0].clone()], env2)
}

/// nop: No operation - returns Unit immediately
/// Usage: (nop) or (nop ...) - any arguments are ignored
/// Always returns Unit
pub(super) fn eval_nop(_items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // nop ignores all arguments and returns Unit
    (vec![MettaValue::Unit], env)
}

/// Convert a MettaValue to a string for printing
/// This converts the value to its MeTTa representation
fn atom_to_string(value: &MettaValue) -> String {
    match value {
        MettaValue::Long(n) => n.to_string(),
        MettaValue::Float(f) => f.to_string(),
        MettaValue::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        MettaValue::String(s) => s.clone(), // Print raw string without quotes for println!
        MettaValue::Atom(a) => a.clone(),
        MettaValue::Nil => "Nil".to_string(),
        MettaValue::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(atom_to_string).collect();
            format!("({})", inner.join(" "))
        }
        MettaValue::Error(msg, _) => format!("(Error \"{}\")", msg),
        MettaValue::Type(t) => format!("(: {})", atom_to_string(t)),
        MettaValue::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(atom_to_string).collect();
            format!("(, {})", inner.join(" "))
        }
        MettaValue::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValue::State(id) => format!("(State {})", id),
        MettaValue::Unit => "()".to_string(),
        MettaValue::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nop_returns_unit() {
        let env = Environment::new();

        // (nop)
        let items = vec![MettaValue::Atom("nop".to_string())];
        let (results, _) = eval_nop(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_nop_ignores_arguments() {
        let env = Environment::new();

        // (nop 1 2 3) - arguments should be ignored
        let items = vec![
            MettaValue::Atom("nop".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ];
        let (results, _) = eval_nop(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_atom_to_string() {
        assert_eq!(atom_to_string(&MettaValue::Long(42)), "42");
        assert_eq!(atom_to_string(&MettaValue::Bool(true)), "True");
        assert_eq!(atom_to_string(&MettaValue::Bool(false)), "False");
        assert_eq!(
            atom_to_string(&MettaValue::String("hello".to_string())),
            "hello"
        );
        assert_eq!(atom_to_string(&MettaValue::Atom("foo".to_string())), "foo");
        assert_eq!(atom_to_string(&MettaValue::Nil), "Nil");
        assert_eq!(atom_to_string(&MettaValue::Unit), "()");
    }

    #[test]
    fn test_atom_to_string_sexpr() {
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert_eq!(atom_to_string(&sexpr), "(add 1 2)");
    }

    // ============================================================
    // println! tests
    // ============================================================

    #[test]
    fn test_println_basic_value() {
        let env = Environment::new();

        // (println! 42) - basic value printing
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::Long(42),
        ];
        let (results, _) = eval_println(items, env);

        // println! returns Unit
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_println_string() {
        let env = Environment::new();

        // (println! "Hello, World!")
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::String("Hello, World!".to_string()),
        ];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_println_atom() {
        let env = Environment::new();

        // (println! foo)
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::Atom("foo".to_string()),
        ];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_println_sexpr() {
        let env = Environment::new();

        // (println! (foo bar))
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("bar".to_string()),
            ]),
        ];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_println_missing_args() {
        let env = Environment::new();

        // (println!) - missing argument
        let items = vec![MettaValue::Atom("println!".to_string())];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_println_with_expression() {
        let env = Environment::new();

        // (println! (+ 2 3)) - prints the result of the expression
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    // ============================================================
    // trace! tests
    // ============================================================

    #[test]
    fn test_trace_returns_value() {
        let env = Environment::new();

        // (trace! "debug" 42) - should return 42
        let items = vec![
            MettaValue::Atom("trace!".to_string()),
            MettaValue::String("debug".to_string()),
            MettaValue::Long(42),
        ];
        let (results, _) = eval_trace(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_trace_with_complex_value() {
        let env = Environment::new();

        // (trace! "msg" (foo bar)) - should return (foo bar)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);
        let items = vec![
            MettaValue::Atom("trace!".to_string()),
            MettaValue::String("checking value".to_string()),
            value.clone(),
        ];
        let (results, _) = eval_trace(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], value);
    }

    #[test]
    fn test_trace_missing_args() {
        let env = Environment::new();

        // (trace! "msg") - missing value
        let items = vec![
            MettaValue::Atom("trace!".to_string()),
            MettaValue::String("msg".to_string()),
        ];
        let (results, _) = eval_trace(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("requires exactly 2 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_trace_no_args() {
        let env = Environment::new();

        // (trace!) - missing both args
        let items = vec![MettaValue::Atom("trace!".to_string())];
        let (results, _) = eval_trace(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("requires exactly 2 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    // ============================================================
    // atom_to_string tests (additional)
    // ============================================================

    #[test]
    fn test_atom_to_string_float() {
        assert_eq!(atom_to_string(&MettaValue::Float(3.25)), "3.25");
        assert_eq!(atom_to_string(&MettaValue::Float(-2.5)), "-2.5");
    }

    #[test]
    fn test_atom_to_string_conjunction() {
        let conj = MettaValue::Conjunction(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        assert_eq!(atom_to_string(&conj), "(, a b)");
    }

    #[test]
    fn test_atom_to_string_error() {
        let err = MettaValue::Error(
            "test error".to_string(),
            std::sync::Arc::new(MettaValue::Nil),
        );
        assert_eq!(atom_to_string(&err), "(Error \"test error\")");
    }

    #[test]
    fn test_atom_to_string_type() {
        let typ = MettaValue::Type(std::sync::Arc::new(MettaValue::Atom("Int".to_string())));
        assert_eq!(atom_to_string(&typ), "(: Int)");
    }

    #[test]
    fn test_atom_to_string_nested_sexpr() {
        let nested = MettaValue::SExpr(vec![
            MettaValue::Atom("outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("inner".to_string()),
                MettaValue::Long(1),
            ]),
        ]);
        assert_eq!(atom_to_string(&nested), "(outer (inner 1))");
    }
}
