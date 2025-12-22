//! Tests for dynamic exec generation (MORK meta-programming)
//!
//! Tests Phase 3 of fixed-point evaluation: exec rules that match and generate
//! other exec rules, enabling the meta-programming pattern from ancestor.mm2 lines 33-36.

use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;
use mettatron::backend::eval::fixed_point::{eval_env_to_fixed_point, ExecRule};

#[test]
fn test_exec_stored_as_fact() {
    let mut env = Environment::new();

    // Define an exec rule
    let exec_source = "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))";
    let exec_state = compile(exec_source).unwrap();

    // Evaluate the exec (should store it as a fact)
    let (_results, new_env) = eval(exec_state.source[0].clone(), env);
    env = new_env;

    // Query for exec facts
    let query_source = "(match &self (exec $p $a $c) (exec $p $a $c))";
    let query_state = compile(query_source).unwrap();
    let (matches, _) = eval(query_state.source[0].clone(), env);

    // Should find the exec rule we added
    assert!(!matches.is_empty(), "Exec should be stored as a fact");
}

#[test]
fn test_match_exec_in_antecedent() {
    let mut env = Environment::new();

    // Add an exec rule as a fact
    let exec1 = compile("(exec (1 0) (, (a $x)) (, (b $x)))").unwrap();
    env.add_to_space(&exec1.source[0]);

    // Query to match exec rules with priority pattern
    let query = compile("(match &self (exec (1 $n) $a $c) (found (1 $n)))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    // Should match and bind $n to 0
    assert_eq!(results.len(), 1, "Should match exec rule in antecedent");
}

#[test]
fn test_exec_in_consequent_not_executed() {
    let mut env = Environment::new();

    // Create a rule that generates an exec in its consequent
    // (exec (0 0) (, (trigger)) (, (exec (1 0) (, (a $x)) (, (b $x)))))
    let meta_rule =
        compile("(exec (0 0) (, (trigger)) (, (exec (1 0) (, (a $x)) (, (b $x)))))").unwrap();
    env.add_to_space(&meta_rule.source[0]);

    // Add trigger fact
    env.add_to_space(&compile("(trigger)").unwrap().source[0]);

    // Evaluate the meta-rule (should NOT immediately execute the generated exec)
    let rule = ExecRule::from_sexpr(&meta_rule.source[0]).unwrap();
    env = mettatron::backend::eval::fixed_point::eval_to_fixed_point(vec![rule], env, 10).env;

    // The generated exec should be in space as a fact
    let query = compile("(match &self (exec (1 0) $a $c) (exec (1 0) $a $c))").unwrap();
    let (results, _) = eval(query.source[0].clone(), env);

    assert!(
        !results.is_empty(),
        "Generated exec should be stored as fact"
    );
}

#[test]
fn test_simple_meta_programming() {
    let mut env = Environment::new();

    // Meta-rule: when we see (level Z), generate (exec ...) for next level
    // Simplified version of ancestor.mm2 lines 33-36
    let meta_rule_source = "
        (exec (0 0)
            (, (level Z))
            (, (exec (1 0) (, (base $x)) (, (result $x)))))
    ";

    // IMPORTANT: Evaluate the exec to register it (not just add as fact)
    let meta_rule = compile(meta_rule_source).unwrap();
    let (_, env_after_exec) = eval(meta_rule.source[0].clone(), env);
    env = env_after_exec;

    // Add trigger fact
    env.add_to_space(&compile("(level Z)").unwrap().source[0]);

    // Debug: Check what exec rules are in space before fixed-point
    let before_query = compile("(match &self (exec $p $a $c) (exec $p $a $c))").unwrap();
    let (before_execs, _) = eval(before_query.source[0].clone(), env.clone());
    println!("Exec rules before fixed-point: {}", before_execs.len());

    // Debug: Check if (level Z) is in space
    let level_query = compile("(match &self (level Z) (level Z))").unwrap();
    let (level_matches, _) = eval(level_query.source[0].clone(), env.clone());
    println!(
        "(level Z) facts before fixed-point: {}",
        level_matches.len()
    );

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 10);
    println!(
        "Fixed-point result: iterations={}, converged={}, facts_added={}",
        result.iterations, result.converged, result.facts_added
    );

    // Debug: Check all facts after fixed-point
    let after_all = compile("(match &self $x $x)").unwrap();
    let (all_facts, _) = eval(after_all.source[0].clone(), final_env.clone());
    println!("Total facts after: {}", all_facts.len());

    // Should have generated the new exec rule
    let query = compile("(match &self (exec (1 0) $a $c) (exec (1 0) $a $c))").unwrap();
    let (results, _) = eval(query.source[0].clone(), final_env.clone());
    println!("Exec rules with priority (1 0): {}", results.len());

    assert!(!results.is_empty(), "Meta-rule should generate new exec");
    assert!(result.converged, "Should converge");
}

#[test]
fn test_peano_successor_generation() {
    let mut env = Environment::new();

    // Rule that matches (level Z) and generates (level (S Z))
    let rule = compile(
        "
        (exec (0 0)
            (, (level Z))
            (, (level (S Z))))
    ",
    )
    .unwrap();

    env.add_to_space(&rule.source[0]);
    env.add_to_space(&compile("(level Z)").unwrap().source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 10);

    // Should have generated (level (S Z))
    let query = compile("(match &self (level (S Z)) (level (S Z)))").unwrap();
    let (results, _) = eval(query.source[0].clone(), final_env);

    assert!(!results.is_empty(), "Should generate successor level");
    assert!(result.converged, "Should converge");
}

