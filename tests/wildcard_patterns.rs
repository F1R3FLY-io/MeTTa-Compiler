//! Tests for wildcard pattern matching
//!
//! Tests that wildcard patterns ($_ and _) work correctly in all contexts

use mettatron::{compile, eval, new_env};

#[test]
fn test_underscore_wildcard() {
    let mut env = new_env();

    // Add facts (each compile creates a MettaState that must stay alive)
    let fact1 = compile("(data 1 foo)").expect("compile failed");
    let fact2 = compile("(data 2 bar)").expect("compile failed");
    let fact3 = compile("(data 3 baz)").expect("compile failed");
    env.add_to_space(&fact1.source()[0]);
    env.add_to_space(&fact2.source()[0]);
    env.add_to_space(&fact3.source()[0]);

    // Match with underscore wildcard (should match anything, but not bind)
    let query_state = compile("(match &self (data _ $value) $value)").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query_state.source()[0];
    let (results, _) = eval(expr, env, &query_state);

    assert_eq!(results.len(), 3, "Should match all three facts");
}

#[test]
fn test_dollar_underscore_wildcard() {
    let mut env = new_env();

    // Add generation facts like in ancestor.mm2 line 38
    let fact1 = compile("(generation Z Alice Bob)").expect("compile failed");
    let fact2 = compile("(generation (S Z) Bob Carol)").expect("compile failed");
    let fact3 = compile("(generation (S (S Z)) Carol Dave)").expect("compile failed");
    env.add_to_space(&fact1.source()[0]);
    env.add_to_space(&fact2.source()[0]);
    env.add_to_space(&fact3.source()[0]);

    // Match with $_ wildcard (should match anything and bind, but value is ignored)
    let query_state =
        compile("(match &self (generation $_ $p $a) (ancestor $p $a))").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query_state.source()[0];
    let (results, _) = eval(expr, env, &query_state);

    assert_eq!(results.len(), 3, "Should match all generation levels");
}

#[test]
fn test_multiple_wildcards() {
    let mut env = new_env();

    // Add facts with multiple fields
    let fact1 = compile("(record 1 foo 100 alpha)").expect("compile failed");
    let fact2 = compile("(record 2 bar 200 beta)").expect("compile failed");
    env.add_to_space(&fact1.source()[0]);
    env.add_to_space(&fact2.source()[0]);

    // Match with multiple wildcards
    let query_state = compile("(match &self (record _ $name _ $code) (item $name $code))")
        .expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query_state.source()[0];
    let (results, _) = eval(expr, env, &query_state);

    assert_eq!(results.len(), 2, "Should match both records");
}

#[test]
fn test_wildcard_in_nested_pattern() {
    let mut env = new_env();

    // Add nested facts
    let fact1 = compile("(data (info 1 foo) result)").expect("compile failed");
    let fact2 = compile("(data (info 2 bar) result)").expect("compile failed");
    env.add_to_space(&fact1.source()[0]);
    env.add_to_space(&fact2.source()[0]);

    // Match with wildcard in nested position
    let query_state =
        compile("(match &self (data (info _ $x) result) $x)").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr = query_state.source()[0];
    let (results, _) = eval(expr, env, &query_state);

    assert_eq!(results.len(), 2, "Should match both nested patterns");
}

#[test]
fn test_wildcard_vs_variable() {
    let mut env = new_env();

    // Add facts
    let fact1 = compile("(pair 1 1)").expect("compile failed");
    let fact2 = compile("(pair 1 2)").expect("compile failed");
    let fact3 = compile("(pair 2 2)").expect("compile failed");
    env.add_to_space(&fact1.source()[0]);
    env.add_to_space(&fact2.source()[0]);
    env.add_to_space(&fact3.source()[0]);

    // Match with variable (requires both to be same)
    let query1_state = compile("(match &self (pair $x $x) (same $x))").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr1 = query1_state.source()[0];
    let (results1, _) = eval(expr1, env.clone(), &query1_state);
    assert_eq!(results1.len(), 2, "Should match pairs with same values");

    // Match with wildcard (ignores first value)
    let query2_state = compile("(match &self (pair _ $y) $y)").expect("compile failed");
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let expr2 = query2_state.source()[0];
    let (results2, _) = eval(expr2, env, &query2_state);
    assert_eq!(results2.len(), 3, "Should match all pairs");
}
