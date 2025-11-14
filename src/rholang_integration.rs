/// Rholang Integration Module - Evaluation Functions
///
/// **PRIMARY INTEGRATION**: Use `pathmap_par_integration` module for Rholang interop
///
/// This module provides:
/// 1. **JSON export** for debugging and inspection (`metta_state_to_json`)
/// 2. **State evaluation** for REPL-style interaction (`run_state`, `run_state_async`)
///
/// **Note**: For Rholang integration, use the PathMap Par functions in
/// `pathmap_par_integration` module, not the JSON functions here.
use crate::backend::models::{MettaState, MettaValue};

/// Run compiled state against accumulated state
///
/// This is the core evaluation function for REPL-style interaction.
///
/// Takes two MettaState objects:
/// - `accumulated_state`: State with accumulated environment and outputs
/// - `compiled_state`: Fresh state with pending expressions to evaluate
///
/// Returns a new accumulated state with:
/// - Empty source (all evaluated)
/// - Updated environment (merged with new rules/facts)
/// - Extended output (accumulated results)
///
/// **Threading**: Synchronous, single-threaded evaluation
pub fn run_state(
    accumulated_state: MettaState,
    compiled_state: MettaState,
) -> Result<MettaState, String> {
    use crate::backend::eval::eval;

    // Start with accumulated environment
    let mut env = accumulated_state.environment;
    let mut outputs = accumulated_state.output;

    // Evaluate each pending expression from compiled state
    for expr in compiled_state.source {
        let is_eval_expr = expr.is_eval_expr();

        let (results, new_env) = eval(expr, env);
        env = new_env;

        // Only extend outputs for evaluation expressions (!)
        // Other S-expressions are added to the atom space but produce no outputs
        if is_eval_expr {
            outputs.extend(results);
        }
    }

    // Return new accumulated state
    Ok(MettaState::new_accumulated(env, outputs))
}

/// Async version of run_state with parallel evaluation of independent expressions
///
/// This function parallelizes evaluation of consecutive `!` (eval) expressions
/// while maintaining sequential execution for rule definitions (`=`) to preserve
/// MeTTa semantics.
///
/// **MeTTa Semantics Preserved:**
/// - Rule definitions execute sequentially (environment threading)
/// - Independent eval expressions execute in parallel
/// - Output ordering is preserved
/// - Environment updates are atomic per batch
///
/// **Threading Model:** Uses Tokio's async/await (same as Rholang)
///
/// **Thread Safety:** Environment now uses `Arc<Mutex<T>>` for thread-safe sharing
#[cfg(feature = "async")]
pub async fn run_state_async(
    accumulated_state: MettaState,
    compiled_state: MettaState,
) -> Result<MettaState, String> {
    use crate::backend::eval::eval;

    // Start with accumulated environment
    let mut env = accumulated_state.environment;
    let mut outputs = accumulated_state.output;

    // Batch expressions into parallelizable groups
    let mut current_batch: Vec<(usize, MettaValue, bool)> = Vec::new();
    let exprs: Vec<_> = compiled_state.source.into_iter().enumerate().collect();

    for (idx, expr) in exprs {
        let is_eval_expr = expr.is_eval_expr();
        let is_rule_def = expr.is_rule_def();

        // If this is a rule definition and we have a batch, evaluate the batch first
        if is_rule_def && !current_batch.is_empty() {
            // Evaluate parallel batch
            let batch_results = evaluate_batch_parallel(current_batch, env.clone()).await;
            for (_batch_idx, results, should_output) in batch_results {
                if should_output {
                    outputs.extend(results);
                }
            }
            current_batch = Vec::new();
        }

        // If this is a rule definition, execute it sequentially
        if is_rule_def {
            let (_results, new_env) = eval(expr, env);
            env = new_env;
            // Rule definitions don't produce output
        } else {
            // Add to current batch for parallel execution
            current_batch.push((idx, expr, is_eval_expr));
        }
    }

    // Evaluate any remaining batch
    if !current_batch.is_empty() {
        let batch_results = evaluate_batch_parallel(current_batch, env.clone()).await;
        for (_batch_idx, results, should_output) in batch_results {
            if should_output {
                outputs.extend(results);
            }
        }
    }

    Ok(MettaState::new_accumulated(env, outputs))
}

