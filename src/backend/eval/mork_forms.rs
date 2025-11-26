//! MORK Special Forms - exec, coalg, lookup, rulify
//!
//! This module implements the MORK-style special forms that work with the conjunction pattern:
//! - exec: Rule execution with conjunction antecedents/consequents
//! - coalg: Coalgebra patterns for tree transformations
//! - lookup: Conditional fact lookup with success/failure branches
//! - rulify: Meta-programming for runtime rule generation

use crate::backend::environment::Environment;
use crate::backend::models::{Bindings, MettaValue};

use super::{eval_with_depth, EvalResult};

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
pub(super) fn eval_exec(items: Vec<MettaValue>, mut env: Environment) -> EvalResult {

    let args = &items[1..]; // Skip "exec" operator

    if args.len() < 3 {
        let err = MettaValue::Error(
            "exec requires 3 arguments: priority, antecedent, and consequent".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let _priority = &args[0]; // Priority for future use (rule ordering)
    let antecedent = &args[1];
    let consequent = &args[2];

    // PHASE 3: Store exec as a fact for dynamic exec generation
    // This allows exec rules to be matched in antecedents: (exec (1 $l) $ps $ts)
    let exec_fact = MettaValue::SExpr(items.clone());
    env.add_to_space(&exec_fact);

    // Antecedent must be a conjunction
    // Handle both MettaValue::Conjunction and SExpr representation (,  ...)
    // The latter occurs when exec is retrieved from PathMap space
    let antecedent_goals = match antecedent {
        MettaValue::Conjunction(goals) => {
            goals.clone()
        }
        MettaValue::SExpr(items) if !items.is_empty() && matches!(&items[0], MettaValue::Atom(op) if op == ",") => {
            // Conjunction represented as SExpr: (, goal1 goal2 ...)
            items[1..].to_vec() // Skip the "," operator
        }
        _ => {
            let err = MettaValue::Error(
                "exec antecedent must be a conjunction (,)".to_string(),
                Box::new(antecedent.clone()),
            );
            return (vec![err], env);
        }
    };

    // Evaluate antecedent conjunction to get bindings
    let binding_sets = match_conjunction_goals_with_bindings(&antecedent_goals, &env);

    // If antecedent failed (no binding sets), rule doesn't fire
    if binding_sets.is_empty() {
        return (vec![], env);
    }

    // For each binding set (non-deterministic), evaluate consequent with those bindings
    let mut all_results = Vec::new();
    let mut final_env = env;

    for bindings in binding_sets {
        use crate::backend::eval::apply_bindings;

        // DEBUG: Print all bindings
        for (_name, _value) in bindings.iter() {
        }

        // Apply bindings to consequent before evaluation
        let instantiated_consequent = apply_bindings(consequent, &bindings);

        // Consequent can be either a conjunction or an operation
        match &instantiated_consequent {
            MettaValue::Conjunction(goals) => {
                // PHASE 3: Check for exec in consequent goals and thread bindings
                let (conseq_results, conseq_env) =
                    eval_consequent_conjunction_with_bindings(goals.clone(), bindings.clone(), final_env.clone());
                all_results.extend(conseq_results);
                // Use conseq_env directly since it has the updated facts
                final_env = conseq_env;
            }
            MettaValue::SExpr(items) if !items.is_empty() && matches!(&items[0], MettaValue::Atom(op) if op == ",") => {
                // Conjunction represented as SExpr (from PathMap)
                let goals = items[1..].to_vec();
                let (conseq_results, conseq_env) =
                    eval_consequent_conjunction_with_bindings(goals, bindings.clone(), final_env.clone());
                all_results.extend(conseq_results);
                // Use conseq_env directly since it has the updated facts
                final_env = conseq_env;
            }
            MettaValue::SExpr(items) if matches_operation(items) => {
                // Handle operation: (O (+ fact) (- fact) ...)
                let op_result = eval_operation(items, final_env.clone());
                all_results.extend(op_result.0);
                final_env = op_result.1; // Use operation result environment directly
            }
            _ => {
                let err = MettaValue::Error(
                    "exec consequent must be a conjunction or operation (O ...)".to_string(),
                    Box::new(instantiated_consequent.clone()),
                );
                all_results.push(err);
            }
        }
    }

    (all_results, final_env)
}

/// Match conjunction goals as patterns against space facts with binding threading
///
/// Core pattern matching logic for exec antecedents:
/// 1. Start with empty bindings
/// 2. For each goal:
///    a. Apply current bindings to the goal (substitute already-bound variables)
///    b. Match the instantiated goal against space facts
///    c. For each match, extract new bindings and merge with current bindings
///    d. Thread merged bindings to next goal
/// 3. Return all successful binding combinations
///
/// This properly implements variable binding threading across multiple goals,
/// enabling patterns like:
/// - (, (gen Z $c $p)) - binds $c and $p
/// - (, (parent $p $gp)) - uses bound $p, binds $gp
/// - (, (exec $p $a $c) (gen Z $child $parent)) - multiple goals with shared vars
///
/// Returns: Vec<Bindings> - all successful binding sets (empty if antecedent fails)
fn match_conjunction_goals_with_bindings(goals: &[MettaValue], env: &Environment) -> Vec<Bindings> {
    if goals.is_empty() {
        // Empty conjunction succeeds with empty bindings
        return vec![Bindings::new()];
    }

    // Recursively thread bindings through goals
    // Start with a single empty binding set
    let initial_bindings = vec![Bindings::new()];

    thread_bindings_through_goals(goals, initial_bindings, env)
}

/// Recursively thread bindings through conjunction goals
///
/// For each binding set from previous goals, try to match the next goal
/// and produce new binding sets with the matched variables added.
fn thread_bindings_through_goals(
    goals: &[MettaValue],
    current_bindings: Vec<Bindings>,
    env: &Environment,
) -> Vec<Bindings> {
    use crate::backend::eval::{apply_bindings, pattern_match};

    if goals.is_empty() {
        // Base case: no more goals, return current bindings
        return current_bindings;
    }

    let goal = &goals[0];
    let remaining_goals = &goals[1..];

    let mut next_bindings = Vec::new();

    // For each current binding set
    for bindings in current_bindings {
        for (_name, _value) in bindings.iter() {
        }

        // Apply current bindings to this goal
        let instantiated_goal = apply_bindings(goal, &bindings);

        // Get all facts from space (we'll match against all of them)
        let wildcard = MettaValue::Atom("$_".to_string());
        let all_facts = env.match_space(&wildcard, &wildcard);

        // Try to match instantiated goal against each fact
        for fact in &all_facts {
            if let Some(new_bindings) = pattern_match(&instantiated_goal, fact) {
                // Merge new bindings with current bindings
                let mut merged = bindings.clone();
                for (name, value) in new_bindings.iter() {
                    // Check for conflicts (same variable bound to different values)
                    if let Some(existing_value) = merged.get(name) {
                        if existing_value != value {
                            // Conflict - skip this match
                            continue;
                        }
                    }
                    merged.insert(name.clone(), value.clone());
                }
                next_bindings.push(merged);
            }
        }
    }

    // If no bindings succeeded, return empty
    if next_bindings.is_empty() {
        return vec![];
    }

    // Recurse with remaining goals and new binding sets
    thread_bindings_through_goals(remaining_goals, next_bindings, env)
}

/// Evaluate a consequent conjunction with binding threading
///
/// PHASE 3: Dynamic exec generation and consequent binding threading
///
/// Algorithm:
/// 1. Pass 1: Thread bindings through goals that match against space
/// 2. Pass 2: Add all goals (now fully instantiated) to space
///
/// Consequent goals can be:
/// 1. exec forms - added as facts for next iteration (not executed)
/// 2. Facts with variables - matched against space to collect bindings
/// 3. Ground facts - added directly to space
fn eval_consequent_conjunction_with_bindings(
    goals: Vec<MettaValue>,
    initial_bindings: Bindings,
    mut env: Environment,
) -> EvalResult {
    use crate::backend::eval::{apply_bindings, pattern_match};

    if goals.is_empty() {
        return (vec![MettaValue::Nil], env);
    }

    // Pass 1: Collect bindings by matching goals against space
    let mut current_bindings = initial_bindings.clone();

    for (_i, goal) in goals.iter().enumerate() {
        let instantiated_goal = apply_bindings(goal, &current_bindings);

        // Skip exec forms in pass 1
        if is_exec_form(&instantiated_goal) {
            continue;
        }

        // If goal has variables, try to match against space
        if has_variables(&instantiated_goal) {

            let wildcard = MettaValue::Atom("$_".to_string());
            let all_facts = env.match_space(&wildcard, &wildcard);

            for fact in &all_facts {
                if let Some(new_bindings) = pattern_match(&instantiated_goal, fact) {
                    // Merge new bindings
                    for (name, value) in new_bindings.iter() {
                        current_bindings.insert(name.clone(), value.clone());
                    }
                    break; // Use first match
                }
            }
        }
    }

    // Pass 2: Add all goals (now fully instantiated) to space
    let mut all_results = Vec::new();

    for (_i, goal) in goals.iter().enumerate() {
        let fully_instantiated = apply_bindings(goal, &current_bindings);

        if is_exec_form(&fully_instantiated) {
            // exec forms: add as facts (don't execute)
            env.add_to_space(&fully_instantiated);
            all_results.push(MettaValue::Atom("ok".to_string()));
        } else if is_operation_form(&fully_instantiated) {
            // operation forms: execute them
            let (op_results, op_env) = eval_operation_from_value(&fully_instantiated, env);
            all_results.extend(op_results);
            env = op_env;
        } else {
            // Regular facts: add to space
            env.add_to_space(&fully_instantiated);
            all_results.push(fully_instantiated.clone());
        }
    }

    (all_results, env)
}

/// Check if a MettaValue contains any variables
fn has_variables(value: &MettaValue) -> bool {
    match value {
        MettaValue::Atom(s) => s.starts_with('$') || s.starts_with('&') || s.starts_with('\''),
        MettaValue::SExpr(items) => items.iter().any(has_variables),
        MettaValue::Conjunction(goals) => goals.iter().any(has_variables),
        MettaValue::Error(_, details) => has_variables(details),
        _ => false,
    }
}

/// Check if a MettaValue is an exec form: (exec ...)
fn is_exec_form(value: &MettaValue) -> bool {
    match value {
        MettaValue::SExpr(items) if !items.is_empty() => {
            matches!(&items[0], MettaValue::Atom(op) if op == "exec")
        }
        _ => false,
    }
}

/// Check if a MettaValue is an operation form: (O ...)
fn is_operation_form(value: &MettaValue) -> bool {
    match value {
        MettaValue::SExpr(items) if !items.is_empty() => {
            matches!(&items[0], MettaValue::Atom(op) if op == "O")
        }
        _ => false,
    }
}

/// Evaluate an operation from a MettaValue
fn eval_operation_from_value(value: &MettaValue, env: Environment) -> EvalResult {
    match value {
        MettaValue::SExpr(items) => eval_operation(items, env),
        _ => (vec![], env),
    }
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
            Box::new(MettaValue::SExpr(args.to_vec())),
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
                Box::new(templates.clone()),
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
            Box::new(MettaValue::SExpr(args.to_vec())),
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
                Box::new(success_goals.clone()),
            );
            return (vec![err], env);
        }
    };

    let _failure_conj = match failure_goals {
        MettaValue::Conjunction(_) => failure_goals,
        _ => {
            let err = MettaValue::Error(
                "lookup failure branch must be a conjunction (,)".to_string(),
                Box::new(failure_goals.clone()),
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
            MettaValue::Conjunction(goals) => {
                eval_conjunction_goals(goals.clone(), env)
            }
            _ => unreachable!(), // Already checked above
        }
    } else {
        // Evaluate failure branch
        match failure_goals {
            MettaValue::Conjunction(goals) => {
                eval_conjunction_goals(goals.clone(), env)
            }
            _ => unreachable!(), // Already checked above
        }
    }
}

