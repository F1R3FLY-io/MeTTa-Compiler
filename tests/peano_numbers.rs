//! Tests for Peano number support in MORK
//!
//! Tests that Peano numbers (Z, S Z, S (S Z), ...) work correctly
//! in pattern matching and evaluation for PR #2 (feature/mork-peano-numbers).

use mettatron::{compile, eval, new_env};

#[test]
fn test_peano_zero_literal() {
    let env = new_env();

    // Parse Peano zero
    let source = "Z";
    let state = compile(source).expect("compile failed");

    // Z should be an atom
    assert_eq!(state.source().len(), 1);

    // Evaluate Z (should return itself)
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = state.source()[0];
    let (results, _, ..) = eval(expr, env, &state);
    assert_eq!(results.len(), 1);
}

#[test]
fn test_peano_successor_literals() {
    let env = new_env();

    // Parse Peano successors
    let source1 = "(S Z)";
    let state1 = compile(source1).expect("compile failed");
    assert_eq!(state1.source().len(), 1);

    let source2 = "(S (S Z))";
    let state2 = compile(source2).expect("compile failed");
    assert_eq!(state2.source().len(), 1);

    let source3 = "(S (S (S Z)))";
    let state3 = compile(source3).expect("compile failed");
    assert_eq!(state3.source().len(), 1);

    // Evaluate (should return themselves as they're ground)
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr1 = state1.source()[0];
    let (results1, _, ..) = eval(expr1, env.clone(), &state1);
    assert_eq!(results1.len(), 1);

    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr2 = state2.source()[0];
    let (results2, _, ..) = eval(expr2, env.clone(), &state2);
    assert_eq!(results2.len(), 1);

    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr3 = state3.source()[0];
    let (results3, _, ..) = eval(expr3, env.clone(), &state3);
    assert_eq!(results3.len(), 1);
}

#[test]
fn test_peano_pattern_matching_zero() {
    let mut env = new_env();

    // Add a fact with Peano zero
    let fact_source = "(number Z)";
    let fact_state = compile(fact_source).expect("compile failed");
    env.add_to_space(&fact_state.source()[0]);

    // Query for Z
    let query_source = "(match &self (number Z) (number Z))";
    let query_state = compile(query_source).expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query_state.source()[0];
    let (results, _, ..) = eval(expr, env, &query_state);

    assert_eq!(results.len(), 1, "Should match Peano zero");
}

#[test]
fn test_peano_pattern_matching_successor() {
    let mut env = new_env();

    // Add facts with Peano successors
    let fact1 = compile("(number (S Z))").expect("compile failed");
    env.add_to_space(&fact1.source()[0]);

    let fact2 = compile("(number (S (S Z)))").expect("compile failed");
    env.add_to_space(&fact2.source()[0]);

    // Query for exact match
    let query1 = compile("(match &self (number (S Z)) (number (S Z)))").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr1 = query1.source()[0];
    let (results1, _, ..) = eval(expr1, env.clone(), &query1);
    assert_eq!(results1.len(), 1, "Should match (S Z)");

    // Query for pattern with variable
    let query2 = compile("(match &self (number (S $x)) (S $x))").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr2 = query2.source()[0];
    let (results2, _, ..) = eval(expr2, env, &query2);
    assert_eq!(
        results2.len(),
        2,
        "Should match both successors with variable"
    );
}

#[test]
fn test_peano_nested_pattern_matching() {
    let mut env = new_env();

    // Add fact with nested Peano
    let fact = compile("(generation (S (S Z)) Alice Bob)").expect("compile failed");
    env.add_to_space(&fact.source()[0]);

    // Query with exact match
    let query1 =
        compile("(match &self (generation (S (S Z)) Alice Bob) (generation (S (S Z)) Alice Bob))")
            .expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr1 = query1.source()[0];
    let (results1, _, ..) = eval(expr1, env.clone(), &query1);
    assert_eq!(results1.len(), 1, "Should match exact Peano structure");

    // Query with variable in Peano
    let query2 = compile("(match &self (generation $n Alice Bob) $n)").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr2 = query2.source()[0];
    let (results2, _, ..) = eval(expr2, env, &query2);
    assert_eq!(
        results2.len(),
        1,
        "Should match and bind Peano number to variable"
    );
}

#[test]
fn test_peano_in_rules() {
    let env = new_env();

    // Define a rule using Peano numbers
    let rule_source = "(= (next Z) (S Z))";
    let rule_state = compile(rule_source).expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let rule_expr = rule_state.source()[0];
    let (_, env, ..) = eval(rule_expr, env, &rule_state);

    // Query the rule
    let query = compile("!(next Z)").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let query_expr = query.source()[0];
    let (results, _, ..) = eval(query_expr, env, &query);

    // Should get (S Z) as result
    assert_eq!(results.len(), 1, "Rule should produce successor");
}

#[test]
fn test_peano_pattern_destructuring() {
    let mut env = new_env();

    // Add facts with Peano numbers
    let s1 = compile("(num (S Z))").expect("compile failed");
    env.add_to_space(&s1.source()[0]);
    let s2 = compile("(num (S (S Z)))").expect("compile failed");
    env.add_to_space(&s2.source()[0]);
    let s3 = compile("(num (S (S (S Z))))").expect("compile failed");
    env.add_to_space(&s3.source()[0]);

    // Match pattern (S $x) to get the predecessor
    let query = compile("(match &self (num (S $x)) $x)").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query.source()[0];
    let (results, _, ..) = eval(expr, env, &query);

    // Should match all three and bind Z, (S Z), (S (S Z))
    assert_eq!(
        results.len(),
        3,
        "Should destructure all three Peano numbers"
    );
}

#[test]
fn test_peano_in_space_operations() {
    let mut env = new_env();

    // Add Peano number via direct API
    let peano_fact = compile("(count (S (S (S Z))))").expect("compile failed");
    env.add_to_space(&peano_fact.source()[0]);

    // Verify it exists
    let query = compile("(match &self (count (S (S (S Z)))) (count (S (S (S Z)))))")
        .expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query.source()[0];
    let (results, _, ..) = eval(expr, env.clone(), &query);
    assert_eq!(results.len(), 1, "Peano fact should be in space");

    // Remove it
    env.remove_from_space(&peano_fact.source()[0]);

    // Verify it's gone
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query.source()[0];
    let (results_after, _, ..) = eval(expr, env, &query);
    assert_eq!(results_after.len(), 0, "Peano fact should be removed");
}
