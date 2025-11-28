//! Tests for Peano number support in MORK
//!
//! Tests that Peano numbers (Z, S Z, S (S Z), ...) work correctly
//! in pattern matching and evaluation for PR #2 (feature/mork-peano-numbers).

use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;

#[test]
fn test_peano_zero_literal() {
    let env = Environment::new();

    // Parse Peano zero
    let source = "Z";
    let state = compile(source).unwrap();

    // Z should be an atom
    assert_eq!(state.source.len(), 1);

    // Evaluate Z (should return itself)
    let (results, _) = eval(state.source[0].clone(), env);
    assert_eq!(results.len(), 1);
}

#[test]
fn test_peano_successor_literals() {
    let env = Environment::new();

    // Parse Peano successors
    let source1 = "(S Z)";
    let state1 = compile(source1).unwrap();
    assert_eq!(state1.source.len(), 1);

    let source2 = "(S (S Z))";
    let state2 = compile(source2).unwrap();
    assert_eq!(state2.source.len(), 1);

    let source3 = "(S (S (S Z)))";
    let state3 = compile(source3).unwrap();
    assert_eq!(state3.source.len(), 1);

    // Evaluate (should return themselves as they're ground)
    let (results1, _) = eval(state1.source[0].clone(), env.clone());
    assert_eq!(results1.len(), 1);

    let (results2, _) = eval(state2.source[0].clone(), env.clone());
    assert_eq!(results2.len(), 1);

    let (results3, _) = eval(state3.source[0].clone(), env.clone());
    assert_eq!(results3.len(), 1);
}

#[test]
fn test_peano_pattern_matching_zero() {
    let mut env = Environment::new();

    // Add a fact with Peano zero
    let fact_source = "(number Z)";
    let fact_state = compile(fact_source).unwrap();
    env.add_to_space(&fact_state.source[0]);

    // Query for Z
    let query_source = "(match &self (number Z) (number Z))";
    let query_state = compile(query_source).unwrap();
    let (results, _) = eval(query_state.source[0].clone(), env);

    assert_eq!(results.len(), 1, "Should match Peano zero");
}

#[test]
fn test_peano_pattern_matching_successor() {
    let mut env = Environment::new();

    // Add facts with Peano successors
    let fact1 = compile("(number (S Z))").unwrap();
    env.add_to_space(&fact1.source[0]);

    let fact2 = compile("(number (S (S Z)))").unwrap();
    env.add_to_space(&fact2.source[0]);

    // Query for exact match
    let query1 = compile("(match &self (number (S Z)) (number (S Z)))").unwrap();
    let (results1, _) = eval(query1.source[0].clone(), env.clone());
    assert_eq!(results1.len(), 1, "Should match (S Z)");

    // Query for pattern with variable
    let query2 = compile("(match &self (number (S $x)) (S $x))").unwrap();
    let (results2, _) = eval(query2.source[0].clone(), env);
    assert_eq!(
        results2.len(),
        2,
        "Should match both successors with variable"
    );
}

#[test]
fn test_peano_nested_pattern_matching() {
    let mut env = Environment::new();

    // Add fact with nested Peano
    let fact = compile("(generation (S (S Z)) Alice Bob)").unwrap();
    env.add_to_space(&fact.source[0]);

    // Query with exact match
    let query1 =
        compile("(match &self (generation (S (S Z)) Alice Bob) (generation (S (S Z)) Alice Bob))")
            .unwrap();
    let (results1, _) = eval(query1.source[0].clone(), env.clone());
    assert_eq!(results1.len(), 1, "Should match exact Peano structure");

    // Query with variable in Peano
    let query2 = compile("(match &self (generation $n Alice Bob) $n)").unwrap();
    let (results2, _) = eval(query2.source[0].clone(), env);
    assert_eq!(
        results2.len(),
        1,
        "Should match and bind Peano number to variable"
    );
}

#[test]
fn test_peano_in_rules() {
    let env = Environment::new();

    // Define a rule using Peano numbers
    let rule_source = "(= (next Z) (S Z))";
    let rule_state = compile(rule_source).unwrap();
    let (_, env) = eval(rule_state.source[0].clone(), env);

    // Query the rule
    let query = compile("!(next Z)").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    // Should get (S Z) as result
    assert_eq!(results.len(), 1, "Rule should produce successor");
}

#[test]
fn test_peano_pattern_destructuring() {
    let mut env = Environment::new();

    // Add facts with Peano numbers
    env.add_to_space(&compile("(num (S Z))").unwrap().source[0]);
    env.add_to_space(&compile("(num (S (S Z)))").unwrap().source[0]);
    env.add_to_space(&compile("(num (S (S (S Z))))").unwrap().source[0]);

    // Match pattern (S $x) to get the predecessor
    let query = compile("(match &self (num (S $x)) $x)").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    // Should match all three and bind Z, (S Z), (S (S Z))
    assert_eq!(
        results.len(),
        3,
        "Should destructure all three Peano numbers"
    );
}

#[test]
fn test_peano_in_space_operations() {
    let mut env = Environment::new();

    // Add Peano number via direct API
    let peano_fact = compile("(count (S (S (S Z))))").unwrap();
    env.add_to_space(&peano_fact.source[0]);

    // Verify it exists
    let query = compile("(match &self (count (S (S (S Z)))) (count (S (S (S Z)))))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env.clone());
    assert_eq!(results.len(), 1, "Peano fact should be in space");

    // Remove it
    env.remove_from_space(&peano_fact.source[0]);

    // Verify it's gone
    let (results_after, _) = eval(query.source[0].clone(), env);
    assert_eq!(results_after.len(), 0, "Peano fact should be removed");
}
