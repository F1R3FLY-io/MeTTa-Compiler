use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

#[allow(unused_imports)]
use super::eval;
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

/// repr: Convert an atom to its string representation
/// Usage: (repr atom)
/// Returns the string representation of the atom
///
/// DEPRECATED: Use eval_repr_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(super) fn eval_repr(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("repr", items, 1, env, "(repr atom)");

    let atom = &items[1];

    // Evaluate the argument
    let (results, env1) = eval(atom.clone(), env);
    if results.is_empty() {
        let err = MettaValue::Error(
            "repr: argument evaluated to empty".to_string(),
            atom.clone(),
        );
        return (vec![err], env1);
    }

    // Convert the first result to its string representation
    let value = &results[0];
    let repr = atom_repr(value);
    (vec![MettaValue::String(repr)], env1)
}

/// format-args: String formatting with {} placeholders
/// Usage: (format-args format-string args-expression)
/// Replaces {} placeholders with the corresponding argument values
/// Example: (format-args "Hello, {}!" (name)) -> "Hello, Alice!"
///
/// DEPRECATED: Use eval_format_args_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(super) fn eval_format_args(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!(
        "format-args",
        items,
        2,
        env,
        "(format-args format-string args)"
    );

    let format_arg = &items[1];
    let args_arg = &items[2];

    // Evaluate the format string
    let (format_results, env1) = eval(format_arg.clone(), env);
    if format_results.is_empty() {
        let err = MettaValue::Error(
            "format-args: format string evaluated to empty".to_string(),
            format_arg.clone(),
        );
        return (vec![err], env1);
    }

    // Get the format string
    let format_str = match format_results[0].inner() {
        MettaValueInner::String(s) => s.clone(),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "format-args: first argument must be a string, got {}",
                    super::friendly_value_repr(&format_results[0])
                ),
                format_results[0].clone(),
            );
            return (vec![err], env1);
        }
    };

    // Evaluate the args expression
    let (args_results, env2) = eval(args_arg.clone(), env1);
    if args_results.is_empty() {
        let err = MettaValue::Error(
            "format-args: args evaluated to empty".to_string(),
            args_arg.clone(),
        );
        return (vec![err], env2);
    }

    // Get the args as a list of values
    let args: Vec<&MettaValue> = match args_results[0].inner() {
        MettaValueInner::SExpr(items) => items.iter().collect(),
        _ => vec![&args_results[0]],
    };

    // Perform the formatting
    let result = format_string(&format_str, &args);
    (vec![MettaValue::String(result)], env2)
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
        let env = Environment::new();

        // Test with a simple value
        let items = vec![MettaValue::Atom("repr".to_string()), MettaValue::Long(42)];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("42".to_string()));
    }

    // ============================================================
    // repr tests (additional)
    // ============================================================

    #[test]
    fn test_repr_string_includes_quotes() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::String("hello".to_string()),
        ];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        // repr includes quotes for strings
        assert_eq!(results[0], MettaValue::String("\"hello\"".to_string()));
    }

    #[test]
    fn test_repr_bool() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("repr".to_string()), MettaValue::Bool(true)];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("True".to_string()));
    }

    #[test]
    fn test_repr_bool_false() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Bool(false),
        ];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("False".to_string()));
    }

    #[test]
    fn test_repr_sexpr() {
        let env = Environment::new();

        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let items = vec![MettaValue::Atom("repr".to_string()), sexpr];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("(foo 1 2)".to_string()));
    }

    #[test]
    fn test_repr_atom() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Atom("my-symbol".to_string()),
        ];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("my-symbol".to_string()));
    }

    #[test]
    fn test_repr_nil() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("repr".to_string()), MettaValue::Nil()];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("Nil".to_string()));
    }

    #[test]
    fn test_repr_unit() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("repr".to_string()), MettaValue::Unit()];
        let (results, _) = eval_repr(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("()".to_string()));
    }

    #[test]
    fn test_repr_float() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("repr".to_string()),
            MettaValue::Float(3.25),
        ];
        let (results, _) = eval_repr(items, env);

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
        let env = Environment::new();

        let items = vec![MettaValue::Atom("repr".to_string())];
        let (results, _) = eval_repr(items, env);

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
        let env = Environment::new();

        // (format-args "Hello, {}!" name) where name evaluates to "World"
        let items = vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("Hello, {}!".to_string()),
            MettaValue::String("World".to_string()),
        ];
        let (results, _) = eval_format_args(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("Hello, World!".to_string()));
    }

    #[test]
    fn test_format_args_multiple() {
        let env = Environment::new();

        // (format-args "{} + {} = {}" (1 2 3))
        let items = vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("{} + {} = {}".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ];
        let (results, _) = eval_format_args(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("1 + 2 = 3".to_string()));
    }

    #[test]
    fn test_format_args_no_placeholders() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("No placeholders here".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(42)]),
        ];
        let (results, _) = eval_format_args(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::String("No placeholders here".to_string())
        );
    }

    #[test]
    fn test_format_args_missing_format_string() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("format-args".to_string())];
        let (results, _) = eval_format_args(items, env);

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
        let env = Environment::new();

        // First arg must be a string
        let items = vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::Long(42), // Not a string
            MettaValue::String("arg".to_string()),
        ];
        let (results, _) = eval_format_args(items, env);

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
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("format-args".to_string()),
            MettaValue::String("Value is: {{}} = {}".to_string()),
            MettaValue::Long(42),
        ];
        let (results, _) = eval_format_args(items, env);

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
