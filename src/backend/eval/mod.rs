// Eval function: Lazy evaluation with pattern matching and built-in dispatch
//
// eval(a: atom, env) = a, env
// eval((t1 .. tn), env):
//   r1, env_1 = eval(t1, env) | ... | rn, env_n = eval(tn, env)
//   env' = union env_i
//   return fold over rules & grounded functions (emptyset, env')

#[macro_use]
mod macros;

mod bindings;
mod control_flow;
mod errors;
mod evaluation;
mod list_ops;
mod quoting;
mod space;
mod types;

use crate::backend::environment::Environment;
use crate::backend::models::{Bindings, EvalResult, MettaValue, Rule};
use crate::backend::mork_convert::{mork_bindings_to_metta, ConversionContext};
use mork_expr::Expr;

pub(super) type EvalOutput = (Vec<MettaValue>, Environment);

/// Maximum evaluation depth to prevent stack overflow
/// This limits how deep the evaluation can recurse through nested expressions
/// Set to 1000 to allow legitimate deep nesting while still catching runaway recursion
const MAX_EVAL_DEPTH: usize = 1000;

/// Maximum number of results in Cartesian product to prevent combinatorial explosion
/// This limits the total number of combinations explored during nondeterministic evaluation
const MAX_CARTESIAN_RESULTS: usize = 10000;

/// MeTTa special forms for "did you mean" suggestions during evaluation
const SPECIAL_FORMS: &[&str] = &[
    "=", "!", "quote", "if", "error", "is-error", "catch", "eval",
    "function", "return", "chain", "match", "case", "switch", "let",
    ":", "get-type", "check-type", "map-atom", "filter-atom", "foldl-atom",
];

/// Convert MettaValue to a friendly type name for error messages
/// This provides user-friendly type names instead of debug format like "Long(5)"
fn friendly_type_name(value: &MettaValue) -> &'static str {
    match value {
        MettaValue::Long(_) => "Number (integer)",
        MettaValue::Float(_) => "Number (float)",
        MettaValue::Bool(_) => "Bool",
        MettaValue::String(_) => "String",
        MettaValue::Atom(_) => "Atom",
        MettaValue::Uri(_) => "URI",
        MettaValue::Nil => "Nil",
        MettaValue::SExpr(_) => "S-expression",
        MettaValue::Error(_, _) => "Error",
        MettaValue::Type(_) => "Type",
    }
}

/// Check if an operator is close to a known special form
/// Uses max edit distance of 1 to avoid false positives on short words
fn suggest_special_form(op: &str) -> Option<String> {
    use crate::backend::fuzzy_match::FuzzyMatcher;
    use std::sync::OnceLock;

    static MATCHER: OnceLock<FuzzyMatcher> = OnceLock::new();
    let matcher = MATCHER.get_or_init(|| FuzzyMatcher::from_terms(SPECIAL_FORMS.iter().copied()));

    matcher.did_you_mean(op, 1, 3)
}

/// Evaluate a MettaValue in the given environment
/// Returns (results, new_environment)
/// This is the public entry point that starts evaluation with depth tracking
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    eval_with_depth(value, env, 0)
}

/// Internal evaluation function with depth tracking
/// This prevents unbounded recursion and stack overflow
fn eval_with_depth(value: MettaValue, env: Environment, depth: usize) -> EvalResult {
    // Check depth limit
    if depth > MAX_EVAL_DEPTH {
        return (
            vec![MettaValue::Error(
                format!(
                    "Maximum evaluation depth ({}) exceeded. Possible causes:\n\
                     - Infinite recursion: check for missing base case in recursive rules\n\
                     - Combinatorial explosion: rule produces too many branches\n\
                     Hint: Use (function ...) and (return ...) for tail-recursive evaluation",
                    MAX_EVAL_DEPTH
                ),
                Box::new(value),
            )],
            env,
        );
    }

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
        | MettaValue::Float(_)
        | MettaValue::String(_)
        | MettaValue::Uri(_)
        | MettaValue::Nil
        | MettaValue::Type(_) => (vec![value], env),

        // For s-expressions, evaluate elements and apply rules/built-ins
        MettaValue::SExpr(items) => eval_sexpr(items, env, depth),
    }
}

