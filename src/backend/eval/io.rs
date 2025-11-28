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
        MettaValue::Space(id, name) => format!("(Space {} \"{}\")", id, name),
        MettaValue::State(id) => format!("(State {})", id),
        MettaValue::Unit => "()".to_string(),
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
        assert_eq!(atom_to_string(&MettaValue::String("hello".to_string())), "hello");
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
}