#[test]
fn test_generation_chain() {
    let mut env = Environment::new();

    // Rule that generates next generation
    // Simulates ancestor.mm2 pattern without full complexity
    let rule = compile(
        "
        (exec (0 0)
            (, (gen Z $c $p))
            (, (gen (S Z) $c $gp) (parent $p $gp)))
    ",
    )
    .unwrap();

    // Add initial facts
    env.add_to_space(&rule.source[0]);
    env.add_to_space(&compile("(gen Z Alice Bob)").unwrap().source[0]);
    env.add_to_space(&compile("(parent Bob Carol)").unwrap().source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 10);

    // Should have generated (gen (S Z) Alice Carol)
    let query = compile("(match &self (gen (S Z) Alice Carol) (gen (S Z) Alice Carol))").unwrap();
    let (results, _) = eval(query.source[0].clone(), final_env.clone());

    assert!(!results.is_empty(), "Should generate next generation");
    assert!(result.converged, "Should converge");

    // Verify exactly what was generated
    let all_gens = compile("(match &self (gen $n $c $p) (gen $n $c $p))").unwrap();
    let (all_results, _) = eval(all_gens.source[0].clone(), final_env);
    assert!(all_results.len() >= 2, "Should have at least 2 generations");
}

#[test]
fn test_ancestor_mm2_pattern_simplified() {
    // This is a simplified version of ancestor.mm2 lines 33-36
    // The full pattern is: match exec rules with priority (1 $l),
    // then generate new exec rules with priority (1 (S $l))

    let mut env = Environment::new();

    // Initial exec rule for generation tracking
    let base_rule = compile(
        "
        (exec (1 Z)
            (, (poi $c) (parent $c $p))
            (, (gen Z $c $p)))
    ",
    )
    .unwrap();

    // Meta-rule that generates successor rules (simplified ancestor.mm2 lines 33-36)
    let meta_rule = compile(
        "
        (exec (0 0)
            (, (exec (1 Z) $a $c) (gen Z $child $parent) (parent $parent $gp))
            (, (exec (1 (S Z)) $a $c) (gen (S Z) $child $gp)))
    ",
    )
    .unwrap();

    // Add rules
    env.add_to_space(&base_rule.source[0]);
    env.add_to_space(&meta_rule.source[0]);

    // Add facts
    env.add_to_space(&compile("(poi Ann)").unwrap().source[0]);
    env.add_to_space(&compile("(parent Ann Bob)").unwrap().source[0]);
    env.add_to_space(&compile("(parent Bob Carol)").unwrap().source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 20);

    // Should have generated both generations
    let query_z = compile("(match &self (gen Z Ann Bob) (gen Z Ann Bob))").unwrap();
    let (results_z, _) = eval(query_z.source[0].clone(), final_env.clone());

    let query_s = compile("(match &self (gen (S Z) Ann Carol) (gen (S Z) Ann Carol))").unwrap();
    let (results_s, _) = eval(query_s.source[0].clone(), final_env);

    assert!(!results_z.is_empty(), "Should generate Z generation");
    assert!(!results_s.is_empty(), "Should generate (S Z) generation");
    assert!(result.converged, "Should converge");
    assert!(result.iterations > 1, "Should take multiple iterations");
}

#[test]
fn test_fixed_point_convergence() {
    let mut env = Environment::new();

    // Rule that only fires once (no infinite generation)
    let rule = compile(
        "
        (exec (0 0)
            (, (start))
            (, (done)))
    ",
    )
    .unwrap();

    env.add_to_space(&rule.source[0]);
    env.add_to_space(&compile("(start)").unwrap().source[0]);

    // Run to fixed point
    let (_final_env, result) = eval_env_to_fixed_point(env, 10);

    // Should converge quickly
    assert!(result.converged, "Should reach fixed point");
    assert!(result.iterations <= 3, "Should converge in few iterations");
}

#[test]
fn test_iteration_limit_safety() {
    let mut env = Environment::new();

    // Rule that could generate infinite facts (if implemented poorly)
    // Our implementation should handle this gracefully
    let rule = compile(
        "
        (exec (0 0)
            (, (infinite))
            (, (more)))
    ",
    )
    .unwrap();

    env.add_to_space(&rule.source[0]);
    env.add_to_space(&compile("(infinite)").unwrap().source[0]);

    // Run with low iteration limit
    let (_final_env, result) = eval_env_to_fixed_point(env, 5);

    // Should hit iteration limit or converge
    assert!(result.iterations <= 5, "Should respect iteration limit");
}

#[test]
fn test_priority_ordering_with_dynamic_exec() {
    let mut env = Environment::new();

    // Lower priority rule (executes first)
    let low = compile("(exec (0 0) (, (trigger)) (, (low)))").unwrap();
    // Higher priority rule (executes second)
    let high = compile("(exec (1 0) (, (trigger)) (, (high)))").unwrap();

    env.add_to_space(&low.source[0]);
    env.add_to_space(&high.source[0]);
    env.add_to_space(&compile("(trigger)").unwrap().source[0]);

    // Run to fixed point
    let (final_env, result) = eval_env_to_fixed_point(env, 10);

    // Both should execute (priority affects order, not whether they execute)
    let query_low = compile("(match &self (low) (low))").unwrap();
    let (results_low, _) = eval(query_low.source[0].clone(), final_env.clone());

    let query_high = compile("(match &self (high) (high))").unwrap();
    let (results_high, _) = eval(query_high.source[0].clone(), final_env);

    assert!(!results_low.is_empty(), "Low priority rule should execute");
    assert!(
        !results_high.is_empty(),
        "High priority rule should execute"
    );
    assert!(result.converged, "Should converge");
}
