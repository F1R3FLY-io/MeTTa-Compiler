use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

#[allow(unused_imports)]
use super::eval;
use super::EvalStep;

// ============================================================
// I/O Operations (println!, trace!, nop)
// ============================================================

/// Step version of eval_println - defers evaluation to trampoline.
/// Usage: (println! atom)
pub(crate) fn eval_println_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            format!(
                "println! requires exactly 1 argument, got {}. Usage: (println! atom)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let atom = items[1].clone();

    EvalStep::StartPrintln { atom, env, depth }
}

/// Step version of eval_trace - defers evaluation to trampoline.
/// Usage: (trace! message value)
pub(crate) fn eval_trace_step(items: Vec<MettaValue>, env: HeapEnvironment, depth: usize) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            format!(
                "trace! requires exactly 2 arguments, got {}. Usage: (trace! message value)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let message = items[1].clone();
    let value_expr = items[2].clone();

    EvalStep::StartTrace {
        message,
        value_expr,
        env,
        depth,
    }
}

/// println!: Print an atom to stdout
/// Usage: (println! atom)
/// Returns Unit after printing
///
/// DEPRECATED: Use eval_println_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(super) fn eval_println(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("println!", items, 1, env, "(println! atom)");

    let atom = &items[1];

    // Evaluate the argument
    let (results, env1) = eval(atom.clone(), env);
    if results.is_empty() {
        let err = MettaValue::Error(
            "println!: argument evaluated to empty".to_string(),
            atom.clone(),
        );
        return (vec![err], env1);
    }

    // Print the first result to stdout
    let value = &results[0];
    println!("{}", atom_to_string(value));

    (vec![MettaValue::Unit()], env1)
}

/// trace!: Debug trace - prints message to stderr and returns the value
/// Usage: (trace! message value)
/// Prints message to stderr, returns value unchanged
///
/// DEPRECATED: Use eval_trace_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(super) fn eval_trace(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("trace!", items, 2, env, "(trace! message value)");

    let message = &items[1];
    let value_expr = &items[2];

    // Evaluate the message
    let (msg_results, env1) = eval(message.clone(), env);
    if msg_results.is_empty() {
        let err = MettaValue::Error(
            "trace!: message evaluated to empty".to_string(),
            message.clone(),
        );
        return (vec![err], env1);
    }

    // Evaluate the value
    let (value_results, env2) = eval(value_expr.clone(), env1);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "trace!: value evaluated to empty".to_string(),
            value_expr.clone(),
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
pub(super) fn eval_nop(_items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    // nop ignores all arguments and returns Unit
    (vec![MettaValue::Unit()], env)
}

/// Convert a MettaValue to a string for printing
/// This converts the value to its MeTTa representation
pub fn atom_to_string(value: &MettaValue) -> String {
    match value.inner() {
        MettaValueInner::Long(n) => n.to_string(),
        MettaValueInner::Float(f) => f.to_string(),
        MettaValueInner::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        MettaValueInner::String(s) => s.clone(), // Print raw string without quotes for println!
        MettaValueInner::Atom(a) => a.clone(),
        MettaValueInner::Nil => "Nil".to_string(),
        MettaValueInner::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(atom_to_string).collect();
            format!("({})", inner.join(" "))
        }
        MettaValueInner::Error(msg, _) => format!("(Error \"{}\")", msg),
        MettaValueInner::Type(t) => format!("(: {})", atom_to_string(t)),
        MettaValueInner::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(atom_to_string).collect();
            format!("(, {})", inner.join(" "))
        }
        MettaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValueInner::State(id) => format!("(State {})", id),
        MettaValueInner::Unit => "()".to_string(),
        MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
        MettaValueInner::Empty => "Empty".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nop_returns_unit() {
        let env = HeapEnvironment::default();

        // (nop)
        let items = vec![MettaValue::Atom("nop".to_string())];
        let (results, _) = eval_nop(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());
    }

    #[test]
    fn test_nop_ignores_arguments() {
        let env = HeapEnvironment::default();

        // (nop 1 2 3) - arguments should be ignored
        let items = vec![
            MettaValue::Atom("nop".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ];
        let (results, _) = eval_nop(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());
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
        assert_eq!(atom_to_string(&MettaValue::Nil()), "Nil");
        assert_eq!(atom_to_string(&MettaValue::Unit()), "()");
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
        let env = HeapEnvironment::default();

        // (println! 42) - basic value printing
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::Long(42),
        ];
        let (results, _) = eval_println(items, env);

        // println! returns Unit
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());
    }

    #[test]
    fn test_println_string() {
        let env = HeapEnvironment::default();

        // (println! "Hello, World!")
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::String("Hello, World!".to_string()),
        ];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());
    }

    #[test]
    fn test_println_atom() {
        let env = HeapEnvironment::default();

        // (println! foo)
        let items = vec![
            MettaValue::Atom("println!".to_string()),
            MettaValue::Atom("foo".to_string()),
        ];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());
    }

    #[test]
    fn test_println_sexpr() {
        let env = HeapEnvironment::default();

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
        assert_eq!(results[0], MettaValue::Unit());
    }

    #[test]
    fn test_println_missing_args() {
        let env = HeapEnvironment::default();

        // (println!) - missing argument
        let items = vec![MettaValue::Atom("println!".to_string())];
        let (results, _) = eval_println(items, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_println_with_expression() {
        let env = HeapEnvironment::default();

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
        assert_eq!(results[0], MettaValue::Unit());
    }

    // ============================================================
    // trace! tests
    // ============================================================

    #[test]
    fn test_trace_returns_value() {
        let env = HeapEnvironment::default();

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
        let env = HeapEnvironment::default();

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
        let env = HeapEnvironment::default();

        // (trace! "msg") - missing value
        let items = vec![
            MettaValue::Atom("trace!".to_string()),
            MettaValue::String("msg".to_string()),
        ];
        let (results, _) = eval_trace(items, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("requires exactly 2 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_trace_no_args() {
        let env = HeapEnvironment::default();

        // (trace!) - missing both args
        let items = vec![MettaValue::Atom("trace!".to_string())];
        let (results, _) = eval_trace(items, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
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
        let err = MettaValue::Error("test error".to_string(), MettaValue::Nil());
        assert_eq!(atom_to_string(&err), "(Error \"test error\")");
    }

    #[test]
    fn test_atom_to_string_type() {
        let typ = MettaValue::Type(MettaValue::Atom("Int".to_string()));
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
