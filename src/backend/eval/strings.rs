use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::sync::Arc;

use super::eval;

// ============================================================
// String Operations (repr, format-args)
// ============================================================

/// repr: Convert an atom to its string representation
/// Usage: (repr atom)
/// Returns the string representation of the atom
pub(super) fn eval_repr(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("repr", items, 1, env, "(repr atom)");

    let atom = &items[1];

    // Evaluate the argument
    let (results, env1) = eval(atom.clone(), env);
    if results.is_empty() {
        let err = MettaValue::Error(
            "repr: argument evaluated to empty".to_string(),
            Arc::new(atom.clone()),
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
            Arc::new(format_arg.clone()),
        );
        return (vec![err], env1);
    }

    // Get the format string
    let format_str = match &format_results[0] {
        MettaValue::String(s) => s.clone(),
        other => {
            let err = MettaValue::Error(
                format!(
                    "format-args: first argument must be a string, got {}",
                    super::friendly_value_repr(other)
                ),
                Arc::new(other.clone()),
            );
            return (vec![err], env1);
        }
    };

    // Evaluate the args expression
    let (args_results, env2) = eval(args_arg.clone(), env1);
    if args_results.is_empty() {
        let err = MettaValue::Error(
            "format-args: args evaluated to empty".to_string(),
            Arc::new(args_arg.clone()),
        );
        return (vec![err], env2);
    }

    // Get the args as a list of values
    let args: Vec<&MettaValue> = match &args_results[0] {
        MettaValue::SExpr(items) => items.iter().collect(),
        other => vec![other],
    };

    // Perform the formatting
    let result = format_string(&format_str, &args);
    (vec![MettaValue::String(result)], env2)
}

/// Convert a MettaValue to its repr string (MeTTa representation)
fn atom_repr(value: &MettaValue) -> String {
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
        MettaValue::String(s) => format!("\"{}\"", s), // Include quotes for string repr
        MettaValue::Atom(a) => a.clone(),
        MettaValue::Nil => "Nil".to_string(),
        MettaValue::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(atom_repr).collect();
            format!("({})", inner.join(" "))
        }
        MettaValue::Error(msg, _) => format!("(Error \"{}\")", msg),
        MettaValue::Type(t) => format!("(: {})", atom_repr(t)),
        MettaValue::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(atom_repr).collect();
            format!("(, {})", inner.join(" "))
        }
        MettaValue::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValue::State(id) => format!("(State {})", id),
        MettaValue::Unit => "()".to_string(),
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
        MettaValue::String(s) => s.clone(), // No quotes for formatting
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
        assert_eq!(atom_repr(&MettaValue::Unit), "()");
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
}