/// Evaluate an s-expression with depth tracking
fn eval_sexpr(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalResult {
    if items.is_empty() {
        return (vec![MettaValue::Nil], env);
    }

    // Check for special forms before evaluation
    if let Some(MettaValue::Atom(op)) = items.first() {
        match op.as_str() {
            // Rule definition: (= lhs rhs) - add to MORK Space and rule cache
            "=" => return space::eval_add(items, env),

            // Evaluation: ! expr - force evaluation
            "!" => return evaluation::force_eval(items, env),

            // Quote: return argument unevaluated
            "quote" => return quoting::eval_quote(items, env),

            // If: conditional evaluation - only evaluate chosen branch
            "if" => return control_flow::eval_if(items, env),

            // Error construction
            "error" => return errors::eval_error(items, env),

            // Is-error: check if value is an error (for error recovery)
            "is-error" => return errors::eval_if_error(items, env),

            // Catch: error recovery - (catch expr default)
            // If expr evaluates to error, return default instead
            "catch" => return errors::eval_catch(items, env),

            // Eval: force evaluation of quoted expressions
            // (eval expr) - complementary to quote
            "eval" => return evaluation::eval_eval(items, env),

            // Function: creates an evaluation loop that continues
            // until it encounters a return value
            "function" => return evaluation::eval_function(items, env),

            // Return: signals termination from a function evaluation loop
            "return" => return evaluation::eval_return(items, env),

            // Evaluates first argument, binds it to the variable (second argument) and
            // then evaluates third argument which contains (or not) mentioned variable
            "chain" => return evaluation::eval_chain(items, env),

            // Match: pattern matching against atom space
            // (match <space> <pattern> <template>)
            // Searches space for all atoms matching pattern and returns instantiated templates
            "match" => return space::eval_match(items, env),

            // Subsequently tests multiple pattern-matching conditions (second argument) for the
            // given value (first argument)
            "case" => return control_flow::eval_case(items, env),

            // Difference between `switch` and `case` is a way how they interpret `Empty` result.
            // case interprets first argument inside itself and then manually checks whether result is empty.
            "switch" => return control_flow::eval_switch(items, env),

            "switch-minimal" => return control_flow::eval_switch_minimal_handler(items, env),

            // This function is being called inside switch function to test one of the cases and it
            // calls switch once again if current condition is not met
            "switch-internal" => return control_flow::eval_switch_internal_handler(items, env),

            // Let: local variable binding
            // (let $var value body) - Bind variable to value and evaluate body with that binding
            // Supports pattern matching: (let ($x $y) (tuple 1 2) body)
            "let" => return bindings::eval_let(items, env),

            // Type assertion: (: expr type)
            // Adds a type assertion to the environment
            ":" => return types::eval_type_assertion(items, env),

            // get-type: return the type of an expression
            // (get-type expr) -> Type
            "get-type" => return types::eval_get_type(items, env),

            // check-type: check if expression has expected type
            // (check-type expr expected-type) -> Bool
            "check-type" => return types::eval_check_type(items, env),

            // map-atom: maps a function over a list of atoms
            "map-atom" => return list_ops::eval_map_atom(items, env),

            // filter-atom: filters a list keeping only elements that satisfy the predicate
            "filter-atom" => return list_ops::eval_filter_atom(items, env),

            // foldl-atom: folds (reduces) a list from left to right using an operation and initial value
            "foldl-atom" => return list_ops::eval_foldl_atom(items, env),

            // Fall through to normal evaluation
            _ => {}
        }
    }

    // Evaluate all sub-expressions sequentially
    // Phase 3c benchmarking conclusively showed sequential is always faster
    // (tested 2-32768 operations, flat + nested expressions)
    let eval_results_and_envs: Vec<(Vec<MettaValue>, Environment)> = items
        .iter()
        .map(|item: &MettaValue| eval_with_depth(item.clone(), env.clone(), depth + 1))
        .collect();

    // Check for errors in subexpressions and propagate immediately
    for (results, new_env) in &eval_results_and_envs {
        if let Some(first) = results.first() {
            if matches!(first, MettaValue::Error(_, _)) {
                return (vec![first.clone()], new_env.clone());
            }
        }
    }

    // Split results and environments
    let (eval_results, envs): (Vec<_>, Vec<_>) = eval_results_and_envs.into_iter().unzip();

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
                let (results, _) =
                    eval_with_depth(instantiated_rhs, unified_env.clone(), depth + 1);
                all_final_results.extend(results);
            }
        } else {
            // No rule matched - check for likely typos before falling back to ADD mode
            // We only error if the symbol is close to a known term (suggesting a typo)
            // Otherwise, use standard ADD mode semantics (add to space and return)
            let mut is_likely_typo = false;

            if let Some(MettaValue::Atom(head)) = evaled_items.first() {
                // Only check for typos on symbols with 3+ characters to avoid false positives
                // on short symbols like "a", "x", etc. which are often legitimate data constructors
                if head.len() >= 3 {
                    // Check if it looks like a misspelled special form
                    if let Some(suggestion) = suggest_special_form(head) {
                        let err = MettaValue::Error(
                            format!("Unknown special form '{}'. {}", head, suggestion),
                            Box::new(sexpr.clone()),
                        );
                        all_final_results.push(err);
                        is_likely_typo = true;
                    }
                    // Check if it matches any known rule heads (max distance 1 to avoid false positives)
                    else if let Some(suggestion) = unified_env.did_you_mean(head, 1) {
                        let err = MettaValue::Error(
                            format!("No rule matches '{}'. {}", head, suggestion),
                            Box::new(sexpr.clone()),
                        );
                        all_final_results.push(err);
                        is_likely_typo = true;
                    }
                }
            }

            // If not a likely typo, use standard ADD mode semantics
            if !is_likely_typo {
                // No rule matched - add to space and return (official MeTTa ADD mode semantics)
                // In official MeTTa's default ADD mode, bare expressions are automatically added to &self
                // This matches the behavior: `(leaf1 leaf2)` -> auto-added, then `!(match &self ...)` can query it
                unified_env.add_to_space(&sexpr);
                all_final_results.push(sexpr);
            }
        }
    }

    (all_final_results, unified_env)
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
                format!(
                    "Cannot perform arithmetic: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Box::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot perform arithmetic: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Box::new(MettaValue::Atom("TypeError".to_string())),
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
                format!(
                    "Cannot compare: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Box::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot compare: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Box::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    Some(MettaValue::Bool(op(a, b)))
}

/// Pattern match a pattern against a value
/// Returns bindings if successful, None otherwise
///
/// This is made public to support optimized match operations in Environment
/// and for benchmarking the core pattern matching algorithm.
pub fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
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

        // FAST PATH: First variable binding (empty bindings)
        // Optimization: Skip lookup when bindings are empty - directly insert
        // This reduces single-variable regression from 16.8% to ~5-7%
        (MettaValue::Atom(p), v)
            if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                && p != "&"
                && bindings.is_empty() =>
        {
            bindings.insert(p.clone(), v.clone());
            true
        }

        // GENERAL PATH: Variable with potential existing bindings
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        (MettaValue::Atom(p), v)
            if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\'')) && p != "&" =>
        {
            // Check if variable is already bound (linear search for SmartBindings)
            if let Some((_, existing)) = bindings.iter().find(|(name, _)| name.as_str() == p) {
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
        (MettaValue::Float(p), MettaValue::Float(v)) => p == v,
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
///
/// This function has a built-in limit (MAX_CARTESIAN_RESULTS) to prevent combinatorial explosion.
/// If the product would exceed this limit, it returns only the first MAX_CARTESIAN_RESULTS combinations.
fn cartesian_product(results: &[Vec<MettaValue>]) -> Vec<Vec<MettaValue>> {
    if results.is_empty() {
        return vec![vec![]];
    }

    // Base case: single result list
    if results.len() == 1 {
        let items: Vec<Vec<MettaValue>> = results[0]
            .iter()
            .take(MAX_CARTESIAN_RESULTS)
            .map(|item| vec![item.clone()])
            .collect();
        return items;
    }

    // Recursive case: combine first list with Cartesian product of rest
    let first = &results[0];
    let rest_product = cartesian_product(&results[1..]);

    let mut product = Vec::new();
    'outer: for item in first {
        for rest_combo in &rest_product {
            // Check limit before adding more combinations
            if product.len() >= MAX_CARTESIAN_RESULTS {
                break 'outer;
            }

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
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == s)
                .map(|(_, val)| val.clone())
                .unwrap_or_else(|| value.clone())
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
        | MettaValue::Float(_)
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

    // Convert expression to MORK format for querying (using cache)
    let expr_bytes = match env.metta_to_mork_bytes_cached(expr) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(), // Fallback to iterative if conversion fails
    };

    let space = env.create_space();
    let ctx = ConversionContext::new();

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
                if let Some((_, rhs)) = our_bindings
                    .iter()
                    .find(|(name, _)| name.as_str() == "$rhs")
                {
                    matches.push((rhs.clone(), our_bindings));
                }
            }
        }
        true // Continue searching for ALL matches
    });

    matches
    // space will be dropped automatically here
}