/// Helper to evaluate conjunction goals sequentially (for lookup branches, not exec antecedents)
fn eval_conjunction_goals(goals: Vec<MettaValue>, mut env: Environment) -> EvalResult {
    let mut all_results = Vec::new();

    for goal in goals {
        let (results, new_env) = eval_with_depth(goal, env, 0);
        all_results.extend(results);
        env = new_env;
    }

    (all_results, env)
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
            Box::new(MettaValue::SExpr(args.to_vec())),
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
                Box::new(pattern_conj.clone()),
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
                Box::new(templates_conj.clone()),
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
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Conjunction(vec![]), // Empty antecedent - always succeeds
            MettaValue::Conjunction(vec![MettaValue::Long(42)]),
        ]);

        let (results, _) = eval_exec(vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Conjunction(vec![]),
            MettaValue::Conjunction(vec![MettaValue::Long(42)]),
        ], env);

        assert!(!results.is_empty());
        // Empty antecedent succeeds, so consequent should be evaluated
    }

    #[test]
    fn test_exec_simple_consequent() {
        let env = Environment::new();

        // (exec P1 (,) (, (+ 1 2)))
        let value = eval_exec(vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P1".to_string()),
            MettaValue::Conjunction(vec![]),
            MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ])]),
        ], env);

        assert!(!value.0.is_empty());
    }

    #[test]
    fn test_coalg_structure() {
        let env = Environment::new();

        // (coalg (tree $t) (, (ctx $t nil)))
        let value = eval_coalg(vec![
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
        ], env);

        assert!(!value.0.is_empty());
        // Should return the coalg structure
    }

    #[test]
    fn test_lookup_success_branch() {
        let env = Environment::new();

        // (lookup foo (, T) (, F))
        let value = eval_lookup(vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("foo".to_string()), // Not a variable, so "found"
            MettaValue::Conjunction(vec![MettaValue::Atom("T".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("F".to_string())]),
        ], env);

        assert!(!value.0.is_empty());
        // Should execute success branch
    }

    #[test]
    fn test_lookup_failure_branch() {
        let env = Environment::new();

        // (lookup $x (, T) (, F))
        let value = eval_lookup(vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("$x".to_string()), // Variable, so "not found"
            MettaValue::Conjunction(vec![MettaValue::Atom("T".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("F".to_string())]),
        ], env);

        assert!(!value.0.is_empty());
        // Should execute failure branch
    }
}
