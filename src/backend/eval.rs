// Eval function: Lazy evaluation with pattern matching and built-in dispatch
//
// eval(a: atom, env) = a, env
// eval((t1 .. tn), env):
//   r1, env_1 = eval(t1, env) | ... | rn, env_n = eval(tn, env)
//   env' = union env_i
//   return fold over rules & grounded functions (emptyset, env')

use crate::backend::environment::Environment;
use crate::backend::models::{Bindings, EvalResult, MettaValue, Rule};
use crate::backend::mork_convert::{
    metta_to_mork_bytes, mork_bindings_to_metta, ConversionContext,
};
use mork_expr::Expr;

/// Evaluate a MettaValue in the given environment
/// Returns (results, new_environment)
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    match value {
        // Errors propagate immediately without further evaluation
        MettaValue::Error(_, _) => (vec![value], env),

        // For atoms: add bare symbols to MORK Space, then return as-is
        MettaValue::Atom(_) => {
            // Atoms evaluate to themselves without being stored in the space
            // Only rules (via =), type assertions (via :), and unmatched s-expressions
            // are stored in the MORK space
            (vec![value], env)
        }

        // For other ground types, return as-is
        MettaValue::Bool(_)
        | MettaValue::Long(_)
        | MettaValue::String(_)
        | MettaValue::Uri(_)
        | MettaValue::Nil
        | MettaValue::Type(_) => (vec![value], env),

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
            // Rule definition: (= lhs rhs) - add to MORK Space and rule cache
            "=" => {
                if items.len() >= 3 {
                    let lhs = items[1].clone();
                    let rhs = items[2].clone();

                    let mut new_env = env.clone();

                    // Add rule using add_rule (stores in both rule_cache and MORK Space)
                    new_env.add_rule(Rule { lhs, rhs });

                    // Return empty list (rule definitions don't produce output)
                    return (vec![], new_env);
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

            // Match: pattern matching against atom space
            // (match <space> <pattern> <template>)
            // Searches space for all atoms matching pattern and returns instantiated templates
            "match" => {
                return eval_match(&items[1..], env);
            }

            // Let: local variable binding
            // (let $var value body) - Bind variable to value and evaluate body with that binding
            // Supports pattern matching: (let ($x $y) (tuple 1 2) body)
            "let" => {
                return eval_let(&items[1..], env);
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

                    // Add the type assertion to MORK Space
                    let type_expr = MettaValue::SExpr(items);
                    new_env.add_to_space(&type_expr);

                    // Return empty list (type assertions don't produce output)
                    return (vec![], new_env);
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

    // Handle nondeterministic evaluation: generate Cartesian product of all sub-expression results
    // When any sub-expression returns multiple results, we need to try all combinations
    let combinations = cartesian_product(&eval_results);

    // Collect all final results from all combinations
    let mut all_final_results = Vec::new();

    for evaled_items in combinations {
        // Check if this is a grounded operation
        if let Some(MettaValue::Atom(op)) = evaled_items.first() {
            if let Some(result) = try_eval_builtin(op, &evaled_items[1..]) {
                all_final_results.push(result);
                continue; // Move to next combination
            }
        }

        // Try to match against rules in MORK Space
        let sexpr = MettaValue::SExpr(evaled_items.clone());

        // Collect ALL matching rules with the BEST specificity (MeTTa returns multiple results)
        // The helper function already filters to return only rules with the best specificity
        let all_matches = try_match_all_rules(&sexpr, &unified_env);

        if !all_matches.is_empty() {
            // Evaluate all matching rule bodies (all have the same specificity)
            for (rhs, bindings) in all_matches {
                // Apply bindings to RHS and evaluate
                let instantiated_rhs = apply_bindings(&rhs, &bindings);
                let (results, _) = eval(instantiated_rhs, unified_env.clone());
                all_final_results.extend(results);
            }
        } else {
            // No rule matched, add to MORK Space and return it
            let mut final_env = unified_env.clone();
            final_env.add_to_space(&sexpr);
            all_final_results.push(sexpr);
        }
    }

    (all_final_results, unified_env)
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

/// Evaluate match: (match <space-ref> <space-name> <pattern> <template>)
/// Searches the space for all atoms matching the pattern and returns instantiated templates
///
/// Optimized to use Environment::match_space which performs pattern matching
/// directly on MORK expressions without unnecessary intermediate allocations
fn eval_match(args: &[MettaValue], env: Environment) -> EvalResult {
    if args.len() < 4 {
        let err = MettaValue::Error(
            "match requires 4 arguments: &, space-name, pattern, and template".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let space_ref = &args[0];
    let space_name = &args[1];
    let pattern = &args[2];
    let template = &args[3];

    // Check that first arg is & (space reference operator)
    match space_ref {
        MettaValue::Atom(s) if s == "&" => {
            // Check space name (for now, only support "self")
            match space_name {
                MettaValue::Atom(name) if name == "self" => {
                    // Use optimized match_space method that works directly with MORK
                    let results = env.match_space(pattern, template);
                    (results, env)
                }
                _ => {
                    let err = MettaValue::Error(
                        format!(
                            "match only supports 'self' as space name, got: {:?}",
                            space_name
                        ),
                        Box::new(MettaValue::SExpr(args.to_vec())),
                    );
                    (vec![err], env)
                }
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!("match requires & as first argument, got: {:?}", space_ref),
                Box::new(MettaValue::SExpr(args.to_vec())),
            );
            (vec![err], env)
        }
    }
}

/// Evaluate let binding: (let pattern value body)
/// Evaluates value, binds it to pattern, and evaluates body with those bindings
/// Supports both simple variable binding and pattern matching:
///   - (let $x 42 body) - simple binding
///   - (let ($a $b) (tuple 1 2) body) - destructuring pattern
fn eval_let(args: &[MettaValue], env: Environment) -> EvalResult {
    if args.len() < 3 {
        let err = MettaValue::Error(
            "let requires 3 arguments: pattern, value, and body".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let value_expr = &args[1];
    let body = &args[2];

    // Evaluate the value expression first
    let (value_results, value_env) = eval(value_expr.clone(), env);

    // Handle nondeterminism: if value evaluates to multiple results, try each one
    let mut all_results = Vec::new();

    for value in value_results {
        // Try to match the pattern against the value
        if let Some(bindings) = pattern_match(pattern, &value) {
            // Apply bindings to the body and evaluate it
            let instantiated_body = apply_bindings(body, &bindings);
            let (body_results, _) = eval(instantiated_body, value_env.clone());
            all_results.extend(body_results);
        } else {
            // Pattern match failed
            let err = MettaValue::Error(
                format!("let pattern {:?} does not match value {:?}", pattern, value),
                Box::new(MettaValue::SExpr(args.to_vec())),
            );
            all_results.push(err);
        }
    }

    (all_results, value_env)
}

/// Try to evaluate a built-in operation
/// Dispatches directly to built-in functions without going through Rholang interpreter
/// Uses operator symbols (+, -, *, etc.) instead of normalized names
fn try_eval_builtin(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    match op {
        "+" => eval_binary_arithmetic(args, |a, b| a + b),
        "-" => eval_binary_arithmetic(args, |a, b| a - b),
        "*" => eval_binary_arithmetic(args, |a, b| a * b),
        "/" => eval_binary_arithmetic(args, |a, b| a / b),
        "<" => eval_comparison(args, |a, b| a < b),
        "<=" => eval_comparison(args, |a, b| a <= b),
        ">" => eval_comparison(args, |a, b| a > b),
        ">=" => eval_comparison(args, |a, b| a >= b),
        "==" => eval_comparison(args, |a, b| a == b),
        "!=" => eval_comparison(args, |a, b| a != b),
        _ => None,
    }
}

/// Evaluate a binary arithmetic operation with strict type checking
fn eval_binary_arithmetic<F>(args: &[MettaValue], op: F) -> Option<MettaValue>
where
    F: Fn(i64, i64) -> i64,
{
    if args.len() != 2 {
        return Some(MettaValue::Error(
            format!(
                "Arithmetic operation requires exactly 2 arguments, got {}",
                args.len()
            ),
            Box::new(MettaValue::Nil),
        ));
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!("{:?}", other),
                Box::new(MettaValue::Atom("BadType".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!("{:?}", other),
                Box::new(MettaValue::Atom("BadType".to_string())),
            ));
        }
    };

    Some(MettaValue::Long(op(a, b)))
}

/// Evaluate a comparison operation with strict type checking
fn eval_comparison<F>(args: &[MettaValue], op: F) -> Option<MettaValue>
where
    F: Fn(i64, i64) -> bool,
{
    if args.len() != 2 {
        return Some(MettaValue::Error(
            format!(
                "Comparison operation requires exactly 2 arguments, got {}",
                args.len()
            ),
            Box::new(MettaValue::Nil),
        ));
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!("{:?}", other),
                Box::new(MettaValue::Atom("BadType".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!("{:?}", other),
                Box::new(MettaValue::Atom("BadType".to_string())),
            ));
        }
    };

    Some(MettaValue::Bool(op(a, b)))
}

/// Pattern match a pattern against a value
/// Returns bindings if successful, None otherwise
///
/// This is made public to support optimized match operations in Environment
pub(crate) fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
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
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        (MettaValue::Atom(p), v)
            if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\'')) && p != "&" =>
        {
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

/// Generate Cartesian product of evaluation results for nondeterministic evaluation
/// When sub-expressions return multiple results, we need to try all combinations
///
/// Example: [[a, b], [1, 2]] -> [[a, 1], [a, 2], [b, 1], [b, 2]]
fn cartesian_product(results: &[Vec<MettaValue>]) -> Vec<Vec<MettaValue>> {
    if results.is_empty() {
        return vec![vec![]];
    }

    // Base case: single result list
    if results.len() == 1 {
        return results[0].iter().map(|item| vec![item.clone()]).collect();
    }

    // Recursive case: combine first list with Cartesian product of rest
    let first = &results[0];
    let rest_product = cartesian_product(&results[1..]);

    let mut product = Vec::new();
    for item in first {
        for rest_combo in &rest_product {
            let mut combo = vec![item.clone()];
            combo.extend(rest_combo.clone());
            product.push(combo);
        }
    }

    product
}

/// Apply variable bindings to a value
///
/// This is made public to support optimized match operations in Environment
pub(crate) fn apply_bindings(value: &MettaValue, bindings: &Bindings) -> MettaValue {
    match value {
        // Apply bindings to variables (atoms starting with $, &, or ')
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValue::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && s != "&" =>
        {
            bindings.get(s).cloned().unwrap_or_else(|| value.clone())
        }
        MettaValue::SExpr(items) => {
            let new_items: Vec<_> = items
                .iter()
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

/// Extract the head symbol from a pattern for indexing
/// Returns None if the pattern doesn't have a clear head symbol
fn get_head_symbol(pattern: &MettaValue) -> Option<String> {
    match pattern {
        // For s-expressions like (double $x), extract "double"
        // EXCEPT: standalone "&" is allowed as a head symbol (used in match)
        MettaValue::SExpr(items) if !items.is_empty() => match &items[0] {
            MettaValue::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || head == "&")
                    && !head.starts_with('\'')
                    && head != "_" =>
            {
                Some(head.clone())
            }
            _ => None,
        },
        // For bare atoms like foo, use the atom itself
        // EXCEPT: standalone "&" is allowed (used in match)
        MettaValue::Atom(head)
            if !head.starts_with('$')
                && (!head.starts_with('&') || head == "&")
                && !head.starts_with('\'')
                && head != "_" =>
        {
            Some(head.clone())
        }
        _ => None,
    }
}

/// Compute the specificity of a pattern (lower is more specific)
/// More specific patterns have fewer variables
fn pattern_specificity(pattern: &MettaValue) -> usize {
    match pattern {
        // Variables are least specific
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValue::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') || s == "_")
                && s != "&" =>
        {
            1000 // Variables are least specific
        }
        MettaValue::Atom(_)
        | MettaValue::Bool(_)
        | MettaValue::Long(_)
        | MettaValue::String(_)
        | MettaValue::Uri(_)
        | MettaValue::Nil => {
            0 // Literals are most specific (including standalone "&")
        }
        MettaValue::SExpr(items) => {
            // Sum specificity of all items
            items.iter().map(pattern_specificity).sum()
        }
        // Errors: use specificity of details
        MettaValue::Error(_, details) => pattern_specificity(details),
        // Types: use specificity of inner type
        MettaValue::Type(t) => pattern_specificity(t),
    }
}

/// Find ALL rules in the environment that match the given expression
/// Returns Vec<(rhs, bindings)> with all matching rules
///
/// This function supports MeTTa's non-deterministic semantics where multiple rules
/// can match the same expression and all results should be returned.
fn try_match_all_rules(expr: &MettaValue, env: &Environment) -> Vec<(MettaValue, Bindings)> {
    // Try query_multi optimization first
    let query_multi_results = try_match_all_rules_query_multi(expr, env);
    if !query_multi_results.is_empty() {
        return query_multi_results;
    }

    // Fall back to iteration-based approach
    try_match_all_rules_iterative(expr, env)
}

/// Try pattern matching using MORK's query_multi to find ALL matching rules (O(k) where k = matching rules)
fn try_match_all_rules_query_multi(
    expr: &MettaValue,
    env: &Environment,
) -> Vec<(MettaValue, Bindings)> {
    // Create a pattern that queries for rules: (= <expr-pattern> $rhs)
    // This will find all rules where the LHS matches our expression
    let space = env.space.lock().unwrap();

    // Convert expression to MORK format for querying
    let mut ctx = ConversionContext::new();
    let expr_bytes = match metta_to_mork_bytes(expr, &space, &mut ctx) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(), // Fallback to iterative if conversion fails
    };

    // Create a query pattern: (= <expr> $rhs)
    let pattern_str = format!("(= {} $rhs)", String::from_utf8_lossy(&expr_bytes));
    let pattern_bytes = pattern_str.as_bytes();

    // Parse the pattern using MORK's parser
    let mut parse_buffer = vec![0u8; 4096];
    let mut pdp = mork::space::ParDataParser::new(&space.sm);
    use mork_frontend::bytestring_parser::Parser;
    let mut ez = mork_expr::ExprZipper::new(Expr {
        ptr: parse_buffer.as_mut_ptr(),
    });
    let mut context = mork_frontend::bytestring_parser::Context::new(pattern_bytes);

    if pdp.sexpr(&mut context, &mut ez).is_err() {
        return Vec::new(); // Fallback if parsing fails
    }

    let pattern_expr = Expr {
        ptr: parse_buffer.as_ptr().cast_mut(),
    };

    // Collect ALL matches using query_multi
    // Note: All matches from query_multi will have the same LHS pattern (since we're querying for it)
    // Therefore, they all have the same LHS specificity and we should return all of them
    let mut matches: Vec<(MettaValue, Bindings)> = Vec::new();

    mork::space::Space::query_multi(&space.btm, pattern_expr, |result, _matched_expr| {
        if let Err(bindings) = result {
            // Convert MORK bindings to our format
            if let Ok(our_bindings) = mork_bindings_to_metta(&bindings, &ctx, &space) {
                // Extract the RHS from bindings
                if let Some(rhs) = our_bindings.get("$rhs") {
                    matches.push((rhs.clone(), our_bindings));
                }
            }
        }
        true // Continue searching for ALL matches
    });

    matches
    // space will be dropped automatically here
}

/// Fallback: Try pattern matching using iteration to find ALL matching rules (O(n) where n = total rules)
fn try_match_all_rules_iterative(
    expr: &MettaValue,
    env: &Environment,
) -> Vec<(MettaValue, Bindings)> {
    // Try to extract head symbol for filtering
    let target_head = get_head_symbol(expr);

    // Collect all matching rules
    let mut matching_rules: Vec<Rule> = Vec::new();

    // First pass: collect rules with matching head symbol
    if let Some(ref head) = target_head {
        for rule in env.iter_rules() {
            if let Some(rule_head) = rule.lhs.get_head_symbol() {
                if &rule_head == head {
                    matching_rules.push(rule);
                }
            }
        }
    }

    // Second pass: collect rules without a head symbol (e.g., variable patterns)
    for rule in env.iter_rules() {
        if rule.lhs.get_head_symbol().is_none() {
            matching_rules.push(rule);
        }
    }

    // Sort rules by specificity (more specific first)
    matching_rules.sort_by_key(|rule| pattern_specificity(&rule.lhs));

    // Collect ALL matching rules, tracking LHS specificity
    let mut matches: Vec<(MettaValue, Bindings, usize, Rule)> = Vec::new();
    for rule in matching_rules {
        if let Some(bindings) = pattern_match(&rule.lhs, expr) {
            let lhs_specificity = pattern_specificity(&rule.lhs);
            matches.push((rule.rhs.clone(), bindings, lhs_specificity, rule));
        }
    }

    // Find the best (lowest) specificity
    if let Some(best_spec) = matches.iter().map(|(_, _, spec, _)| *spec).min() {
        // Filter to only matches with the best specificity
        let best_matches: Vec<_> = matches
            .into_iter()
            .filter(|(_, _, spec, _)| *spec == best_spec)
            .collect();

        // Duplicate results based on rule count
        let mut final_matches = Vec::new();
        for (rhs, bindings, _, rule) in best_matches {
            let count = env.get_rule_count(&rule);
            for _ in 0..count {
                final_matches.push((rhs.clone(), bindings.clone()));
            }
        }
        final_matches
    } else {
        Vec::new()
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
                .unwrap_or_else(|| MettaValue::Atom("Undefined".to_string()))
        }

        // For s-expressions, try to infer from function application
        MettaValue::SExpr(items) => {
            if items.is_empty() {
                return MettaValue::Atom("Nil".to_string());
            }

            // Get the operator/function
            if let Some(MettaValue::Atom(op)) = items.first() {
                // Check for built-in operators (using symbols, not normalized names)
                match op.as_str() {
                    "+" | "-" | "*" | "/" => {
                        return MettaValue::Atom("Number".to_string());
                    }
                    "<" | "<=" | ">" | ">=" | "==" | "!=" => {
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
                            if let MettaValue::SExpr(ref type_items) = func_type {
                                if let Some(MettaValue::Atom(arrow)) = type_items.first() {
                                    if arrow == "->" && type_items.len() > 1 {
                                        // Return type is last element
                                        return type_items.last().cloned().unwrap();
                                    }
                                }
                            }
                            return func_type;
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
            a_items
                .iter()
                .zip(e_items.iter())
                .all(|(a, e)| types_match(a, e))
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
    use crate::backend::models::Rule;

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
            MettaValue::Atom("+".to_string()),
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
            MettaValue::Atom("<".to_string()),
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
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]);
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
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
                MettaValue::Atom("*".to_string()),
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
        let error = MettaValue::Error("test error".to_string(), Box::new(MettaValue::Long(42)));

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
            MettaValue::Atom("+".to_string()),
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
                MettaValue::Atom("+".to_string()),
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
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
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
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
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
                MettaValue::Atom("<".to_string()),
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

        let (result, _new_env) = eval(rule_def, env);

        // Rule definition should return empty list
        assert!(result.is_empty());
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
                MettaValue::Atom("+".to_string()),
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
            MettaValue::Atom("+".to_string()),
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
                    MettaValue::Atom("+".to_string()),
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
                MettaValue::Atom("+".to_string()),
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
                    MettaValue::Atom("==".to_string()),
                    MettaValue::Atom("$y".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("error".to_string()),
                    MettaValue::String("division by zero".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("/".to_string()),
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

        // Type assertion should return empty list
        assert!(result.is_empty());

        // Environment should have the type assertion
        assert_eq!(
            new_env.get_type("x"),
            Some(MettaValue::Atom("Number".to_string()))
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
                MettaValue::Atom("+".to_string()),
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
                MettaValue::Atom("<".to_string()),
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
        // Using a user-defined function name instead of builtin "+"
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

        // Should return empty list
        assert!(result.is_empty());

        // Get the type back
        let arrow_type_expected = MettaValue::SExpr(vec![
            MettaValue::Atom("->".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);
        assert_eq!(env.get_type("add"), Some(arrow_type_expected));
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
                MettaValue::Atom("*".to_string()),
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

    // === Tests adapted from hyperon-experimental ===
    // Source: https://github.com/trueagi-io/hyperon-experimental

    #[test]
    fn test_nested_arithmetic() {
        // From c1_grounded_basic.metta: (+ 2 (* 3 5))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(5),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(17)); // 2 + (3 * 5) = 17
    }

    #[test]
    fn test_comparison_with_arithmetic() {
        // From c1_grounded_basic.metta: (< 4 (+ 2 (* 3 5)))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(4),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Long(3),
                    MettaValue::Long(5),
                ]),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Bool(true)); // 4 < 17
    }

    #[test]
    fn test_equality_literals() {
        // From c1_grounded_basic.metta: (== 4 (+ 2 2))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("==".to_string()),
            MettaValue::Long(4),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(2),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Bool(true)); // 4 == 4
    }

    #[test]
    fn test_equality_sexpr() {
        // From c1_grounded_basic.metta: structural equality tests
        let env = Environment::new();

        // (== (A B) (A B)) should be supported via pattern matching
        // For now we test that equal atoms are equal
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("==".to_string()),
            MettaValue::Long(42),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_factorial_recursive() {
        // From c1_grounded_basic.metta: factorial example with if guard
        // (= (fact $n) (if (> $n 0) (* $n (fact (- $n 1))) 1))
        let mut env = Environment::new();

        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("fact".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                // Condition: (> $n 0)
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(0),
                ]),
                // Then branch: (* $n (fact (- $n 1)))
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("fact".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("-".to_string()),
                            MettaValue::Atom("$n".to_string()),
                            MettaValue::Long(1),
                        ]),
                    ]),
                ]),
                // Else branch: 1
                MettaValue::Long(1),
            ]),
        };
        env.add_rule(rule);

        // Test (fact 3) = 6
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));
    }

    #[test]
    fn test_factorial_with_compile() {
        // Test factorial using compile() to ensure the compiled version works
        // This complements test_factorial_recursive which uses manual construction
        use crate::backend::compile::compile;

        let input = r#"
            (= (fact $n) (if (> $n 0) (* $n (fact (- $n 1))) 1))
            !(fact 0)
            !(fact 1)
            !(fact 2)
            !(fact 3)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut results = Vec::new();

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            // Collect non-empty results (skip rule definitions)
            if !expr_results.is_empty() {
                results.extend(expr_results);
            }
        }

        // Should have 4 results: fact(0)=1, fact(1)=1, fact(2)=2, fact(3)=6
        assert_eq!(results.len(), 4);
        assert_eq!(results[0], MettaValue::Long(1)); // fact(0)
        assert_eq!(results[1], MettaValue::Long(1)); // fact(1)
        assert_eq!(results[2], MettaValue::Long(2)); // fact(2)
        assert_eq!(results[3], MettaValue::Long(6)); // fact(3)
    }

    #[test]
    fn test_incremental_nested_arithmetic() {
        // From test_metta.py: !(+ 1 (+ 2 (+ 3 4)))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(3),
                    MettaValue::Long(4),
                ]),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    #[test]
    fn test_function_definition_and_call() {
        // From test_run_metta.py: (= (f) (+ 2 3)) !(f)
        let mut env = Environment::new();

        // Define rule: (= (f) (+ 2 3))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        };
        env.add_rule(rule);

        // Evaluate (f)
        let value = MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(5));
    }

    #[test]
    fn test_multiple_pattern_variables() {
        // Test pattern matching with multiple variables
        let mut env = Environment::new();

        // (= (add3 $a $b $c) (+ $a (+ $b $c)))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("add3".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
                MettaValue::Atom("$c".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$b".to_string()),
                    MettaValue::Atom("$c".to_string()),
                ]),
            ]),
        };
        env.add_rule(rule);

        // (add3 10 20 30) = 60
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add3".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(20),
            MettaValue::Long(30),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(60));
    }

    #[test]
    fn test_nested_pattern_matching() {
        // Test nested S-expression pattern matching
        let mut env = Environment::new();

        // (= (eval-pair (pair $x $y)) (+ $x $y))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("eval-pair".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("pair".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        };
        env.add_rule(rule);

        // (eval-pair (pair 5 7)) = 12
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("eval-pair".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pair".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(7),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(12));
    }

    #[test]
    fn test_wildcard_pattern() {
        // Test wildcard matching
        let pattern = MettaValue::Atom("_".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());

        // Wildcard should not bind the value
        let bindings = bindings.unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_variable_consistency_in_pattern() {
        // Test that the same variable in a pattern must match the same value
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        // Should match when both are the same
        let value1 = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(5),
        ]);
        assert!(pattern_match(&pattern, &value1).is_some());

        // Should not match when they differ
        let value2 = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(7),
        ]);
        assert!(pattern_match(&pattern, &value2).is_none());
    }

    #[test]
    fn test_conditional_with_pattern_matching() {
        // Test combining if with pattern matching
        let mut env = Environment::new();

        // (= (abs $x) (if (< $x 0) (- 0 $x) $x))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("abs".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("<".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("-".to_string()),
                    MettaValue::Long(0),
                    MettaValue::Atom("$x".to_string()),
                ]),
                MettaValue::Atom("$x".to_string()),
            ]),
        };
        env.add_rule(rule);

        // abs(-5) = 5
        let value1 = MettaValue::SExpr(vec![
            MettaValue::Atom("abs".to_string()),
            MettaValue::Long(-5),
        ]);
        let (results, env1) = eval(value1, env.clone());
        assert_eq!(results[0], MettaValue::Long(5));

        // abs(7) = 7
        let value2 = MettaValue::SExpr(vec![
            MettaValue::Atom("abs".to_string()),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(value2, env1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_string_values() {
        // Test string value handling
        let env = Environment::new();
        let value = MettaValue::String("test".to_string());
        let (results, _) = eval(value.clone(), env);
        assert_eq!(results[0], value);
    }

    #[test]
    fn test_boolean_values() {
        let env = Environment::new();

        let value_true = MettaValue::Bool(true);
        let (results, _) = eval(value_true.clone(), env.clone());
        assert_eq!(results[0], value_true);

        let value_false = MettaValue::Bool(false);
        let (results, _) = eval(value_false.clone(), env);
        assert_eq!(results[0], value_false);
    }

    #[test]
    fn test_nil_value() {
        let env = Environment::new();
        let value = MettaValue::Nil;
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Nil);
    }

    // === Fact Database Tests ===

    #[test]
    fn test_symbol_added_to_fact_database() {
        // Bare atoms should NOT be added to the fact database
        // Only rules, type assertions, and unmatched s-expressions are stored
        let env = Environment::new();

        // Evaluate the symbol "Hello"
        let symbol = MettaValue::Atom("Hello".to_string());
        let (results, new_env) = eval(symbol.clone(), env);

        // Symbol should be returned unchanged
        assert_eq!(results[0], symbol);

        // Bare atoms should NOT be added to fact database (this prevents pollution)
        assert!(!new_env.has_fact("Hello"));
    }

    #[test]
    fn test_variables_not_added_to_fact_database() {
        let env = Environment::new();

        // Test $variable
        let var1 = MettaValue::Atom("$x".to_string());
        let (_, new_env) = eval(var1, env.clone());
        assert!(!new_env.has_fact("$x"));

        // Test &variable
        let var2 = MettaValue::Atom("&y".to_string());
        let (_, new_env) = eval(var2, env.clone());
        assert!(!new_env.has_fact("&y"));

        // Test 'variable
        let var3 = MettaValue::Atom("'z".to_string());
        let (_, new_env) = eval(var3, env.clone());
        assert!(!new_env.has_fact("'z"));

        // Test wildcard
        let wildcard = MettaValue::Atom("_".to_string());
        let (_, new_env) = eval(wildcard, env);
        assert!(!new_env.has_fact("_"));
    }

    #[test]
    fn test_multiple_symbols_in_fact_database() {
        // Bare atoms should NOT be added to fact database
        // This test verifies that evaluating multiple atoms doesn't pollute the environment
        let env = Environment::new();

        // Evaluate multiple symbols
        let symbol1 = MettaValue::Atom("Foo".to_string());
        let (_, env1) = eval(symbol1, env);

        let symbol2 = MettaValue::Atom("Bar".to_string());
        let (_, env2) = eval(symbol2, env1);

        let symbol3 = MettaValue::Atom("Baz".to_string());
        let (_, env3) = eval(symbol3, env2);

        // Bare atoms should NOT be in the fact database
        assert!(!env3.has_fact("Foo"));
        assert!(!env3.has_fact("Bar"));
        assert!(!env3.has_fact("Baz"));
    }

    #[test]
    fn test_sexpr_added_to_fact_database() {
        // When an s-expression like (Hello World) is evaluated, it should be added to the fact database
        let env = Environment::new();

        // Evaluate the s-expression (Hello World)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("Hello".to_string()),
            MettaValue::Atom("World".to_string()),
        ]);
        let expected_result = MettaValue::SExpr(vec![
            MettaValue::Atom("Hello".to_string()),
            MettaValue::Atom("World".to_string()),
        ]);

        let (results, new_env) = eval(sexpr.clone(), env);

        // S-expression should be returned (with evaluated elements)
        assert_eq!(results[0], expected_result);

        // S-expression should be added to fact database
        assert!(new_env.has_sexpr_fact(&expected_result));

        // Individual atoms should also be in the fact database
        assert!(new_env.has_fact("Hello"));
        assert!(new_env.has_fact("World"));
    }

    #[test]
    fn test_nested_sexpr_in_fact_database() {
        let env = Environment::new();

        // Evaluate a nested s-expression
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Inner".to_string()),
                MettaValue::Atom("Nested".to_string()),
            ]),
        ]);

        let (_, new_env) = eval(sexpr, env);

        // Outer s-expression should be in fact database
        let expected_outer = MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Inner".to_string()),
                MettaValue::Atom("Nested".to_string()),
            ]),
        ]);
        assert!(new_env.has_sexpr_fact(&expected_outer));

        // Inner s-expression should also be in fact database
        let expected_inner = MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]);
        assert!(new_env.has_sexpr_fact(&expected_inner));

        // All atoms should be in the atom fact database
        assert!(new_env.has_fact("Outer"));
        assert!(new_env.has_fact("Inner"));
        assert!(new_env.has_fact("Nested"));
    }

    #[test]
    fn test_grounded_operations_not_added_to_sexpr_facts() {
        let env = Environment::new();

        // Evaluate an arithmetic operation (add 1 2)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let (results, new_env) = eval(sexpr.clone(), env);

        // Result should be 3
        assert_eq!(results[0], MettaValue::Long(3));

        // The s-expression should NOT be in the fact database
        // because it was reduced to a value by a grounded operation
        assert!(!new_env.has_sexpr_fact(&sexpr));
    }

    #[test]
    fn test_rule_definition_added_to_fact_database() {
        let env = Environment::new();

        // Define a rule: (= (double $x) (* $x 2))
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (result, new_env) = eval(rule_def.clone(), env);

        // Rule definition should return empty list
        assert!(result.is_empty());

        // Rule definition should also be in the fact database
        assert!(new_env.has_sexpr_fact(&rule_def));
    }

    // === Type Error Tests ===

    #[test]
    fn test_arithmetic_type_error_string() {
        let env = Environment::new();

        // Test: !(+ 1 "a") should produce BadType error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::String("a".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                // Error message should contain the invalid value
                assert!(msg.contains("String"));
                // Error details should be BadType
                assert_eq!(**details, MettaValue::Atom("BadType".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_arithmetic_type_error_first_arg() {
        let env = Environment::new();

        // Test: !(+ "a" 1) - first argument wrong type
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::String("a".to_string()),
            MettaValue::Long(1),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("String"));
                assert_eq!(**details, MettaValue::Atom("BadType".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_arithmetic_type_error_bool() {
        let env = Environment::new();

        // Test: !(* true false) - booleans not valid for arithmetic
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("Bool"));
                assert_eq!(**details, MettaValue::Atom("BadType".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_comparison_type_error() {
        let env = Environment::new();

        // Test: !(< "a" "b") - strings not valid for comparison
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::String("a".to_string()),
            MettaValue::String("b".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("String"));
                assert_eq!(**details, MettaValue::Atom("BadType".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_arithmetic_wrong_arity() {
        let env = Environment::new();

        // Test: !(+ 1) - wrong number of arguments
        let value = MettaValue::SExpr(vec![MettaValue::Atom("+".to_string()), MettaValue::Long(1)]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("2 arguments"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_type_error_propagation() {
        let env = Environment::new();

        // Test: !(+ 1 (+ 2 "bad")) - error should propagate from inner expression
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::String("bad".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // The error from the inner expression should propagate
        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("String"));
                assert_eq!(**details, MettaValue::Atom("BadType".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_type_assertion_added_to_fact_database() {
        let env = Environment::new();

        // Define a type assertion: (: x Number)
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Atom("Number".to_string()),
        ]);

        let (result, new_env) = eval(type_assertion.clone(), env);

        // Type assertion should return empty list
        assert!(result.is_empty());

        // Type should be in the type database
        assert_eq!(
            new_env.get_type("x"),
            Some(MettaValue::Atom("Number".to_string()))
        );

        // Type assertion should also be in the fact database
        assert!(new_env.has_sexpr_fact(&type_assertion));
    }

    // === Let Binding Tests ===

    #[test]
    fn test_let_simple_binding() {
        let env = Environment::new();

        // (let $x 42 $x)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_let_with_expression() {
        let env = Environment::new();

        // (let $y (+ 10 5) (* $y 2))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$y".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(10),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$y".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30));
    }

    #[test]
    fn test_let_with_pattern_matching() {
        let env = Environment::new();

        // (let (tuple $a $b) (tuple 1 2) (+ $a $b))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_let_nested() {
        let env = Environment::new();

        // (let $z 3 (let $w 4 (+ $z $w)))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$z".to_string()),
            MettaValue::Long(3),
            MettaValue::SExpr(vec![
                MettaValue::Atom("let".to_string()),
                MettaValue::Atom("$w".to_string()),
                MettaValue::Long(4),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$z".to_string()),
                    MettaValue::Atom("$w".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_let_with_if() {
        let env = Environment::new();

        // (let $base 10 (if (> $base 5) (* $base 2) $base))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$base".to_string()),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(5),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(2),
                ]),
                MettaValue::Atom("$base".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(20));
    }

    #[test]
    fn test_let_pattern_mismatch() {
        let env = Environment::new();

        // (let (foo $x) (bar 42) $x) - pattern mismatch should error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("bar".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("does not match"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}
