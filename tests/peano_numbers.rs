//! Tests for Peano number support in MORK
//!
//! Tests that Peano numbers (Z, S Z, S (S Z), ...) work correctly
//! in pattern matching and evaluation for PR #2 (feature/mork-peano-numbers).

use mettatron::{compile_arena, eval_arena, new_arena_env};

#[test]
fn test_peano_zero_literal() {
    let env = new_arena_env();

    // Parse Peano zero
    let source = "Z";
    let state = compile_arena(source).expect("compile failed");

    // Z should be an atom
    assert_eq!(state.source().len(), 1);

    // Evaluate Z (should return itself)
    let (results, _) = eval_arena(state.source()[0], env, &state);
    assert_eq!(results.len(), 1);
}

#[test]
fn test_peano_successor_literals() {
    let env = new_arena_env();

    // Parse Peano successors
    let source1 = "(S Z)";
    let state1 = compile_arena(source1).expect("compile failed");
    assert_eq!(state1.source().len(), 1);

    let source2 = "(S (S Z))";
    let state2 = compile_arena(source2).expect("compile failed");
    assert_eq!(state2.source().len(), 1);

    let source3 = "(S (S (S Z)))";
    let state3 = compile_arena(source3).expect("compile failed");
    assert_eq!(state3.source().len(), 1);

    // Evaluate (should return themselves as they're ground)
    let (results1, _) = eval_arena(state1.source()[0], env.clone(), &state1);
    assert_eq!(results1.len(), 1);

    let (results2, _) = eval_arena(state2.source()[0], env.clone(), &state2);
    assert_eq!(results2.len(), 1);

    let (results3, _) = eval_arena(state3.source()[0], env.clone(), &state3);
    assert_eq!(results3.len(), 1);
}

#[test]
fn test_peano_pattern_matching_zero() {
    let mut env = new_arena_env();

    // Add a fact with Peano zero
    let fact_source = "(number Z)";
    let fact_state = compile_arena(fact_source).expect("compile failed");
    env.add_to_space(&fact_state.source()[0]);

    // Query for Z
    let query_source = "(match &self (number Z) (number Z))";
    let query_state = compile_arena(query_source).expect("compile failed");
    let (results, _) = eval_arena(query_state.source()[0], env, &query_state);

    assert_eq!(results.len(), 1, "Should match Peano zero");
}

#[test]
fn test_peano_pattern_matching_successor() {
    let mut env = new_arena_env();

    // Add facts with Peano successors
    let fact1 = compile_arena("(number (S Z))").expect("compile failed");
    env.add_to_space(&fact1.source()[0]);

    let fact2 = compile_arena("(number (S (S Z)))").expect("compile failed");
    env.add_to_space(&fact2.source()[0]);

    // Query for exact match
    let query1 = compile_arena("(match &self (number (S Z)) (number (S Z)))").expect("compile failed");
    let (results1, _) = eval_arena(query1.source()[0], env.clone(), &query1);
    assert_eq!(results1.len(), 1, "Should match (S Z)");

    // Query for pattern with variable
    let query2 = compile_arena("(match &self (number (S $x)) (S $x))").expect("compile failed");
    let (results2, _) = eval_arena(query2.source()[0], env, &query2);
    assert_eq!(
        results2.len(),
        2,
        "Should match both successors with variable"
    );
}

#[test]
fn test_peano_nested_pattern_matching() {
    let mut env = new_arena_env();

    // Add fact with nested Peano
    let fact = compile_arena("(generation (S (S Z)) Alice Bob)").expect("compile failed");
    env.add_to_space(&fact.source()[0]);

    // Query with exact match
    let query1 =
        compile_arena("(match &self (generation (S (S Z)) Alice Bob) (generation (S (S Z)) Alice Bob))")
            .expect("compile failed");
    let (results1, _) = eval_arena(query1.source()[0], env.clone(), &query1);
    assert_eq!(results1.len(), 1, "Should match exact Peano structure");

    // Query with variable in Peano
    let query2 = compile_arena("(match &self (generation $n Alice Bob) $n)").expect("compile failed");
    let (results2, _) = eval_arena(query2.source()[0], env, &query2);
    assert_eq!(
        results2.len(),
        1,
        "Should match and bind Peano number to variable"
    );
}

#[test]
fn test_peano_in_rules() {
    let env = new_arena_env();

    // Define a rule using Peano numbers
    let rule_source = "(= (next Z) (S Z))";
    let rule_state = compile_arena(rule_source).expect("compile failed");
    let (_, env) = eval_arena(rule_state.source()[0], env, &rule_state);

    // Query the rule
    let query = compile_arena("!(next Z)").expect("compile failed");
    let (results, _) = eval_arena(query.source()[0], env, &query);

    // Should get (S Z) as result
    assert_eq!(results.len(), 1, "Rule should produce successor");
}

#[test]
fn test_peano_pattern_destructuring() {
    let mut env = new_arena_env();

    // Add facts with Peano numbers
    let s1 = compile_arena("(num (S Z))").expect("compile failed");
    env.add_to_space(&s1.source()[0]);
    let s2 = compile_arena("(num (S (S Z)))").expect("compile failed");
    env.add_to_space(&s2.source()[0]);
    let s3 = compile_arena("(num (S (S (S Z))))").expect("compile failed");
    env.add_to_space(&s3.source()[0]);

    // Match pattern (S $x) to get the predecessor
    let query = compile_arena("(match &self (num (S $x)) $x)").expect("compile failed");
    let (results, _) = eval_arena(query.source()[0], env, &query);

    // Should match all three and bind Z, (S Z), (S (S Z))
    assert_eq!(
        results.len(),
        3,
        "Should destructure all three Peano numbers"
    );
}

#[test]
fn test_peano_in_space_operations() {
    let mut env = new_arena_env();

    // Add Peano number via direct API
    let peano_fact = compile_arena("(count (S (S (S Z))))").expect("compile failed");
    env.add_to_space(&peano_fact.source()[0]);

    // Verify it exists
    let query = compile_arena("(match &self (count (S (S (S Z)))) (count (S (S (S Z)))))").expect("compile failed");
    let (results, _) = eval_arena(query.source()[0], env.clone(), &query);
    assert_eq!(results.len(), 1, "Peano fact should be in space");

    // Remove it
    env.remove_from_space(&peano_fact.source()[0]);

    // Verify it's gone
    let (results_after, _) = eval_arena(query.source()[0], env, &query);
    assert_eq!(results_after.len(), 0, "Peano fact should be removed");
}