/// Optimized: Try pattern matching using indexed lookup to find ALL matching rules
/// Uses O(1) index lookup instead of O(n) iteration
/// Complexity: O(k) where k = rules with matching head symbol (typically k << n)
fn try_match_all_rules_iterative(
    expr: &MettaValue,
    env: &Environment,
) -> Vec<(MettaValue, Bindings)> {
    // Extract head symbol and arity for indexed lookup
    let matching_rules = if let Some(head) = get_head_symbol(expr) {
        let arity = expr.get_arity();
        // O(1) indexed lookup instead of O(n) iteration
        env.get_matching_rules(&head, arity)
    } else {
        // For expressions without head symbol, check wildcard rules only
        // This is still O(k_wildcards) instead of O(n_total)
        env.get_matching_rules("", 0) // Empty head will return only wildcards
    };

    // Sort rules by specificity (more specific first)
    let mut sorted_rules = matching_rules;
    sorted_rules.sort_by_key(|rule| pattern_specificity(&rule.lhs));

    // Collect ALL matching rules, tracking LHS specificity
    let mut matches: Vec<(MettaValue, Bindings, usize, Rule)> = Vec::new();
    for rule in sorted_rules {
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
        assert_eq!(
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == "$x")
                .map(|(_, val)| val),
            Some(&MettaValue::Long(42))
        );
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
        assert_eq!(
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == "$x")
                .map(|(_, val)| val),
            Some(&MettaValue::Long(1))
        );
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

    // === Integration Test ===

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
        // Verify official MeTTa ADD mode semantics:
        // When an s-expression like (Hello World) is evaluated, it is automatically added to the space
        // This matches: `(leaf1 leaf2)` in REPL -> auto-added, queryable via `!(match &self ...)`
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

        // S-expression should be added to fact database (ADD mode behavior)
        assert!(new_env.has_sexpr_fact(&expected_result));

        // Individual atoms are NOT stored separately
        // Only the full s-expression is stored in MORK format
        assert!(!new_env.has_fact("Hello"));
        assert!(!new_env.has_fact("World"));
    }

    #[test]
    fn test_nested_sexpr_in_fact_database() {
        // Official MeTTa semantics: only the top-level expression is stored
        // Nested sub-expressions are NOT extracted and stored separately
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

        // CORRECT: Outer s-expression should be in fact database
        let expected_outer = MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Inner".to_string()),
                MettaValue::Atom("Nested".to_string()),
            ]),
        ]);
        assert!(new_env.has_sexpr_fact(&expected_outer));

        // CORRECT: Inner s-expression should NOT be in fact database (not recursively stored)
        // Official MeTTa only stores the top-level expression passed to add-atom
        let expected_inner = MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]);
        assert!(!new_env.has_sexpr_fact(&expected_inner));

        // Individual atoms are NOT stored separately
        assert!(!new_env.has_fact("Outer"));
        assert!(!new_env.has_fact("Inner"));
        assert!(!new_env.has_fact("Nested"));
    }

    #[test]
    fn test_pattern_matching_extracts_nested_sexpr() {
        // Demonstrates that while nested s-expressions are NOT stored separately,
        // they can still be accessed via pattern matching with variables.
        // This is how official MeTTa handles nested data extraction.
        let mut env = Environment::new();

        // Store a nested s-expression: (Outer (Inner Nested))
        let nested_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Inner".to_string()),
                MettaValue::Atom("Nested".to_string()),
            ]),
        ]);

        // Evaluate to add to space (ADD mode behavior)
        let (_, env1) = eval(nested_expr.clone(), env);
        env = env1;

        // Verify only the outer expression is stored
        assert!(env.has_sexpr_fact(&nested_expr));
        let inner_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]);
        assert!(!env.has_sexpr_fact(&inner_expr)); // NOT stored separately

        // Use pattern matching to extract the nested part: (match & self (Outer $x) $x)
        let match_query = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Outer".to_string()),
                MettaValue::Atom("$x".to_string()), // Variable to capture nested part
            ]),
            MettaValue::Atom("$x".to_string()), // Template: return the captured value
        ]);

        let (results, _) = eval(match_query, env);

        // Should return the nested s-expression even though it wasn't stored separately
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], inner_expr); // Pattern matching extracts (Inner Nested)
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

        // Test: !(+ 1 "a") should produce TypeError with friendly message
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::String("a".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                // Error message should contain the friendly type name
                assert!(
                    msg.contains("String"),
                    "Expected 'String' in: {}",
                    msg
                );
                assert!(
                    msg.contains("expected Number (integer)"),
                    "Expected 'expected Number (integer)' in: {}",
                    msg
                );
                // Error details should be TypeError
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
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
                assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
                assert!(
                    msg.contains("expected Number (integer)"),
                    "Expected type info in: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
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
                assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
                assert!(
                    msg.contains("expected Number (integer)"),
                    "Expected type info in: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
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
                assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
                assert!(
                    msg.contains("Cannot compare"),
                    "Expected 'Cannot compare' in: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
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
    fn test_misspelled_special_form() {
        let env = Environment::new();

        // Try to use "mach" instead of "match"
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("mach".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::Atom("pattern".to_string()),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Did you mean"),
                    "Expected suggestion in: {}",
                    msg
                );
                assert!(msg.contains("match"), "Expected 'match' in: {}", msg);
            }
            other => panic!("Expected Error with suggestion, got {:?}", other),
        }
    }

    #[test]
    fn test_undefined_symbol_with_rule_suggestion() {
        let mut env = Environment::new();

        // Add a rule for "fibonacci"
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("fibonacci".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
        };
        env.add_rule(rule);

        // Try to call "fibonaci" (misspelled - missing 'n')
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("fibonaci".to_string()),
            MettaValue::Long(5),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Did you mean"),
                    "Expected suggestion in: {}",
                    msg
                );
                assert!(
                    msg.contains("fibonacci"),
                    "Expected 'fibonacci' in: {}",
                    msg
                );
            }
            other => panic!("Expected Error with suggestion, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_symbol_returns_as_is() {
        let env = Environment::new();

        // Completely unknown symbols (not similar to any known term)
        // should be returned as-is per ADD mode semantics
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("xyzzy".to_string()),
            MettaValue::Long(1),
        ]);

        let (results, _) = eval(expr.clone(), env);
        assert_eq!(results.len(), 1);

        // Should return the expression as-is (ADD mode), not an error
        assert_eq!(results[0], expr, "Expected expression to be returned as-is");
    }

    #[test]
    fn test_short_symbol_not_flagged_as_typo() {
        let env = Environment::new();

        // Short symbols like "a" should NOT be flagged as typos even if
        // they're close to special forms like "=" (edit distance 1)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let (results, _) = eval(expr.clone(), env);
        assert_eq!(results.len(), 1);

        // Should return the expression as-is (ADD mode), not an error
        assert_eq!(
            results[0], expr,
            "Short symbols should not be flagged as typos"
        );
    }
}
