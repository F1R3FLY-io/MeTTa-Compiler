//! Integration test for ancestor.mm2 patterns
//!
//! Tests the complete ancestor.mm2 logic with real MORK patterns

use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;
use mettatron::backend::eval::fixed_point::eval_env_to_fixed_point;

#[test]
fn test_ancestor_mm2_simple() {
    let mut env = Environment::new();

    // Add parent facts
    env.add_to_space(&compile("(parent Bob Ann)").unwrap().source[0]);
    env.add_to_space(&compile("(parent Tom Bob)").unwrap().source[0]);
    env.add_to_space(&compile("(parent Pam Bob)").unwrap().source[0]);

    // Add point of interest
    env.add_to_space(&compile("(poi Ann)").unwrap().source[0]);

    // Rule 1: parent -> child
    let rule1 = compile("(exec (0 0) (, (parent $p $c)) (, (child $c $p)))").unwrap();
    env.add_to_space(&rule1.source[0]);

    // Rule 2: poi + child -> generation Z
    let rule2 =
        compile("(exec (0 1) (, (poi $c) (child $c $p)) (, (generation Z $c $p)))").unwrap();
    env.add_to_space(&rule2.source[0]);

    // Rule 3: Meta-rule that generates successor rules (simplified from lines 33-36)
    let rule3 = compile(
        "
        (exec (1 Z)
            (, (generation Z $c $p) (child $p $gp))
            (, (generation (S Z) $c $gp)))
    ",
    )
    .unwrap();
    env.add_to_space(&rule3.source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 20);

    println!(
        "Fixed-point result: iterations={}, converged={}, facts_added={}",
        result.iterations, result.converged, result.facts_added
    );

    // Should have generated generation facts
    let query_z = compile("(match &self (generation Z Ann Bob) (generation Z Ann Bob))").unwrap();
    let (results_z, _) = eval(query_z.source[0].clone(), final_env.clone());

    let query_s =
        compile("(match &self (generation (S Z) Ann $gp) (generation (S Z) Ann $gp))").unwrap();
    let (results_s, _) = eval(query_s.source[0].clone(), final_env.clone());

    println!("Generation Z facts: {}", results_z.len());
    println!("Generation (S Z) facts: {}", results_s.len());

    assert!(!results_z.is_empty(), "Should generate generation Z facts");
    assert!(result.converged, "Should converge");
}

#[test]
fn test_ancestor_mm2_child_derivation() {
    let mut env = Environment::new();

    // Add parent facts
    env.add_to_space(&compile("(parent Tom Bob)").unwrap().source[0]);
    env.add_to_space(&compile("(parent Bob Ann)").unwrap().source[0]);

    // Add rule: parent -> child
    let rule = compile("(exec (0 0) (, (parent $p $c)) (, (child $c $p)))").unwrap();
    env.add_to_space(&rule.source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 10);

    // Should have derived child relationships
    let query = compile("(match &self (child Bob Tom) (child Bob Tom))").unwrap();
    let (results, _) = eval(query.source[0].clone(), final_env.clone());

    assert!(
        !results.is_empty(),
        "Should derive (child Bob Tom) from (parent Tom Bob)"
    );
    assert!(result.converged, "Should converge");

    // Check for other child
    let query2 = compile("(match &self (child Ann Bob) (child Ann Bob))").unwrap();
    let (results2, _) = eval(query2.source[0].clone(), final_env);

    assert!(
        !results2.is_empty(),
        "Should derive (child Ann Bob) from (parent Bob Ann)"
    );
}

#[test]
fn test_ancestor_mm2_generation_z() {
    let mut env = Environment::new();

    // Add facts
    env.add_to_space(&compile("(parent Bob Ann)").unwrap().source[0]);
    env.add_to_space(&compile("(poi Ann)").unwrap().source[0]);

    // Rule 1: parent -> child
    let rule1 = compile("(exec (0 0) (, (parent $p $c)) (, (child $c $p)))").unwrap();
    env.add_to_space(&rule1.source[0]);

    // Rule 2: poi + child -> generation Z
    let rule2 =
        compile("(exec (0 1) (, (poi $c) (child $c $p)) (, (generation Z $c $p)))").unwrap();
    env.add_to_space(&rule2.source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 10);

    // Should have generated (generation Z Ann Bob)
    let query = compile("(match &self (generation Z Ann Bob) (generation Z Ann Bob))").unwrap();
    let (results, _) = eval(query.source[0].clone(), final_env);

    assert!(
        !results.is_empty(),
        "Should generate (generation Z Ann Bob)"
    );
    assert!(result.converged, "Should converge");
}

#[test]
fn test_ancestor_mm2_multiple_generations() {
    let mut env = Environment::new();

    // Three generation family: Tom -> Bob -> Ann
    env.add_to_space(&compile("(parent Tom Bob)").unwrap().source[0]);
    env.add_to_space(&compile("(parent Bob Ann)").unwrap().source[0]);
    env.add_to_space(&compile("(poi Ann)").unwrap().source[0]);

    // Rule 1: parent -> child
    let rule1 = compile("(exec (0 0) (, (parent $p $c)) (, (child $c $p)))").unwrap();
    env.add_to_space(&rule1.source[0]);

    // Rule 2: poi + child -> generation Z (base case)
    let rule2 =
        compile("(exec (0 1) (, (poi $c) (child $c $p)) (, (generation Z $c $p)))").unwrap();
    env.add_to_space(&rule2.source[0]);

    // Rule 3: Generation successor (simplified lines 33-36)
    let rule3 = compile(
        "
        (exec (1 Z)
            (, (generation Z $c $p) (child $p $gp))
            (, (generation (S Z) $c $gp)))
    ",
    )
    .unwrap();
    env.add_to_space(&rule3.source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 20);

    println!(
        "Iterations: {}, Converged: {}, Facts added: {}",
        result.iterations, result.converged, result.facts_added
    );

    // Should have (generation Z Ann Bob)
    let query_z = compile("(match &self (generation Z Ann Bob) (generation Z Ann Bob))").unwrap();
    let (results_z, _) = eval(query_z.source[0].clone(), final_env.clone());

    // Should have (generation (S Z) Ann Tom)
    let query_sz =
        compile("(match &self (generation (S Z) Ann Tom) (generation (S Z) Ann Tom))").unwrap();
    let (results_sz, _) = eval(query_sz.source[0].clone(), final_env.clone());

    println!("Generation Z: {}", results_z.len());
    println!("Generation (S Z): {}", results_sz.len());

    assert!(!results_z.is_empty(), "Should have (generation Z Ann Bob)");
    assert!(
        !results_sz.is_empty(),
        "Should have (generation (S Z) Ann Tom)"
    );
    assert!(result.converged, "Should converge");
}
