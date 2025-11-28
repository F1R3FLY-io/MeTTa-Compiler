//! Tests for MORK fact removal operations
//!
//! Tests the new remove_from_space() and remove_matching() functionality
//! added for PR #1 (feature/mork-fact-removal).

use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;

#[test]
fn test_remove_exact_fact() {
    let mut env = Environment::new();

    // Add a fact
    let source = "(foo bar)";
    let state = compile(source).unwrap();
    let fact = &state.source[0];
    env.add_to_space(fact);

    // Verify it exists
    let query_source = "(match &self (foo bar) (foo bar))";
    let query_state = compile(query_source).unwrap();
    let (results, _) = eval(query_state.source[0].clone(), env.clone());
    assert_eq!(results.len(), 1, "Fact should exist before removal");

    // Remove the fact
    env.remove_from_space(fact);

    // Verify it's gone
    let (results_after, _) = eval(query_state.source[0].clone(), env.clone());
    assert_eq!(results_after.len(), 0, "Fact should be removed");
}

#[test]
fn test_remove_nonexistent_fact() {
    let mut env = Environment::new();

    // Try to remove a fact that doesn't exist (should not panic)
    let source = "(foo bar)";
    let state = compile(source).unwrap();
    env.remove_from_space(&state.source[0]);

    // Should complete without error
}

#[test]
fn test_remove_from_multiple_facts() {
    let mut env = Environment::new();

    // Add multiple facts
    let source = r#"
        (parent Alice Bob)
        (parent Bob Carol)
        (parent Carol Dave)
    "#;
    let state = compile(source).unwrap();
    for expr in &state.source {
        env.add_to_space(expr);
    }

    // Remove middle fact
    env.remove_from_space(&state.source[1]);

    // Verify first and third still exist
    let query1 = compile("(match &self (parent Alice Bob) (parent Alice Bob))").unwrap();
    let (results1, _) = eval(query1.source[0].clone(), env.clone());
    assert_eq!(results1.len(), 1, "First fact should still exist");

    let query3 = compile("(match &self (parent Carol Dave) (parent Carol Dave))").unwrap();
    let (results3, _) = eval(query3.source[0].clone(), env.clone());
    assert_eq!(results3.len(), 1, "Third fact should still exist");

    // Verify middle is gone
    let query2 = compile("(match &self (parent Bob Carol) (parent Bob Carol))").unwrap();
    let (results2, _) = eval(query2.source[0].clone(), env.clone());
    assert_eq!(results2.len(), 0, "Middle fact should be removed");
}

#[test]
fn test_operation_remove_via_direct_api() {
    // This test uses the direct API (add_to_space/remove_from_space) since
    // testing through exec requires more complex integration testing.
    // The example file (examples/mork_removal_demo.metta) demonstrates
    // the full exec-based workflow with (O (+ fact)) and (O (- fact)).

    let mut env = Environment::new();

    // Add a fact directly
    let source = "(temp foo)";
    let state = compile(source).unwrap();
    let fact = &state.source[0];
    env.add_to_space(fact);

    // Verify it exists via match
    let query = compile("(match &self (temp foo) (temp foo))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env.clone());
    assert_eq!(results.len(), 1, "Fact should be added");

    // Remove the fact directly
    env.remove_from_space(fact);

    // Verify it's gone
    let (results_after, _) = eval(query.source[0].clone(), env.clone());
    assert_eq!(results_after.len(), 0, "Fact should be removed");
}

#[test]
fn test_remove_and_readd() {
    let mut env = Environment::new();

    let source = "(data test)";
    let state = compile(source).unwrap();
    let fact = &state.source[0];

    // Add
    env.add_to_space(fact);

    // Remove
    env.remove_from_space(fact);

    // Re-add
    env.add_to_space(fact);

    // Verify it exists
    let query = compile("(match &self (data test) (data test))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env.clone());
    assert_eq!(results.len(), 1, "Fact should exist after re-adding");
}

#[test]
fn test_remove_multiple_identical_facts() {
    let mut env = Environment::new();

    let source = "(foo bar)";
    let state = compile(source).unwrap();
    let fact = &state.source[0];

    // Add same fact twice (PathMap handles duplicates)
    env.add_to_space(fact);
    env.add_to_space(fact);

    // Remove once
    env.remove_from_space(fact);

    // Verify it's gone (PathMap doesn't store duplicates)
    let query = compile("(match &self (foo bar) (foo bar))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env.clone());
    assert_eq!(results.len(), 0, "Fact should be removed");
}

#[test]
fn test_remove_complex_sexpr() {
    let mut env = Environment::new();

    // Add complex nested structure (ground fact - no variables)
    let source = "(rule (pattern (A (B C))) (body (D E)))";
    let state = compile(source).unwrap();
    let fact = &state.source[0];

    env.add_to_space(fact);

    // Verify exists
    let query_source = "(match &self (rule (pattern (A (B C))) (body (D E))) (rule (pattern (A (B C))) (body (D E))))";
    let query = compile(query_source).unwrap();
    let (results, _) = eval(query.source[0].clone(), env.clone());
    assert_eq!(results.len(), 1, "Complex fact should exist");

    // Remove
    env.remove_from_space(fact);

    // Verify gone
    let (results_after, _) = eval(query.source[0].clone(), env.clone());
    assert_eq!(results_after.len(), 0, "Complex fact should be removed");
}
