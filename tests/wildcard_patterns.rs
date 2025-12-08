//! Tests for wildcard pattern matching
//!
//! Tests that wildcard patterns ($_ and _) work correctly in all contexts

use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;

#[test]
fn test_underscore_wildcard() {
    let mut env = Environment::new();

    // Add facts
    env.add_to_space(&compile("(data 1 foo)").unwrap().source[0]);
    env.add_to_space(&compile("(data 2 bar)").unwrap().source[0]);
    env.add_to_space(&compile("(data 3 baz)").unwrap().source[0]);

    // Match with underscore wildcard (should match anything, but not bind)
    let query = compile("(match &self (data _ $value) $value)").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    assert_eq!(results.len(), 3, "Should match all three facts");
}

#[test]
fn test_dollar_underscore_wildcard() {
    let mut env = Environment::new();

    // Add generation facts like in ancestor.mm2 line 38
    env.add_to_space(&compile("(generation Z Alice Bob)").unwrap().source[0]);
    env.add_to_space(&compile("(generation (S Z) Bob Carol)").unwrap().source[0]);
    env.add_to_space(&compile("(generation (S (S Z)) Carol Dave)").unwrap().source[0]);

    // Match with $_ wildcard (should match anything and bind, but value is ignored)
    let query = compile("(match &self (generation $_ $p $a) (ancestor $p $a))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    assert_eq!(results.len(), 3, "Should match all generation levels");
}

#[test]
fn test_multiple_wildcards() {
    let mut env = Environment::new();

    // Add facts with multiple fields
    env.add_to_space(&compile("(record 1 foo 100 alpha)").unwrap().source[0]);
    env.add_to_space(&compile("(record 2 bar 200 beta)").unwrap().source[0]);

    // Match with multiple wildcards
    let query = compile("(match &self (record _ $name _ $code) (item $name $code))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    assert_eq!(results.len(), 2, "Should match both records");
}

#[test]
fn test_wildcard_in_nested_pattern() {
    let mut env = Environment::new();

    // Add nested facts
    env.add_to_space(&compile("(data (info 1 foo) result)").unwrap().source[0]);
    env.add_to_space(&compile("(data (info 2 bar) result)").unwrap().source[0]);

    // Match with wildcard in nested position
    let query = compile("(match &self (data (info _ $x) result) $x)").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    assert_eq!(results.len(), 2, "Should match both nested patterns");
}

#[test]
fn test_wildcard_vs_variable() {
    let mut env = Environment::new();

    // Add facts
    env.add_to_space(&compile("(pair 1 1)").unwrap().source[0]);
    env.add_to_space(&compile("(pair 1 2)").unwrap().source[0]);
    env.add_to_space(&compile("(pair 2 2)").unwrap().source[0]);

    // Match with variable (requires both to be same)
    let query1 = compile("(match &self (pair $x $x) (same $x))").unwrap();
    let (results1, _) = eval(query1.source[0].clone(), env.clone());
    assert_eq!(results1.len(), 2, "Should match pairs with same values");

    // Match with wildcard (ignores first value)
    let query2 = compile("(match &self (pair _ $y) $y)").unwrap();
    let (results2, _) = eval(query2.source[0].clone(), env);
    assert_eq!(results2.len(), 3, "Should match all pairs");
}
