use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

use super::EvalStep;

// ============================================================
// String Operations (repr, format-args)
// ============================================================

/// Step version of eval_repr - defers evaluation to trampoline.
/// Usage: (repr atom)
pub(crate) fn eval_repr_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            format!(
                "repr requires exactly 1 argument, got {}. Usage: (repr atom)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let atom = items[1].clone();

    EvalStep::StartRepr { atom, env, depth }
}

/// Step version of eval_format_args - defers evaluation to trampoline.
/// Usage: (format-args format-string args-expression)
pub(crate) fn eval_format_args_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            format!(
                "format-args requires exactly 2 arguments, got {}. Usage: (format-args format-string args)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let format_arg = items[1].clone();
    let args_arg = items[2].clone();

    EvalStep::StartFormatArgs {
        format_arg,
        args_arg,
        env,
        depth,
    }
}


/// Convert a MettaValue to its repr string (MeTTa representation)
fn atom_repr(value: &MettaValue) -> String {
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
        MettaValueInner::String(s) => format!("\"{}\"", s), // Include quotes for string repr
        MettaValueInner::Atom(a) => a.clone(),
        MettaValueInner::Nil => "Nil".to_string(),
        MettaValueInner::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(atom_repr).collect();
            format!("({})", inner.join(" "))
        }
        MettaValueInner::Error(msg, _) => format!("(Error \"{}\")", msg),
        MettaValueInner::Type(t) => format!("(: {})", atom_repr(t)),
        MettaValueInner::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(atom_repr).collect();
            format!("(, {})", inner.join(" "))
        }
        MettaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValueInner::State(id) => format!("(State {})", id),
        MettaValueInner::Unit => "()".to_string(),
        MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
        MettaValueInner::Empty => "Empty".to_string(),
    }
}

/// Format a string by replacing {} placeholders with argument values
fn format_string(format_str: &str, args: &[&MettaValue]) -> String {
    let mut result = String::with_capacity(format_str.len() * 2);
    let mut chars = format_str.chars().peekable();
    let mut arg_index = 0;

    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'}') {
                chars.next(); // consume '}'
                if arg_index < args.len() {
                    // Use atom_to_string (without quotes for strings)
                    result.push_str(&atom_to_string(args[arg_index]));
                    arg_index += 1;
                } else {
                    // Not enough arguments, keep the placeholder
                    result.push_str("{}");
                }
            } else if chars.peek() == Some(&'{') {
                // Escaped {{ -> {
                chars.next();
                result.push('{');
            } else {
                result.push(c);
            }
        } else if c == '}' && chars.peek() == Some(&'}') {
            // Escaped }} -> }
            chars.next();
            result.push('}');
        } else {
            result.push(c);
        }
    }

    result
}

