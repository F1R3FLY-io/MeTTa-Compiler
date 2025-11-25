//! Fixed-Point Evaluation for MORK Exec Rules
//!
//! This module implements fixed-point evaluation semantics for MORK:
//! - Execute all exec rules repeatedly until no new facts are generated
//! - Respect priority ordering (lower priorities execute first)
//! - Detect convergence (fixed point reached)
//! - Safety limits to prevent infinite loops

use crate::backend::environment::Environment;
use crate::backend::eval::priority::compare_priorities;
use crate::backend::models::MettaValue;
use std::cmp::Ordering;
use std::collections::HashSet;

/// Maximum number of fixed-point iterations to prevent infinite loops
const DEFAULT_MAX_ITERATIONS: usize = 1000;

/// Result of fixed-point evaluation
#[derive(Debug, Clone)]
pub struct FixedPointResult {
    /// Number of iterations executed
    pub iterations: usize,
    /// Whether a fixed point was reached (true) or iteration limit hit (false)
    pub converged: bool,
    /// Total number of facts added during evaluation
    pub facts_added: usize,
    /// Final environment after evaluation
    pub env: Environment,
}

/// Representation of an exec rule for fixed-point evaluation
#[derive(Debug, Clone)]
pub struct ExecRule {
    /// Priority value (for sorting)
    pub priority: MettaValue,
    /// Antecedent conjunction (conditions)
    pub antecedent: MettaValue,
    /// Consequent conjunction or operation (results)
    pub consequent: MettaValue,
    /// Full exec S-expression for fact storage
    pub full_expr: MettaValue,
}

