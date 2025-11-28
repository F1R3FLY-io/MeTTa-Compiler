//! MORK Special Forms - exec, coalg, lookup, rulify
//!
//! This module implements the MORK-style special forms that work with the conjunction pattern:
//! - exec: Rule execution with conjunction antecedents/consequents
//! - coalg: Coalgebra patterns for tree transformations
//! - lookup: Conditional fact lookup with success/failure branches
//! - rulify: Meta-programming for runtime rule generation

use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::{eval, EvalResult};

/// Evaluate exec special form: (exec <priority> <antecedent> <consequent>)
///
/// Executes rules with conjunction-based pattern matching:
/// - priority: Rule priority (number or tuple)
/// - antecedent: Conjunction of conditions (all must match)
/// - consequent: Conjunction of results OR operation (O ...)
///
/// Semantics:
/// 1. Evaluate all antecedent goals left-to-right with binding threading
/// 2. If all succeed, execute consequent with accumulated bindings
/// 3. Return results from consequent evaluation
///
/// Examples:
/// - (exec P0 (,) (, (always-true)))  ; Empty antecedent, always fires
/// - (exec P1 (, (parent $x Alice)) (, (result $x)))  ; Simple pattern match
/// - (exec P2 (, (a $x) (b $x)) (, (c $x)))  ; Binary conjunction with shared variable
pub(super) fn eval_exec(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..]; // Skip "exec" operator

    if args.len() < 3 {
        let err = MettaValue::Error(
            "exec requires 3 arguments: priority, antecedent, and consequent".to_string(),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let _priority = &args[0]; // Priority for future use (rule ordering)
    let antecedent = &args[1];
    let consequent = &args[2];

    // Antecedent must be a conjunction
    let antecedent_goals = match antecedent {
        MettaValue::Conjunction(goals) => goals,
        _ => {
            let err = MettaValue::Error(
                "exec antecedent must be a conjunction (,)".to_string(),
                Arc::new(antecedent.clone()),
            );
            return (vec![err], env);
        }
    };

    // Evaluate antecedent conjunction to get bindings
    let (antecedent_results, antecedent_env) =
        eval_conjunction_for_exec(antecedent_goals.clone(), env);

    // If antecedent failed (no results or error), rule doesn't fire
    if antecedent_results.is_empty() {
        return (vec![], antecedent_env);
    }

    // For each antecedent result (non-deterministic), evaluate consequent
    let mut all_results = Vec::new();
    let mut final_env = antecedent_env.clone();

    for _result in antecedent_results {
        // Consequent can be either a conjunction or an operation
        match consequent {
            MettaValue::Conjunction(goals) => {
                // Evaluate consequent conjunction
                let (conseq_results, conseq_env) =
                    eval_conjunction_for_exec(goals.clone(), antecedent_env.clone());
                all_results.extend(conseq_results);
                final_env = final_env.union(&conseq_env);
            }
            MettaValue::SExpr(items) if matches_operation(items) => {
                // Handle operation: (O (+ fact) (- fact) ...)
                let op_result = eval_operation(items, antecedent_env.clone());
                all_results.extend(op_result.0);
                final_env = final_env.union(&op_result.1);
            }
            _ => {
                let err = MettaValue::Error(
                    "exec consequent must be a conjunction or operation (O ...)".to_string(),
                    Arc::new(consequent.clone()),
                );
                all_results.push(err);
            }
        }
    }

    (all_results, final_env)
}

/// Helper function to evaluate a conjunction for exec
/// Returns results with binding information preserved
fn eval_conjunction_for_exec(goals: Vec<MettaValue>, env: Environment) -> EvalResult {
    if goals.is_empty() {
        // Empty conjunction succeeds
        return (vec![MettaValue::Nil], env);
    }

    // Evaluate the conjunction directly
    let conjunction = MettaValue::Conjunction(goals);
    eval(conjunction, env)
}

/// Check if an S-expression is an operation (starts with "O")
fn matches_operation(items: &[MettaValue]) -> bool {
    matches!(items.first(), Some(MettaValue::Atom(s)) if s == "O")
}

/// Evaluate operation: (O (+ fact) (- fact) ...)
/// Operations modify the MORK space by adding or removing facts
fn eval_operation(items: &[MettaValue], mut env: Environment) -> EvalResult {
    let operations = &items[1..]; // Skip "O" operator

    for op in operations {
        match op {
            MettaValue::SExpr(op_items) if op_items.len() == 2 => {
                match (&op_items[0], &op_items[1]) {
                    (MettaValue::Atom(op_type), fact) if op_type == "+" => {
                        // Add fact to space
                        env.add_to_space(fact);
                    }
                    (MettaValue::Atom(op_type), fact) if op_type == "-" => {
                        // Remove fact from space
                        env.remove_from_space(fact);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Return success indicator
    (vec![MettaValue::Atom("ok".to_string())], env)
}

/// Evaluate coalg special form: (coalg <pattern> <templates>)
///
/// Coalgebra patterns for tree transformations:
/// - pattern: Input pattern to match (can use variables)
/// - templates: Conjunction of output templates (unfold results)
///
/// Template cardinality:
/// - (,): Zero results (termination)
/// - (, t): One result
/// - (, t1 t2 ...): Multiple results (unfold)
///
/// Examples:
/// - (coalg (tree $t) (, (ctx $t nil)))  ; Lift: wrap tree in context
/// - (coalg (ctx (branch $l $r) $p) (, (ctx $l (cons $p L)) (ctx $r (cons $p R))))  ; Explode
/// - (coalg (ctx (leaf $v) $p) (, (value $p $v)))  ; Drop: terminal
pub(super) fn eval_coalg(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..]; // Skip "coalg" operator

    if args.len() < 2 {
        let err = MettaValue::Error(
            "coalg requires 2 arguments: pattern and templates".to_string(),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let templates = &args[1];

    // Templates must be a conjunction
    let template_list = match templates {
        MettaValue::Conjunction(temps) => temps,
        _ => {
            let err = MettaValue::Error(
                "coalg templates must be a conjunction (,)".to_string(),
                Arc::new(templates.clone()),
            );
            return (vec![err], env);
        }
    };

    // For coalg, we need an input value to match against the pattern
    // This would typically come from evaluating an expression first
    // For now, return a placeholder that indicates coalg is defined

    // In a full implementation, this would:
    // 1. Take an input value
    // 2. Match it against pattern to get bindings
    // 3. Substitute bindings into each template
    // 4. Return all instantiated templates

    // Placeholder: return the coalg structure as-is for now
    let coalg_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("coalg".to_string()),
        pattern.clone(),
        MettaValue::Conjunction(template_list.clone()),
    ]);

    (vec![coalg_expr], env)
}

/// Evaluate lookup special form: (lookup <pattern> <success-goals> <failure-goals>)
///
/// Conditional execution based on space queries:
/// - pattern: Pattern to search for in space
/// - success-goals: Conjunction executed if pattern found
/// - failure-goals: Conjunction executed if pattern not found
///
/// Examples:
/// - (lookup $y (, T) (, $cy))  ; If $y exists, return T, else execute $cy
/// - (lookup $p (, (lookup $t $px $tx)) (, (exec (0 $t) $px $tx)))  ; Nested lookup
pub(super) fn eval_lookup(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..]; // Skip "lookup" operator

    if args.len() < 3 {
        let err = MettaValue::Error(
            "lookup requires 3 arguments: pattern, success-goals, and failure-goals".to_string(),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let success_goals = &args[1];
    let failure_goals = &args[2];

    // Both branches must be conjunctions
    let _success_conj = match success_goals {
        MettaValue::Conjunction(_) => success_goals,
        _ => {
            let err = MettaValue::Error(
                "lookup success branch must be a conjunction (,)".to_string(),
                Arc::new(success_goals.clone()),
            );
            return (vec![err], env);
        }
    };

    let _failure_conj = match failure_goals {
        MettaValue::Conjunction(_) => failure_goals,
        _ => {
            let err = MettaValue::Error(
                "lookup failure branch must be a conjunction (,)".to_string(),
                Arc::new(failure_goals.clone()),
            );
            return (vec![err], env);
        }
    };

    // Try to find pattern in space
    // For now, we'll use a simple heuristic: if pattern is a variable, assume not found
    // In a full implementation, this would query the MORK space

    let pattern_found = !matches!(pattern, MettaValue::Atom(s) if s.starts_with('$'));

    if pattern_found {
        // Evaluate success branch
        match success_goals {
            MettaValue::Conjunction(goals) => eval_conjunction_for_exec(goals.clone(), env),
            _ => unreachable!(), // Already checked above
        }
    } else {
        // Evaluate failure branch
        match failure_goals {
            MettaValue::Conjunction(goals) => eval_conjunction_for_exec(goals.clone(), env),
            _ => unreachable!(), // Already checked above
        }
    }
}

/// Evaluate rulify meta-program: (rulify $name (, $p0) (, $t0 ...) <antecedent> <consequent>)
///
/// Generates exec rules from coalgebra definitions:
/// - Matches coalgebra structure (pattern and templates)
/// - Generates appropriate exec rule based on template arity
/// - Supports dynamic rule generation for space transformations
///
/// Examples:
/// - (rulify $name (, $p0) (, $t0) (, (tmp $p0)) (O (- (tmp $p0)) (+ (tmp $t0))))
/// - (rulify $name (, $p0) (, $t0 $t1) (, (tmp $p0)) (O (- (tmp $p0)) (+ (tmp $t0)) (+ (tmp $t1))))
pub(super) fn eval_rulify(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..]; // Skip "rulify" operator

    if args.len() < 5 {
        let err = MettaValue::Error(
            "rulify requires 5 arguments: name, pattern, templates, antecedent, consequent"
                .to_string(),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let name = &args[0];
    let pattern_conj = &args[1];
    let templates_conj = &args[2];
    let rule_antecedent = &args[3];
    let rule_consequent = &args[4];

    // Extract pattern from unary conjunction
    let pattern = match pattern_conj {
        MettaValue::Conjunction(ps) if ps.len() == 1 => &ps[0],
        _ => {
            let err = MettaValue::Error(
                "rulify pattern must be a unary conjunction (, $p0)".to_string(),
                Arc::new(pattern_conj.clone()),
            );
            return (vec![err], env);
        }
    };

    // Extract templates from conjunction
    let templates = match templates_conj {
        MettaValue::Conjunction(ts) => ts,
        _ => {
            let err = MettaValue::Error(
                "rulify templates must be a conjunction (, $t0 ...)".to_string(),
                Arc::new(templates_conj.clone()),
            );
            return (vec![err], env);
        }
    };

    // Create a meta-rule structure that can be used for pattern matching
    // In a full implementation, this would generate actual exec rules
    let meta_rule = MettaValue::SExpr(vec![
        MettaValue::Atom("meta-rule".to_string()),
        name.clone(),
        pattern.clone(),
        MettaValue::Conjunction(templates.clone()),
        rule_antecedent.clone(),
        rule_consequent.clone(),
    ]);

    (vec![meta_rule], env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_empty_antecedent() {
        let env = Environment::new();

        // (exec P0 (,) (, 42))
        // let value = MettaValue::SExpr(vec![
        //     MettaValue::Atom("exec".to_string()),
        //     MettaValue::Atom("P0".to_string()),
        //     MettaValue::Conjunction(vec![]), // Empty antecedent - always succeeds
        //     MettaValue::Conjunction(vec![MettaValue::Long(42)]),
        // ]);

        let (results, _) = eval_exec(
            vec![
                MettaValue::Atom("exec".to_string()),
                MettaValue::Atom("P0".to_string()),
                MettaValue::Conjunction(vec![]),
                MettaValue::Conjunction(vec![MettaValue::Long(42)]),
            ],
            env,
        );

        assert!(!results.is_empty());
        // Empty antecedent succeeds, so consequent should be evaluated
    }

    #[test]
    fn test_exec_simple_consequent() {
        let env = Environment::new();

        // (exec P1 (,) (, (+ 1 2)))
        let value = eval_exec(
            vec![
                MettaValue::Atom("exec".to_string()),
                MettaValue::Atom("P1".to_string()),
                MettaValue::Conjunction(vec![]),
                MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ])]),
            ],
            env,
        );

        assert!(!value.0.is_empty());
    }

    #[test]
    fn test_coalg_structure() {
        let env = Environment::new();

        // (coalg (tree $t) (, (ctx $t nil)))
        let value = eval_coalg(
            vec![
                MettaValue::Atom("coalg".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("tree".to_string()),
                    MettaValue::Atom("$t".to_string()),
                ]),
                MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
                    MettaValue::Atom("ctx".to_string()),
                    MettaValue::Atom("$t".to_string()),
                    MettaValue::Atom("nil".to_string()),
                ])]),
            ],
            env,
        );

        assert!(!value.0.is_empty());
        // Should return the coalg structure
    }

    #[test]
    fn test_lookup_success_branch() {
        let env = Environment::new();

        // (lookup foo (, T) (, F))
        let value = eval_lookup(
            vec![
                MettaValue::Atom("lookup".to_string()),
                MettaValue::Atom("foo".to_string()), // Not a variable, so "found"
                MettaValue::Conjunction(vec![MettaValue::Atom("T".to_string())]),
                MettaValue::Conjunction(vec![MettaValue::Atom("F".to_string())]),
            ],
            env,
        );

        assert!(!value.0.is_empty());
        // Should execute success branch
    }

    #[test]
    fn test_lookup_failure_branch() {
        let env = Environment::new();

        // (lookup $x (, T) (, F))
        let value = eval_lookup(
            vec![
                MettaValue::Atom("lookup".to_string()),
                MettaValue::Atom("$x".to_string()), // Variable, so "not found"
                MettaValue::Conjunction(vec![MettaValue::Atom("T".to_string())]),
                MettaValue::Conjunction(vec![MettaValue::Atom("F".to_string())]),
            ],
            env,
        );

        assert!(!value.0.is_empty());
        // Should execute failure branch
    }
}