/// Convert a MettaValue to a string for formatting (without quotes)
fn atom_to_string(value: &MettaValue) -> String {
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
        MettaValueInner::String(s) => s.clone(), // No quotes for formatting
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
    fn test_atom_repr() {
        assert_eq!(atom_repr(&MettaValue::Long(42)), "42");
        assert_eq!(atom_repr(&MettaValue::Bool(true)), "True");
        assert_eq!(
            atom_repr(&MettaValue::String("hello".to_string())),
            "\"hello\""
        );
        assert_eq!(atom_repr(&MettaValue::Atom("foo".to_string())), "foo");
        assert_eq!(atom_repr(&MettaValue::Unit()), "()");
    }

    #[test]
    fn test_format_string_basic() {
        let args: Vec<&MettaValue> = vec![];
        assert_eq!(format_string("Hello, world!", &args), "Hello, world!");
    }

    #[test]
    fn test_format_string_with_placeholders() {
        let name = MettaValue::String("Alice".to_string());
        let age = MettaValue::Long(25);
        let args: Vec<&MettaValue> = vec![&name, &age];
        assert_eq!(
            format_string("Hello, {}! You are {} years old.", &args),
            "Hello, Alice! You are 25 years old."
        );
    }

    #[test]
    fn test_format_string_escaped_braces() {
        let args: Vec<&MettaValue> = vec![];
        assert_eq!(
            format_string("Use {{}} for placeholders", &args),
            "Use {} for placeholders"
        );
    }

    #[test]
    fn test_format_string_missing_args() {
        let name = MettaValue::String("Bob".to_string());
        let args: Vec<&MettaValue> = vec![&name];
        assert_eq!(
            format_string("Hello, {}! Value: {}", &args),
            "Hello, Bob! Value: {}"
        );
    }

    #[test]
    fn test_repr() {
        use super::super::eval;
        let env = Environment::new();

        // Test with a simple value via eval
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("42".to_string()));
    }

    // ============================================================
    // repr tests (additional)
    // ============================================================

    #[test]
    fn test_repr_string_includes_quotes() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::String("hello".to_string()),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        // repr includes quotes for strings
        assert_eq!(results[0], MettaValue::String("\"hello\"".to_string()));
    }

    #[test]
    fn test_repr_bool() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("True".to_string()));
    }

    #[test]
    fn test_repr_bool_false() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Bool(false),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("False".to_string()));
    }

    #[test]
    fn test_repr_sexpr() {
        use super::super::eval;
        let env = Environment::new();

        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let value = MettaValue::SExpr(vec![MettaValue::Atom("repr".to_string()), sexpr]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("(foo 1 2)".to_string()));
    }

    #[test]
    fn test_repr_atom() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Atom("my-symbol".to_string()),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("my-symbol".to_string()));
    }

    #[test]
    fn test_repr_nil() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Nil(),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("Nil".to_string()));
    }

    #[test]
    fn test_repr_unit() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Unit(),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("()".to_string()));
    }

    #[test]
    fn test_repr_float() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Float(3.25),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        // Float to string representation
        let result_str = match results[0].inner() {
            MettaValueInner::String(s) => s.clone(),
            _ => panic!("Expected string"),
        };
        assert!(result_str.starts_with("3.25"));
    }

    #[test]
    fn test_repr_missing_args() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![MettaValue::Atom("repr".to_string())]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    // ============================================================
    // format-args tests (additional)
    // ============================================================

    #[test]
    fn test_format_args_single() {
        use super::super::eval;
        let env = Environment::new();

        // (format-args "Hello, {}!" name) where name evaluates to "World"
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("Hello, {}!".to_string()),
            MettaValue::String("World".to_string()),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("Hello, World!".to_string()));
    }

    #[test]
    fn test_format_args_multiple() {
        use super::super::eval;
        let env = Environment::new();

        // (format-args "{} + {} = {}" (1 2 3))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("{} + {} = {}".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("1 + 2 = 3".to_string()));
    }

    #[test]
    fn test_format_args_no_placeholders() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("No placeholders here".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(42)]),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::String("No placeholders here".to_string())
        );
    }

    #[test]
    fn test_format_args_missing_format_string() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![MettaValue::Atom("format-args".to_string())]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("requires exactly 2 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_format_args_non_string_format() {
        use super::super::eval;
        let env = Environment::new();

        // First arg must be a string
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::Long(42), // Not a string
            MettaValue::String("arg".to_string()),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("must be a string"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_format_args_with_escaped_braces() {
        use super::super::eval;
        let env = Environment::new();

        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("Value is: {{}} = {}".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(value, env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::String("Value is: {} = 42".to_string())
        );
    }

    // ============================================================
    // atom_repr tests (additional)
    // ============================================================

    #[test]
    fn test_atom_repr_float() {
        assert_eq!(atom_repr(&MettaValue::Float(2.5)), "2.5");
        assert_eq!(atom_repr(&MettaValue::Float(-1.0)), "-1");
    }

    #[test]
    fn test_atom_repr_nil() {
        assert_eq!(atom_repr(&MettaValue::Nil()), "Nil");
    }

    #[test]
    fn test_atom_repr_sexpr_nested() {
        let nested = MettaValue::SExpr(vec![
            MettaValue::Atom("outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("inner".to_string()),
                MettaValue::Long(1),
            ]),
        ]);
        assert_eq!(atom_repr(&nested), "(outer (inner 1))");
    }

    #[test]
    fn test_atom_repr_conjunction() {
        let conj = MettaValue::Conjunction(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        assert_eq!(atom_repr(&conj), "(, a b)");
    }

    #[test]
    fn test_atom_repr_error() {
        let err = MettaValue::Error("test error".to_string(), MettaValue::Nil());
        assert_eq!(atom_repr(&err), "(Error \"test error\")");
    }

    // ============================================================
    // atom_to_string tests
    // ============================================================

    #[test]
    fn test_atom_to_string_no_quotes() {
        // atom_to_string should NOT include quotes for strings
        assert_eq!(
            atom_to_string(&MettaValue::String("hello".to_string())),
            "hello"
        );
    }

    #[test]
    fn test_atom_to_string_vs_repr() {
        let s = MettaValue::String("test".to_string());
        // atom_to_string: no quotes
        assert_eq!(atom_to_string(&s), "test");
        // atom_repr: with quotes
        assert_eq!(atom_repr(&s), "\"test\"");
    }
}
