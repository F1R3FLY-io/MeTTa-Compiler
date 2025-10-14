// Eval function: Lazy evaluation with pattern matching and built-in dispatch
//
// eval(a: atom, env) = a, env
// eval((t1 .. tn), env):
//   r1, env_1 = eval(t1, env) | ... | rn, env_n = eval(tn, env)
//   env' = union env_i
//   return fold over rules & grounded functions (emptyset, env')

use crate::backend::types::{MettaValue, Environment, Bindings, EvalResult};

/// Evaluate a MettaValue in the given environment
/// Returns (results, new_environment)
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    match value {
        // Errors propagate immediately without further evaluation
        MettaValue::Error(_, _) => {
            (vec![value], env)
        }

        // For atoms and types, return as-is
        MettaValue::Atom(_) | MettaValue::Bool(_) | MettaValue::Long(_)
        | MettaValue::String(_) | MettaValue::Uri(_) | MettaValue::Nil
        | MettaValue::Type(_) => {
            (vec![value], env)
        }

        // For s-expressions, evaluate elements and apply rules/built-ins
        MettaValue::SExpr(items) => eval_sexpr(items, env),
    }
}

/// Evaluate an s-expression
fn eval_sexpr(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    if items.is_empty() {
        return (vec![MettaValue::Nil], env);
    }

    // Check for special forms before evaluation
    if let Some(MettaValue::Atom(op)) = items.first() {
        match op.as_str() {
            // Rule definition: (= lhs rhs) - add to environment, don't evaluate
            "=" => {
                if items.len() >= 3 {
                    let lhs = items[1].clone();
                    let rhs = items[2].clone();
                    let mut new_env = env.clone();
                    new_env.add_rule(crate::backend::types::Rule { lhs, rhs });
                    // Return nil to indicate rule was added
                    return (vec![MettaValue::Nil], new_env);
                } else {
                    let err = MettaValue::Error(
                        "= requires exactly two arguments: lhs and rhs".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            // Evaluation: ! expr - force evaluation
            "!" => {
                if items.len() >= 2 {
                    // Evaluate the expression after !
                    return eval(items[1].clone(), env);
                } else {
                    let err = MettaValue::Error(
                        "! requires exactly one argument to evaluate".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            // Quote: return argument unevaluated
            "quote" => {
                if items.len() >= 2 {
                    return (vec![items[1].clone()], env);
                } else {
                    let err = MettaValue::Error(
                        "quote requires exactly one argument".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            // If: conditional evaluation - only evaluate chosen branch
            "if" => {
                return eval_if(&items[1..], env);
            }

            // Error construction
            "error" => {
                if items.len() >= 2 {
                    let msg = match &items[1] {
                        MettaValue::String(s) => s.clone(),
                        MettaValue::Atom(s) => s.clone(),
                        other => format!("{:?}", other),
                    };
                    let details = if items.len() > 2 {
                        items[2].clone()
                    } else {
                        MettaValue::Nil
                    };
                    return (vec![MettaValue::Error(msg, Box::new(details))], env);
                }
            }

            // Catch: error recovery - (catch expr default)
            // If expr evaluates to error, return default instead
            "catch" => {
                return eval_catch(&items[1..], env);
            }

            // Eval: force evaluation of quoted expressions
            // (eval expr) - complementary to quote
            "eval" => {
                if items.len() >= 2 {
                    // First evaluate the argument to get the expression
                    let (arg_results, arg_env) = eval(items[1].clone(), env);
                    if let Some(expr) = arg_results.first() {
                        // Then evaluate the result
                        return eval(expr.clone(), arg_env);
                    } else {
                        return (vec![MettaValue::Nil], arg_env);
                    }
                } else {
                    let err = MettaValue::Error(
                        "eval requires exactly one argument".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            // Is-error: check if value is an error (for error recovery)
            "is-error" => {
                if items.len() >= 2 {
                    let (results, new_env) = eval(items[1].clone(), env);
                    if let Some(first) = results.first() {
                        let is_err = matches!(first, MettaValue::Error(_, _));
                        return (vec![MettaValue::Bool(is_err)], new_env);
                    } else {
                        return (vec![MettaValue::Bool(false)], new_env);
                    }
                } else {
                    let err = MettaValue::Error(
                        "is-error requires exactly one argument".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            // Type assertion: (: expr type)
            // Adds a type assertion to the environment
            ":" => {
                if items.len() >= 3 {
                    let expr = &items[1];
                    let typ = items[2].clone();

                    // Extract name from expression (atom or first element of sexpr)
                    let name = match expr {
                        MettaValue::Atom(s) => s.clone(),
                        MettaValue::SExpr(expr_items) if !expr_items.is_empty() => {
                            if let MettaValue::Atom(s) = &expr_items[0] {
                                s.clone()
                            } else {
                                format!("{:?}", expr)
                            }
                        }
                        _ => format!("{:?}", expr),
                    };

                    let mut new_env = env.clone();
                    new_env.add_type(name, typ);
                    return (vec![MettaValue::Nil], new_env);
                } else {
                    let err = MettaValue::Error(
                        ": requires 2 arguments: expression and type".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            // get-type: return the type of an expression
            // (get-type expr) -> Type
            "get-type" => {
                if items.len() >= 2 {
                    let expr = &items[1];
                    let typ = infer_type(expr, &env);
                    return (vec![typ], env);
                } else {
                    let err = MettaValue::Error(
                        "get-type requires exactly one argument".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            // check-type: check if expression has expected type
            // (check-type expr expected-type) -> Bool
            "check-type" => {
                if items.len() >= 3 {
                    let expr = &items[1];
                    let expected = &items[2];

                    let actual = infer_type(expr, &env);
                    let matches = types_match(&actual, expected);

                    return (vec![MettaValue::Bool(matches)], env);
                } else {
                    let err = MettaValue::Error(
                        "check-type requires 2 arguments: expression and expected type".to_string(),
                        Box::new(MettaValue::SExpr(items)),
                    );
                    return (vec![err], env);
                }
            }

            _ => {} // Fall through to normal evaluation
        }
    }

    // Lazy evaluation: evaluate each element in parallel (conceptually)
    // For now, we'll evaluate sequentially and union environments
    let mut eval_results = Vec::new();
    let mut envs = Vec::new();

    for item in items.iter() {
        let (results, new_env) = eval(item.clone(), env.clone());

        // Check for errors in subexpressions and propagate immediately
        if let Some(first) = results.first() {
            if matches!(first, MettaValue::Error(_, _)) {
                return (vec![first.clone()], new_env);
            }
        }

        eval_results.push(results);
        envs.push(new_env);
    }

    // Union all environments
    let mut unified_env = env.clone();
    for e in envs {
        unified_env = unified_env.union(&e);
    }

    // Flatten the results into a single evaluated expression
    let mut evaled_items = Vec::new();
    for results in eval_results {
        // For now, take the first result (need to handle multiple results properly)
        if let Some(first) = results.first() {
            evaled_items.push(first.clone());
        }
    }

    // Check if this is a grounded operation
    if let Some(MettaValue::Atom(op)) = evaled_items.first() {
        if let Some(result) = try_eval_builtin(op, &evaled_items[1..]) {
            return (vec![result], unified_env);
        }
    }

    // Try to match against rules in the environment
    let sexpr = MettaValue::SExpr(evaled_items.clone());
    for rule in &unified_env.rules {
        if let Some(bindings) = pattern_match(&rule.lhs, &sexpr) {
            // Apply bindings to the rhs and evaluate
            let instantiated_rhs = apply_bindings(&rule.rhs, &bindings);
            return eval(instantiated_rhs, unified_env);
        }
    }

    // No rule matched, return the evaluated s-expression
    (vec![MettaValue::SExpr(evaled_items)], unified_env)
}

/// Evaluate if control flow: (if condition then-branch else-branch)
/// Only evaluates the chosen branch (lazy evaluation)
fn eval_if(args: &[MettaValue], env: Environment) -> EvalResult {
    if args.len() < 3 {
        let err = MettaValue::Error(
            "if requires 3 arguments: condition, then-branch, else-branch".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let condition = &args[0];
    let then_branch = &args[1];
    let else_branch = &args[2];

    // Evaluate the condition
    let (cond_results, env_after_cond) = eval(condition.clone(), env);

    // Check for error in condition
    if let Some(first) = cond_results.first() {
        if matches!(first, MettaValue::Error(_, _)) {
            return (vec![first.clone()], env_after_cond);
        }

        // Check if condition is true
        let is_true = match first {
            MettaValue::Bool(true) => true,
            MettaValue::Bool(false) => false,
            // Non-boolean values: treat as true if not Nil
            MettaValue::Nil => false,
            _ => true,
        };

        // Evaluate only the chosen branch
        if is_true {
            eval(then_branch.clone(), env_after_cond)
        } else {
            eval(else_branch.clone(), env_after_cond)
        }
    } else {
        // No result from condition - treat as false
        eval(else_branch.clone(), env_after_cond)
    }
}

/// Evaluate catch: error recovery mechanism
/// (catch expr default) - if expr returns error, evaluate and return default
/// This prevents error propagation (reduction prevention)
fn eval_catch(args: &[MettaValue], env: Environment) -> EvalResult {
    if args.len() < 2 {
        let err = MettaValue::Error(
            "catch requires 2 arguments: expr and default".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let expr = &args[0];
    let default = &args[1];

    // Evaluate the expression
    let (results, env_after_eval) = eval(expr.clone(), env);

    // Check if result is an error
    if let Some(first) = results.first() {
        if matches!(first, MettaValue::Error(_, _)) {
            // Error occurred - evaluate and return default instead
            // This PREVENTS the error from propagating further
            return eval(default.clone(), env_after_eval);
        }
    }

    // No error - return the result
    (results, env_after_eval)
}

/// Try to evaluate a built-in operation
/// Dispatches directly to built-in functions without going through Rholang interpreter
fn try_eval_builtin(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    match op {
        "add" => eval_binary_arithmetic(args, |a, b| a + b),
        "sub" => eval_binary_arithmetic(args, |a, b| a - b),
        "mul" => eval_binary_arithmetic(args, |a, b| a * b),
        "div" => eval_binary_arithmetic(args, |a, b| a / b),
        "lt" => eval_comparison(args, |a, b| a < b),
        "lte" => eval_comparison(args, |a, b| a <= b),
        "gt" => eval_comparison(args, |a, b| a > b),
        "gte" => eval_comparison(args, |a, b| a >= b),
        "eq" => eval_comparison(args, |a, b| a == b),
        "neq" => eval_comparison(args, |a, b| a != b),
        _ => None,
    }
}

/// Evaluate a binary arithmetic operation
fn eval_binary_arithmetic<F>(args: &[MettaValue], op: F) -> Option<MettaValue>
where
    F: Fn(i64, i64) -> i64,
{
    if args.len() != 2 {
        return None;
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        _ => return None,
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        _ => return None,
    };

    Some(MettaValue::Long(op(a, b)))
}

/// Evaluate a comparison operation
fn eval_comparison<F>(args: &[MettaValue], op: F) -> Option<MettaValue>
where
    F: Fn(i64, i64) -> bool,
{
    if args.len() != 2 {
        return None;
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        _ => return None,
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        _ => return None,
    };

    Some(MettaValue::Bool(op(a, b)))
}

/// Pattern match a pattern against a value
/// Returns bindings if successful, None otherwise
fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
    let mut bindings = Bindings::new();
    if pattern_match_impl(pattern, value, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

fn pattern_match_impl(pattern: &MettaValue, value: &MettaValue, bindings: &mut Bindings) -> bool {
    match (pattern, value) {
        // Wildcard matches anything
        (MettaValue::Atom(p), _) if p == "_" => true,

        // Variables (start with $, &, or ') bind to values
        (MettaValue::Atom(p), v) if p.starts_with('$') || p.starts_with('&') || p.starts_with('\'') => {
            // Check if variable is already bound
            if let Some(existing) = bindings.get(p) {
                existing == v
            } else {
                bindings.insert(p.clone(), v.clone());
                true
            }
        }

        // Atoms must match exactly
        (MettaValue::Atom(p), MettaValue::Atom(v)) => p == v,
        (MettaValue::Bool(p), MettaValue::Bool(v)) => p == v,
        (MettaValue::Long(p), MettaValue::Long(v)) => p == v,
        (MettaValue::String(p), MettaValue::String(v)) => p == v,
        (MettaValue::Uri(p), MettaValue::Uri(v)) => p == v,
        (MettaValue::Nil, MettaValue::Nil) => true,

        // S-expressions must have same length and all elements must match
        (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items)) => {
            if p_items.len() != v_items.len() {
                return false;
            }
            for (p, v) in p_items.iter().zip(v_items.iter()) {
                if !pattern_match_impl(p, v, bindings) {
                    return false;
                }
            }
            true
        }

        // Errors match if message and details match
        (MettaValue::Error(p_msg, p_details), MettaValue::Error(v_msg, v_details)) => {
            p_msg == v_msg && pattern_match_impl(p_details, v_details, bindings)
        }

        _ => false,
    }
}

/// Apply variable bindings to a value
fn apply_bindings(value: &MettaValue, bindings: &Bindings) -> MettaValue {
    match value {
        MettaValue::Atom(s) if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') => {
            bindings.get(s).cloned().unwrap_or_else(|| value.clone())
        }
        MettaValue::SExpr(items) => {
            let new_items: Vec<_> = items.iter()
                .map(|item| apply_bindings(item, bindings))
                .collect();
            MettaValue::SExpr(new_items)
        }
        MettaValue::Error(msg, details) => {
            let new_details = apply_bindings(details, bindings);
            MettaValue::Error(msg.clone(), Box::new(new_details))
        }
        _ => value.clone(),
    }
}

/// Infer the type of an expression
/// Returns a MettaValue representing the type
fn infer_type(expr: &MettaValue, env: &Environment) -> MettaValue {
    match expr {
        // Ground types have built-in types
        MettaValue::Bool(_) => MettaValue::Atom("Bool".to_string()),
        MettaValue::Long(_) => MettaValue::Atom("Number".to_string()),
        MettaValue::String(_) => MettaValue::Atom("String".to_string()),
        MettaValue::Uri(_) => MettaValue::Atom("URI".to_string()),
        MettaValue::Nil => MettaValue::Atom("Nil".to_string()),

        // Type values have type Type
        MettaValue::Type(_) => MettaValue::Atom("Type".to_string()),

        // Errors have Error type
        MettaValue::Error(_, _) => MettaValue::Atom("Error".to_string()),

        // For atoms, look up in environment
        MettaValue::Atom(name) => {
            // Check if it's a variable (starts with $, &, or ')
            if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
                // Type variable - return as-is wrapped in Type
                return MettaValue::Type(Box::new(MettaValue::Atom(name.clone())));
            }

            // Look up type in environment
            env.get_type(name)
                .cloned()
                .unwrap_or_else(|| MettaValue::Atom("Undefined".to_string()))
        }

        // For s-expressions, try to infer from function application
        MettaValue::SExpr(items) => {
            if items.is_empty() {
                return MettaValue::Atom("Nil".to_string());
            }

            // Get the operator/function
            if let Some(MettaValue::Atom(op)) = items.first() {
                // Check for built-in operators
                match op.as_str() {
                    "add" | "sub" | "mul" | "div" => {
                        return MettaValue::Atom("Number".to_string());
                    }
                    "lt" | "lte" | "gt" | "gte" | "eq" | "neq" => {
                        return MettaValue::Atom("Bool".to_string());
                    }
                    "->" => {
                        // Arrow type constructor
                        return MettaValue::Atom("Type".to_string());
                    }
                    _ => {
                        // Look up function type in environment
                        if let Some(func_type) = env.get_type(op) {
                            // If it's an arrow type, extract return type
                            if let MettaValue::SExpr(type_items) = func_type {
                                if let Some(MettaValue::Atom(arrow)) = type_items.first() {
                                    if arrow == "->" && type_items.len() > 1 {
                                        // Return type is last element
                                        return type_items.last().cloned().unwrap();
                                    }
                                }
                            }
                            return func_type.clone();
                        }
                    }
                }
            }

            // Can't infer type
            MettaValue::Atom("Undefined".to_string())
        }
    }
}

/// Check if two types match
/// Handles type variables and structural equality
fn types_match(actual: &MettaValue, expected: &MettaValue) -> bool {
    match (actual, expected) {
        // Type variables match anything
        (_, MettaValue::Atom(e)) if e.starts_with('$') => true,
        (MettaValue::Atom(a), _) if a.starts_with('$') => true,

        // Type variables in Type wrapper
        (_, MettaValue::Type(e)) => {
            if let MettaValue::Atom(name) = e.as_ref() {
                if name.starts_with('$') {
                    return true;
                }
            }
            // Otherwise, unwrap and compare
            if let MettaValue::Type(a) = actual {
                types_match(a, e)
            } else {
                false
            }
        }

        // Exact atom matches
        (MettaValue::Atom(a), MettaValue::Atom(e)) => a == e,

        // Bool matches
        (MettaValue::Bool(a), MettaValue::Bool(e)) => a == e,

        // Long matches
        (MettaValue::Long(a), MettaValue::Long(e)) => a == e,

        // String matches
        (MettaValue::String(a), MettaValue::String(e)) => a == e,

        // S-expression matches (structural equality)
        (MettaValue::SExpr(a_items), MettaValue::SExpr(e_items)) => {
            if a_items.len() != e_items.len() {
                return false;
            }
            a_items.iter().zip(e_items.iter()).all(|(a, e)| types_match(a, e))
        }

        // Nil matches Nil
        (MettaValue::Nil, MettaValue::Nil) => true,

        // Default: no match
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::types::Rule;

    #[test]
    fn test_eval_atom() {
        let env = Environment::new();
        let value = MettaValue::Atom("foo".to_string());
        let (results, _) = eval(value.clone(), env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], value);
    }

    #[test]
    fn test_eval_builtin_add() {
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_eval_builtin_comparison() {
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("lt".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_pattern_match_simple() {
        let pattern = MettaValue::Atom("$x".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.unwrap();
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
    }

    #[test]
    fn test_pattern_match_sexpr() {
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]);
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.unwrap();
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(1)));
    }

    #[test]
    fn test_eval_with_rule() {
        let mut env = Environment::new();

        // Add rule: (= (double $x) (mul $x 2))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("mul".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        };
        env.add_rule(rule);

        // Evaluate (double 5)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Long(5),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    // === Error Handling Tests ===

    #[test]
    fn test_error_propagation() {
        let env = Environment::new();

        // Create an error
        let error = MettaValue::Error(
            "test error".to_string(),
            Box::new(MettaValue::Long(42)),
        );

        // Errors should propagate unchanged
        let (results, _) = eval(error.clone(), env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], error);
    }

    #[test]
    fn test_error_in_subexpression() {
        let env = Environment::new();

        // (+ (error "fail" 42) 10)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("fail".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Long(10),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // Should return the error
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "fail");
            }
            other => panic!("Expected error, got {:?}", other),
        }
    }

    #[test]
    fn test_error_construction() {
        let env = Environment::new();

        // (error "my error" (+ 1 2))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("error".to_string()),
            MettaValue::String("my error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert_eq!(msg, "my error");
                // Details should be unevaluated
                match **details {
                    MettaValue::SExpr(_) => {}
                    _ => panic!("Expected SExpr as error details"),
                }
            }
            _ => panic!("Expected error"),
        }
    }

    // === Control Flow Tests ===

    #[test]
    fn test_if_true_branch() {
        let env = Environment::new();

        // (if true (+ 1 2) (+ 3 4))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(true),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3)); // 1 + 2
    }

    #[test]
    fn test_if_false_branch() {
        let env = Environment::new();

        // (if false (+ 1 2) (+ 3 4))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(false),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7)); // 3 + 4
    }

    #[test]
    fn test_if_with_comparison() {
        let env = Environment::new();

        // (if (< 1 2) "yes" "no")
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("lt".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::String("yes".to_string()),
            MettaValue::String("no".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("yes".to_string()));
    }

    #[test]
    fn test_if_only_evaluates_chosen_branch() {
        let env = Environment::new();

        // (if true 1 (error "should not evaluate"))
        // The error in the else branch should not be evaluated
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(true),
            MettaValue::Long(1),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("should not evaluate".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1)); // No error!
    }

    // === Quote Tests ===

    #[test]
    fn test_quote_prevents_evaluation() {
        let env = Environment::new();

        // (quote (+ 1 2))
        // Should return the expression unevaluated
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // Should be the unevaluated s-expression with "add" not evaluated to 3
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::Atom("add".to_string()));
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

    // === Rule Definition and Evaluation Tests ===

    #[test]
    fn test_rule_definition() {
        let env = Environment::new();

        // (= (f) 42)
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
            MettaValue::Long(42),
        ]);

        let (result, new_env) = eval(rule_def, env);

        // Rule definition should return Nil
        assert_eq!(result[0], MettaValue::Nil);

        // Environment should now have the rule
        assert_eq!(new_env.rules.len(), 1);
    }

    #[test]
    fn test_evaluation_with_exclaim() {
        let env = Environment::new();

        // First define a rule: (= (f) 42)
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
            MettaValue::Long(42),
        ]);
        let (_result, new_env) = eval(rule_def, env);

        // Now evaluate: (! (f))
        let eval_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("!".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
        ]);

        let (result, _) = eval(eval_expr, new_env);

        // Should get 42
        assert_eq!(result[0], MettaValue::Long(42));
    }

    // === Reduction Prevention Tests ===

    #[test]
    fn test_catch_with_error() {
        let env = Environment::new();

        // (catch (error "fail" 42) "recovered")
        // Should return "recovered" instead of propagating error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("fail".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::String("recovered".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("recovered".to_string()));
    }

    #[test]
    fn test_catch_without_error() {
        let env = Environment::new();

        // (catch (+ 1 2) "default")
        // Should return 3 (no error occurred)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::String("default".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_catch_prevents_error_propagation() {
        let env = Environment::new();

        // (+ 10 (catch (error "fail" 0) 5))
        // The error should be caught and replaced with 5, so result is 15
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::Atom("catch".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("error".to_string()),
                    MettaValue::String("fail".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::Long(5),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(15));
    }

    #[test]
    fn test_eval_with_quote() {
        let env = Environment::new();

        // (eval (quote (+ 1 2)))
        // Quote prevents evaluation, eval forces it
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("eval".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("add".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_is_error_with_error() {
        let env = Environment::new();

        // (is-error (error "test" 42))
        // Should return true
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("test".to_string()),
                MettaValue::Long(42),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_is_error_with_normal_value() {
        let env = Environment::new();

        // (is-error (+ 1 2))
        // Should return false
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_reduction_prevention_combo() {
        let env = Environment::new();

        // Complex reduction prevention:
        // (if (is-error (catch (/ 10 0) (error "caught" 0)))
        //     "has-error"
        //     "no-error")
        // catch prevents error, but creates new error, is-error detects it
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("is-error".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("catch".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("error".to_string()),
                        MettaValue::String("div-by-zero".to_string()),
                        MettaValue::Long(0),
                    ]),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("error".to_string()),
                        MettaValue::String("caught".to_string()),
                        MettaValue::Long(0),
                    ]),
                ]),
            ]),
            MettaValue::String("has-error".to_string()),
            MettaValue::String("no-error".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("has-error".to_string()));
    }

    // === Integration Test ===

    #[test]
    fn test_mvp_complete() {
        let mut env = Environment::new();

        // Add a rule: (= (safe-div $x $y) (if (== $y 0) (error "division by zero" $y) (div $x $y)))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("safe-div".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("eq".to_string()),
                    MettaValue::Atom("$y".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("error".to_string()),
                    MettaValue::String("division by zero".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("div".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
        };
        env.add_rule(rule);

        // Test successful division: (safe-div 10 2) -> 5
        let value1 = MettaValue::SExpr(vec![
            MettaValue::Atom("safe-div".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(2),
        ]);
        let (results1, env1) = eval(value1, env.clone());
        assert_eq!(results1[0], MettaValue::Long(5));

        // Test division by zero: (safe-div 10 0) -> Error
        let value2 = MettaValue::SExpr(vec![
            MettaValue::Atom("safe-div".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(0),
        ]);
        let (results2, _) = eval(value2, env1);
        match &results2[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "division by zero");
            }
            other => panic!("Expected error, got {:?}", other),
        }
    }

    // === Type System Tests ===

    #[test]
    fn test_type_assertion() {
        let env = Environment::new();

        // (: x Number)
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);

        let (result, new_env) = eval(type_assertion, env);

        // Type assertion should return Nil
        assert_eq!(result[0], MettaValue::Nil);

        // Environment should have the type assertion
        assert_eq!(
            new_env.get_type("x"),
            Some(&MettaValue::Atom("Number".to_string()))
        );
    }

    #[test]
    fn test_get_type_ground_types() {
        let env = Environment::new();

        // (get-type 42) -> Number
        let get_type_long = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Long(42),
        ]);
        let (result, _) = eval(get_type_long, env.clone());
        assert_eq!(result[0], MettaValue::Atom("Number".to_string()));

        // (get-type true) -> Bool
        let get_type_bool = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Bool(true),
        ]);
        let (result, _) = eval(get_type_bool, env.clone());
        assert_eq!(result[0], MettaValue::Atom("Bool".to_string()));

        // (get-type "hello") -> String
        let get_type_string = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::String("hello".to_string()),
        ]);
        let (result, _) = eval(get_type_string, env);
        assert_eq!(result[0], MettaValue::Atom("String".to_string()));
    }

    #[test]
    fn test_get_type_with_assertion() {
        let mut env = Environment::new();

        // Add type assertion: (: foo Number)
        env.add_type("foo".to_string(), MettaValue::Atom("Number".to_string()));

        // (get-type foo) -> Number
        let get_type = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Atom("foo".to_string()),
        ]);

        let (result, _) = eval(get_type, env);
        assert_eq!(result[0], MettaValue::Atom("Number".to_string()));
    }

    #[test]
    fn test_get_type_builtin_operations() {
        let env = Environment::new();

        // (get-type (add 1 2)) -> Number
        let get_type_add = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);
        let (result, _) = eval(get_type_add, env.clone());
        assert_eq!(result[0], MettaValue::Atom("Number".to_string()));

        // (get-type (lt 1 2)) -> Bool
        let get_type_lt = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("lt".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);
        let (result, _) = eval(get_type_lt, env);
        assert_eq!(result[0], MettaValue::Atom("Bool".to_string()));
    }

    #[test]
    fn test_check_type() {
        let mut env = Environment::new();

        // Add type assertion: (: x Number)
        env.add_type("x".to_string(), MettaValue::Atom("Number".to_string()));

        // (check-type x Number) -> true
        let check_type_match = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);
        let (result, _) = eval(check_type_match, env.clone());
        assert_eq!(result[0], MettaValue::Bool(true));

        // (check-type x String) -> false
        let check_type_mismatch = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("String".to_string()),
        ]);
        let (result, _) = eval(check_type_mismatch, env);
        assert_eq!(result[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_check_type_with_type_variables() {
        let env = Environment::new();

        // (check-type 42 $t) -> true (type variable matches anything)
        let check_type_var = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$t".to_string()),
        ]);
        let (result, _) = eval(check_type_var, env);
        assert_eq!(result[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_arrow_type_assertion() {
        let mut env = Environment::new();

        // (: add (-> Number Number Number))
        let arrow_type = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("add".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("->".to_string()),
                MettaValue::Atom("Number".to_string()),
                MettaValue::Atom("Number".to_string()),
                MettaValue::Atom("Number".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(arrow_type, env);
        env = new_env;

        // Should return Nil
        assert_eq!(result[0], MettaValue::Nil);

        // Get the type back
        let arrow_type_expected = MettaValue::SExpr(vec![
            MettaValue::Atom("->".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);
        assert_eq!(env.get_type("add"), Some(&arrow_type_expected));
    }

    #[test]
    fn test_integration_with_rules_and_types() {
        let mut env = Environment::new();

        // Add type assertion: (: double (-> Number Number))
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("double".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("->".to_string()),
                MettaValue::Atom("Number".to_string()),
                MettaValue::Atom("Number".to_string()),
            ]),
        ]);
        let (_, new_env) = eval(type_assertion, env);
        env = new_env;

        // Add rule: (= (double $x) (mul $x 2))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("mul".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        };
        env.add_rule(rule);

        // Check type of double
        let get_type = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Atom("double".to_string()),
        ]);
        let (result, _) = eval(get_type, env.clone());

        let expected_type = MettaValue::SExpr(vec![
            MettaValue::Atom("->".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);
        assert_eq!(result[0], expected_type);

        // Evaluate (double 5) -> 10
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Long(5),
        ]);
        let (result, _) = eval(value, env);
        assert_eq!(result[0], MettaValue::Long(10));
    }
}