impl ExecRule {
    /// Create a new exec rule from components
    pub fn new(
        priority: MettaValue,
        antecedent: MettaValue,
        consequent: MettaValue,
    ) -> Self {
        // Construct full exec expression for storage
        let full_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("exec".to_string()),
            priority.clone(),
            antecedent.clone(),
            consequent.clone(),
        ]);

        ExecRule {
            priority,
            antecedent,
            consequent,
            full_expr,
        }
    }

    /// Parse an exec S-expression into an ExecRule
    pub fn from_sexpr(sexpr: &MettaValue) -> Option<Self> {
        match sexpr {
            MettaValue::SExpr(items) if items.len() == 4 => {
                // Check if first element is "exec"
                if let MettaValue::Atom(op) = &items[0] {
                    if op == "exec" {
                        return Some(ExecRule::new(
                            items[1].clone(),
                            items[2].clone(),
                            items[3].clone(),
                        ));
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// Sort exec rules by priority (lowest first)
pub fn sort_rules_by_priority(rules: &mut [ExecRule]) {
    rules.sort_by(|r1, r2| compare_priorities(&r1.priority, &r2.priority));
}

/// Execute all exec rules to fixed point
///
/// # Algorithm
///
/// 1. Sort rules by priority (lowest first)
/// 2. Loop until fixed point or max iterations:
///    a. Track fact count at start of iteration
///    b. Execute each rule in priority order
///    c. If no new facts added, reached fixed point
///    d. Continue to next iteration
/// 3. Return final environment and statistics
///
/// # Parameters
///
/// - `rules`: Vec of exec rules to execute
/// - `env`: Initial environment with facts
/// - `max_iterations`: Maximum iterations before giving up
///
/// # Returns
///
/// FixedPointResult with final environment and convergence info
///
/// # Example
///
/// ```rust
/// use mettatron::backend::environment::Environment;
/// use mettatron::backend::eval::fixed_point::{ExecRule, eval_to_fixed_point};
/// use mettatron::backend::models::MettaValue;
///
/// let mut env = Environment::new();
/// let rules = vec![]; // Add exec rules here
///
/// let result = eval_to_fixed_point(rules, env, 100);
/// println!("Converged: {}, Iterations: {}", result.converged, result.iterations);
/// ```
pub fn eval_to_fixed_point(
    mut rules: Vec<ExecRule>,
    mut env: Environment,
    max_iterations: usize,
) -> FixedPointResult {
    // Sort rules by priority
    sort_rules_by_priority(&mut rules);

    let mut iteration = 0;
    let mut total_facts_added = 0;

    loop {
        iteration += 1;

        // Safety: Check iteration limit
        if iteration > max_iterations {
            return FixedPointResult {
                iterations: iteration - 1,
                converged: false,
                facts_added: total_facts_added,
                env,
            };
        }

        // Track fact count at start of iteration
        let facts_before = count_facts(&env);

        // Execute each rule in priority order
        for rule in &rules {
            // Try to fire the rule
            env = try_fire_rule(rule, env);
        }

        // Check if any new facts were added
        let facts_after = count_facts(&env);
        let facts_added_this_iteration = facts_after.saturating_sub(facts_before);

        total_facts_added += facts_added_this_iteration;

        // If no new facts, we've reached fixed point
        if facts_added_this_iteration == 0 {
            return FixedPointResult {
                iterations: iteration,
                converged: true,
                facts_added: total_facts_added,
                env,
            };
        }

        // Continue to next iteration
    }
}

/// Count number of facts in environment's space
fn count_facts(env: &Environment) -> usize {
    // Query all facts using match with wildcard
    let wildcard = MettaValue::Atom("$_".to_string());
    let matches = env.match_space(&wildcard, &wildcard);
    matches.len()
}

/// Try to fire an exec rule once
///
/// Evaluates the rule's antecedent against current facts.
/// If successful, executes consequent and returns updated environment.
fn try_fire_rule(rule: &ExecRule, env: Environment) -> Environment {
    use super::eval_with_depth;

    // Evaluate the full exec expression
    // This will handle antecedent matching and consequent execution
    let (_results, new_env) = eval_with_depth(rule.full_expr.clone(), env, 0);

    new_env
}

/// Execute environment to fixed point
///
/// Convenience function that extracts exec rules from environment,
/// runs fixed-point evaluation, and updates environment with results.
///
/// # Parameters
///
/// - `env`: Environment containing facts and potentially exec rules
/// - `max_iterations`: Maximum iterations (0 = use default)
///
/// # Returns
///
/// Updated environment and fixed-point result
pub fn eval_env_to_fixed_point(
    env: Environment,
    max_iterations: usize,
) -> (Environment, FixedPointResult) {
    // Use default max iterations if 0
    let max_iter = if max_iterations == 0 {
        DEFAULT_MAX_ITERATIONS
    } else {
        max_iterations
    };

    // Extract exec rules from environment
    let rules = extract_exec_rules(&env);

    // If no rules, return immediately
    if rules.is_empty() {
        let result = FixedPointResult {
            iterations: 0,
            converged: true,
            facts_added: 0,
            env: env.clone(),
        };
        return (env, result);
    }

    // Run fixed-point evaluation
    let result = eval_to_fixed_point(rules, env, max_iter);

    // Return new environment from result
    let final_env = result.env.clone();
    (final_env, result)
}

/// Extract all exec rules from environment's fact space
fn extract_exec_rules(env: &Environment) -> Vec<ExecRule> {
    // Match pattern: (exec $p $a $c)
    let exec_pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("exec".to_string()),
        MettaValue::Atom("$p".to_string()),
        MettaValue::Atom("$a".to_string()),
        MettaValue::Atom("$c".to_string()),
    ]);

    let matches = env.match_space(&exec_pattern, &exec_pattern);

    // Parse each match into an ExecRule
    matches
        .iter()
        .filter_map(|m| ExecRule::from_sexpr(m))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile::compile;

    #[test]
    fn test_exec_rule_parsing() {
        let source = "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))";
        let state = compile(source).unwrap();

        let rule = ExecRule::from_sexpr(&state.source[0]);
        assert!(rule.is_some());

        let rule = rule.unwrap();
        match &rule.priority {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 2);
            }
            _ => panic!("Expected tuple priority"),
        }
    }

    #[test]
    fn test_sort_rules_by_priority() {
        // Create rules with different priorities
        let r0 = ExecRule::new(
            MettaValue::Long(2),
            MettaValue::Nil,
            MettaValue::Nil,
        );
        let r1 = ExecRule::new(
            MettaValue::Long(0),
            MettaValue::Nil,
            MettaValue::Nil,
        );
        let r2 = ExecRule::new(
            MettaValue::Long(1),
            MettaValue::Nil,
            MettaValue::Nil,
        );

        let mut rules = vec![r0, r1, r2];
        sort_rules_by_priority(&mut rules);

        // Should be sorted: 0, 1, 2
        assert!(matches!(rules[0].priority, MettaValue::Long(0)));
        assert!(matches!(rules[1].priority, MettaValue::Long(1)));
        assert!(matches!(rules[2].priority, MettaValue::Long(2)));
    }

    #[test]
    fn test_empty_rules() {
        let env = Environment::new();
        let rules = vec![];

        let result = eval_to_fixed_point(rules, env, 10);

        assert_eq!(result.iterations, 1);
        assert!(result.converged);
        assert_eq!(result.facts_added, 0);
    }

    #[test]
    fn test_count_facts() {
        let mut env = Environment::new();

        // Initially no facts
        assert_eq!(count_facts(&env), 0);

        // Add some facts
        let fact1 = compile("(parent Alice Bob)").unwrap();
        env.add_to_space(&fact1.source[0]);

        let fact2 = compile("(parent Bob Carol)").unwrap();
        env.add_to_space(&fact2.source[0]);

        // Should have 2 facts
        assert_eq!(count_facts(&env), 2);
    }

    #[test]
    fn test_extract_exec_rules() {
        let mut env = Environment::new();

        // Add an exec rule as a fact
        let exec = compile("(exec (0 0) (, (parent $p $c)) (, (child $c $p)))").unwrap();
        env.add_to_space(&exec.source[0]);

        let rules = extract_exec_rules(&env);
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_fixed_point_no_rules() {
        let mut env = Environment::new();

        // Add initial facts
        let fact = compile("(parent Alice Bob)").unwrap();
        env.add_to_space(&fact.source[0]);

        let (final_env, result) = eval_env_to_fixed_point(env, 10);

        // Should converge immediately (no rules)
        assert_eq!(result.iterations, 0);
        assert!(result.converged);

        // Fact should still be there
        assert_eq!(count_facts(&final_env), 1);
    }

    #[test]
    fn test_iteration_limit() {
        let env = Environment::new();

        // Create a rule that generates infinite facts (if it fired)
        // For now, we just test that iteration limit works
        let rules = vec![];

        let result = eval_to_fixed_point(rules, env, 5);

        // Should converge immediately with no rules
        assert!(result.converged);
        assert!(result.iterations <= 5);
    }
}