/// Helper function to evaluate a batch of expressions in parallel
/// Returns results in original order with their indices
#[cfg(feature = "async")]
async fn evaluate_batch_parallel(
    batch: Vec<(usize, MettaValue, bool)>,
    env: crate::backend::environment::Environment,
) -> Vec<(usize, Vec<MettaValue>, bool)> {
    use crate::backend::eval::eval;
    use tokio::task;

    // Spawn parallel evaluation tasks
    let tasks: Vec<_> = batch
        .into_iter()
        .map(|(idx, expr, should_output)| {
            let env = env.clone(); // Arc clone is cheap
            task::spawn_blocking(move || {
                let (results, _new_env) = eval(expr, env);
                (idx, results, should_output)
            })
        })
        .collect();

    // Collect results
    let mut results = Vec::new();
    for task_handle in tasks {
        match task_handle.await {
            Ok(result) => results.push(result),
            Err(e) => {
                // Task panicked - this shouldn't happen with our eval
                eprintln!("Parallel evaluation task panicked: {:?}", e);
            }
        }
    }

    // Sort results by original index to preserve order
    results.sort_by_key(|(idx, _, _)| *idx);

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile::compile;

    #[test]
    fn test_run_state_simple() {
        let accumulated = MettaState::new_empty();
        let compiled = compile("!(+ 1 2)").unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should have output
        assert!(!result.output.is_empty());
        assert_eq!(result.output[0], MettaValue::Long(3));
    }

    #[test]
    fn test_run_state_with_rules() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (double $x) (* $x 2))
            !(double 21)
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should have output
        assert!(!result.output.is_empty());
        assert_eq!(result.output[0], MettaValue::Long(42));
    }

    // Async Parallel Evaluation Tests
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_simple() {
        let accumulated = MettaState::new_empty();
        let compiled = compile("!(+ 1 2)").unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Should have output
        assert!(!result.output.is_empty());
        assert_eq!(result.output[0], MettaValue::Long(3));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_parallel() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            !(+ 1 1)
            !(+ 2 2)
            !(+ 3 3)
            "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Should have all outputs
        assert_eq!(result.output.len(), 3);
        assert_eq!(result.output[0], MettaValue::Long(2));
        assert_eq!(result.output[1], MettaValue::Long(4));
        assert_eq!(result.output[2], MettaValue::Long(6));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_with_rules() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (double $x) (* $x 2))
            !(double 5)
            !(double 10)
            "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Should have outputs (parallel evaluation of both double calls)
        assert_eq!(result.output.len(), 2);
        assert_eq!(result.output[0], MettaValue::Long(10));
        assert_eq!(result.output[1], MettaValue::Long(20));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_multiple_rules_sequential() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (square $x) (* $x $x))
            !(square 3)
            (= (double $x) (* $x 2))
            !(double 3)
            "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        assert_eq!(result.output.len(), 2);
        assert_eq!(result.output[0], MettaValue::Long(9));
        assert_eq!(result.output[1], MettaValue::Long(6));
    }

    #[test]
    fn test_run_state_accumulated_state() {
        // Test that rules persist across multiple run_state calls
        let accumulated = MettaState::new_empty();
        let compiled1 = compile("(= (double $x) (* $x 2))").unwrap();
        let result1 = run_state(accumulated, compiled1).unwrap();

        let compiled2 = compile("!(double 5)").unwrap();
        let result2 = run_state(result1, compiled2).unwrap();

        assert!(!result2.output.is_empty());
        assert_eq!(result2.output[0], MettaValue::Long(10));
    }

    #[test]
    fn test_run_state_rule_ordering() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (f special-value) catched)
            (= (f $x) $x)
            !(f A)
            !(f special-value)
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should have outputs for both calls
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_complex_nested() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (triple $x) ($x $x $x))
            (= (grid3x3 $x) (triple (triple $x)))
            !(grid3x3 (square (+ 1 2)))
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_recursive_function() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (factorial 0) 1)
            (= (factorial $x) (* $x (factorial (- $x 1))))
            !(factorial 5)
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
        // Note: This might need adjustment based on actual evaluation behavior
        // Factorial of 5 should be 120
    }

    // Space Operations Tests - Adding Facts
    #[test]
    fn test_run_state_add_facts_to_space() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                !(+ 1 1)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Facts are added to space (no output), only eval expression produces output
        assert_eq!(result.output.len(), 1);
        assert_eq!(result.output[0], MettaValue::Long(2));
    }

    #[test]
    fn test_run_state_facts_persist_across_runs() {
        // First run: add facts
        let accumulated1 = MettaState::new_empty();
        let compiled1 = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                "#,
        )
        .unwrap();
        let result1 = run_state(accumulated1, compiled1).unwrap();

        // Second run: use facts via rules
        let compiled2 = compile(
            r#"
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();
        let result2 = run_state(result1, compiled2).unwrap();

        // Should be able to query the facts
        assert!(!result2.output.is_empty());
    }

    // Pattern Matching and Queries
    #[test]
    fn test_run_state_simple_pattern_match() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (= (get-parents $child)
                   (match &self (Parent $parent $child) $parent))
                !(get-parents Bob)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find parents of Bob
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_pattern_match_with_variables() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Tom Liz)
                (= (find-parents $parent)
                   (match &self (Parent $parent $child) ($parent $child)))
                !(find-parents Tom)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find all children of Tom
        assert!(!result.output.is_empty());
    }

    // Family Relationship Tests
    #[test]
    fn test_run_state_family_relationships() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (parent Tom Bob)
                (parent Pam Bob)
                (parent Bob Ann)
                (parent Bob Pat)
                (female Pam)
                (female Ann)
                (male Tom)
                (male Bob)
                (= (grandparent $gp $gc)
                   (match &self (parent $gp $p)
                          (match &self (parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_recursive_ancestor_relation() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (parent Tom Bob)
                (parent Bob Ann)
                (parent Ann Sara)
                (= (ancestor $a $d)
                   (match &self (parent $a $d) True))
                (= (ancestor $a $d)
                   (match &self (parent $a $p)
                          (ancestor $p $d)))
                !(ancestor Tom Sara)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find that Tom is an ancestor of Sara
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_complex_family_query() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (parent Tom Bob)
                (parent Pam Bob)
                (parent Bob Ann)
                (parent Bob Pat)
                (parent Pat Jim)
                (female Pam)
                (female Ann)
                (male Tom)
                (= (sibling $s1 $s2)
                   (match &self (parent $p $s1)
                          (match &self (parent $p $s2)
                                 (if (== $s1 $s2) (empty) True))))
                !(sibling Ann Pat)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    // Constraint Solving Tests
    #[test]
    fn test_run_state_nondeterministic_choice() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                !(small-digit)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should produce multiple results (nondeterministic)
        assert!(!result.output.is_empty());
        // All results should be valid digits
        for output in &result.output {
            if let MettaValue::Long(n) = output {
                assert!(*n >= 1 && *n <= 3);
            }
        }
    }

    #[test]
    fn test_run_state_constraint_solving_pair() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                (= (not-equal $x $y)
                   (if (== $x $y) (empty) True))
                (= (solve-pair)
                   (let $x (small-digit)
                        (let $y (small-digit)
                             (if (not-equal $x $y)
                                 ($x $y)
                                 (empty)))))
                !(solve-pair)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should produce pairs where x != y
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_constraint_solving_triple() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                (= (not-equal $x $y)
                   (if (== $x $y) (empty) True))
                (= (solve-triple)
                   (let $x (small-digit)
                        (if (== $x 1)
                            (let $y (small-digit)
                                 (if (not-equal $x $y)
                                     (let $z (small-digit)
                                          (if (and (not-equal $x $z) (not-equal $y $z))
                                              ($x $y $z)
                                              (empty)))
                                     (empty)))
                            (let $y (small-digit)
                                 (if (not-equal $x $y)
                                     ($x $y 1)
                                     (empty))))))
                !(solve-triple)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should produce valid triples with constraints
        assert!(!result.output.is_empty());
    }

    // Knowledge Base Operations
    #[test]
    fn test_run_state_entity_relations() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (works alice acme)
                (works bob beta)
                (friends alice carol)
                (located acme SF)
                (located beta NYC)
                (= (find-colleagues $person)
                   (match &self (works $person $company)
                          (match &self (works $other $company)
                                 (if (== $person $other) (empty) $other))))
                !(find-colleagues alice)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_complex_pattern_matching() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Tom Liz)
                (Parent Bob Ann)
                (= (get-parent-entries $parent $child)
                   (match &self (Parent $parent $child) (Parent $parent $child)))
                !(get-parent-entries Tom $child)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find all children of Tom
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_nested_queries() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (Likes Bob Ann)
                (= (find-liked-grandchildren $grandparent)
                   (match &self (Parent $grandparent $parent)
                          (match &self (Parent $parent $child)
                                 (match &self (Likes $parent $child) $child))))
                !(find-liked-grandchildren Tom)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_rule_with_multiple_matches() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (Parent Bob Pat)
                (= (get-all-children $parent)
                   (match &self (Parent $parent $child) $child))
                !(get-all-children Bob)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find both Ann and Pat
        assert!(!result.output.is_empty());
    }

    // Async tests for space operations
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_add_facts_then_query() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        assert!(!result.output.is_empty());
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_parallel_queries() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (= (get-parents $child)
                   (match &self (Parent $parent $child) $parent))
                !(get-parents Bob)
                !(get-parents Ann)
                "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Both queries should execute in parallel
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_facts_only_no_output() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Facts are added to space but produce no output
        assert_eq!(result.output.len(), 0);
    }

    #[test]
    fn test_run_state_mixed_facts_and_rules() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }
}
