//! Tests for MORK fact removal operations
//!
//! Tests the new remove_from_space() and remove_matching() functionality
//! added for PR #1 (feature/mork-fact-removal).
//!
//! Migrated to use the arena API (compile, eval, new_env).

use mettatron::{compile, eval, new_env};

#[test]
fn test_remove_exact_fact() {
    let mut env = new_env();

    // Add a fact
    let source = "(foo bar)";
    let state = compile(source).expect("compile failed");
    let fact = &state.source()[0];
    env.add_to_space(fact);

    // Verify it exists
    let query_state = compile("(match &self (foo bar) (foo bar))").expect("compile failed");
    let (results, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(results.len(), 1, "Fact should exist before removal");

    // Remove the fact
    env.remove_from_space(fact);

    // Verify it's gone
    let (results_after, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(results_after.len(), 0, "Fact should be removed");
}

#[test]
fn test_remove_nonexistent_fact() {
    let mut env = new_env();

    // Try to remove a fact that doesn't exist (should not panic)
    let source = "(foo bar)";
    let state = compile(source).expect("compile failed");
    env.remove_from_space(&state.source()[0]);

    // Should complete without error
}

#[test]
fn test_remove_from_multiple_facts() {
    let mut env = new_env();

    // Add multiple facts
    let source = r#"
        (parent Alice Bob)
        (parent Bob Carol)
        (parent Carol Dave)
    "#;
    let state = compile(source).expect("compile failed");
    for &expr in state.source() {
        env.add_to_space(&expr);
    }

    // Remove middle fact
    env.remove_from_space(&state.source()[1]);

    // Verify first and third still exist
    let query1_state =
        compile("(match &self (parent Alice Bob) (parent Alice Bob))").expect("compile failed");
    let (results1, _) = eval(query1_state.source()[0], env.clone(), &query1_state);
    assert_eq!(results1.len(), 1, "First fact should still exist");

    let query3_state =
        compile("(match &self (parent Carol Dave) (parent Carol Dave))").expect("compile failed");
    let (results3, _) = eval(query3_state.source()[0], env.clone(), &query3_state);
    assert_eq!(results3.len(), 1, "Third fact should still exist");

    // Verify middle is gone
    let query2_state =
        compile("(match &self (parent Bob Carol) (parent Bob Carol))").expect("compile failed");
    let (results2, _) = eval(query2_state.source()[0], env.clone(), &query2_state);
    assert_eq!(results2.len(), 0, "Middle fact should be removed");
}

#[test]
fn test_operation_remove_via_direct_api() {
    // This test uses the direct API (add_to_space/remove_from_space) since
    // testing through exec requires more complex integration testing.
    // The example file (examples/mork_removal_demo.metta) demonstrates
    // the full exec-based workflow with (O (+ fact)) and (O (- fact)).

    let mut env = new_env();

    // Add a fact directly
    let source = "(temp foo)";
    let state = compile(source).expect("compile failed");
    let fact = &state.source()[0];
    env.add_to_space(fact);

    // Verify it exists via match
    let query_state =
        compile("(match &self (temp foo) (temp foo))").expect("compile failed");
    let (results, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(results.len(), 1, "Fact should be added");

    // Remove the fact directly
    env.remove_from_space(fact);

    // Verify it's gone
    let (results_after, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(results_after.len(), 0, "Fact should be removed");
}

#[test]
fn test_remove_and_readd() {
    let mut env = new_env();

    let source = "(data test)";
    let state = compile(source).expect("compile failed");
    let fact = &state.source()[0];

    // Add
    env.add_to_space(fact);

    // Remove
    env.remove_from_space(fact);

    // Re-add
    env.add_to_space(fact);

    // Verify it exists
    let query_state =
        compile("(match &self (data test) (data test))").expect("compile failed");
    let (results, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(results.len(), 1, "Fact should exist after re-adding");
}

#[test]
fn test_remove_multiple_identical_facts() {
    // MeTTa HE semantics: multiplicity tracking (ref-counting)
    // Adding the same fact N times creates multiplicity N
    // Removing once decrements multiplicity to N-1
    // match_space returns N copies for multiplicity N

    let mut env = new_env();

    let source = "(foo bar)";
    let state = compile(source).expect("compile failed");
    let fact = &state.source()[0];

    // Add same fact twice - creates multiplicity 2
    env.add_to_space(fact);
    env.add_to_space(fact);

    // Verify multiplicity is 2 (returns 2 copies)
    let query_state =
        compile("(match &self (foo bar) (foo bar))").expect("compile failed");
    let (results, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(
        results.len(),
        2,
        "Fact should exist with multiplicity 2 after adding twice"
    );

    // Remove once - decrements multiplicity from 2 to 1
    env.remove_from_space(fact);

    // Verify multiplicity is 1 (fact still exists)
    let (results_after_first_remove, _) =
        eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(
        results_after_first_remove.len(),
        1,
        "Fact should still exist with multiplicity 1 after removing once"
    );

    // Remove again - fully removes the fact (multiplicity 0)
    env.remove_from_space(fact);

    // Verify fact is fully removed
    let (results_after_second_remove, _) =
        eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(
        results_after_second_remove.len(),
        0,
        "Fact should be fully removed after removing twice"
    );
}

#[test]
fn test_remove_complex_sexpr() {
    let mut env = new_env();

    // Add complex nested structure (ground fact - no variables)
    let source = "(rule (pattern (A (B C))) (body (D E)))";
    let state = compile(source).expect("compile failed");
    let fact = &state.source()[0];

    env.add_to_space(fact);

    // Verify exists
    let query_source = "(match &self (rule (pattern (A (B C))) (body (D E))) (rule (pattern (A (B C))) (body (D E))))";
    let query_state = compile(query_source).expect("compile failed");
    let (results, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(results.len(), 1, "Complex fact should exist");

    // Remove
    env.remove_from_space(fact);

    // Verify gone
    let (results_after, _) = eval(query_state.source()[0], env.clone(), &query_state);
    assert_eq!(results_after.len(), 0, "Complex fact should be removed");
}
