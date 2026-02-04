//! Generic MORK Special Forms - Zero-Conversion Implementation
//!
//! This module provides generic versions of MORK special forms that work with any
//! value type implementing `MettaValueTrait`. This eliminates ArenaValue <-> MettaValue
//! conversions when using arena allocation.
//!
//! ## Zero-Conversion Path
//!
//! For ArenaValue:
//! - No conversion to MettaValue for MORK operations
//! - Pattern matching uses generic `pattern_match_generic`
//! - Binding application uses generic `apply_bindings_generic`
//! - Space operations use GenericEnvironment methods directly
//!
//! ## Forms
//!
//! - `eval_exec_generic`: Rule execution with conjunction antecedents/consequents
//! - `eval_coalg_generic`: Coalgebra patterns for tree transformations
//! - `eval_lookup_generic`: Conditional fact lookup with success/failure branches
//! - `eval_rulify_generic`: Meta-programming for runtime rule generation

use crate::backend::environment::GenericEnvironment;
use crate::backend::eval::trampoline::{apply_bindings_generic, pattern_match_generic};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

/// Generic result type for MORK operations
pub type GenericMorkResult<V, F> = (Vec<V>, GenericEnvironment<V, F>);

// ============================================================================
// Generic Helper Functions
// ============================================================================

/// Check if a generic value contains any variables.
///
/// Uses `MettaValueTrait` methods for zero-conversion checking.
pub fn has_variables_generic<V: MettaValueTrait>(value: &V) -> bool {
    // Check if it's a variable atom
    if let Some(name) = value.as_atom() {
        return name.starts_with('$') || name.starts_with('&') || name.starts_with('\'');
    }

    // Check S-expression children
    if let Some(items) = value.as_sexpr() {
        return items.iter().any(has_variables_generic);
    }

    // Check conjunction goals
    if let Some(goals) = value.as_conjunction() {
        return goals.iter().any(has_variables_generic);
    }

    // Check error details
    if let Some((_, details)) = value.as_error() {
        return has_variables_generic(details);
    }

    false
}

/// Check if a generic value is an exec form: (exec ...)
pub fn is_exec_form_generic<V: MettaValueTrait>(value: &V) -> bool {
    if let Some(items) = value.as_sexpr() {
        if !items.is_empty() {
            if let Some(op) = items[0].as_atom() {
                return op == "exec";
            }
        }
    }
    false
}

/// Check if a generic value is an operation form: (O ...)
pub fn is_operation_form_generic<V: MettaValueTrait>(value: &V) -> bool {
    if let Some(items) = value.as_sexpr() {
        if !items.is_empty() {
            if let Some(op) = items[0].as_atom() {
                return op == "O";
            }
        }
    }
    false
}

/// Check if items represent an operation (starts with "O")
fn matches_operation_generic<V: MettaValueTrait>(items: &[V]) -> bool {
    if let Some(first) = items.first() {
        if let Some(op) = first.as_atom() {
            return op == "O";
        }
    }
    false
}

/// Extract conjunction goals from a value.
/// Handles both Conjunction variant and SExpr representation (, goal1 goal2 ...)
fn extract_conjunction_goals<V: MettaValueTrait + Clone>(value: &V) -> Option<Vec<V>> {
    // Try Conjunction variant first
    if let Some(goals) = value.as_conjunction() {
        return Some(goals.to_vec());
    }

    // Try SExpr representation: (, goal1 goal2 ...)
    if let Some(items) = value.as_sexpr() {
        if !items.is_empty() {
            if let Some(op) = items[0].as_atom() {
                if op == "," {
                    return Some(items[1..].to_vec());
                }
            }
        }
    }

    None
}

// ============================================================================
// Generic MORK Forms
// ============================================================================

/// Generic eval_exec: (exec <priority> <antecedent> <consequent>)
///
/// Executes rules with conjunction-based pattern matching using generic types.
/// No ArenaValue <-> MettaValue conversion required.
pub fn eval_exec_generic<V, F>(
    items: Vec<V>,
    mut env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    let args = &items[1..]; // Skip "exec" operator

    if args.len() < 3 {
        let err = factory.error(
            "exec requires 3 arguments: priority, antecedent, and consequent",
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let _priority = &args[0]; // Priority for future use
    let antecedent = &args[1];
    let consequent = &args[2];

    // Store exec as a fact for dynamic exec generation
    let exec_fact = factory.sexpr(items.clone());
    env.add_to_space(&exec_fact);

    // Extract antecedent goals
    let antecedent_goals = match extract_conjunction_goals(antecedent) {
        Some(goals) => goals,
        None => {
            let err = factory.error("exec antecedent must be a conjunction (,)", antecedent.clone());
            return (vec![err], env);
        }
    };

    // Evaluate antecedent conjunction to get bindings
    let binding_sets = match_conjunction_goals_generic(&antecedent_goals, &env, factory);

    // If antecedent failed, rule doesn't fire
    if binding_sets.is_empty() {
        return (vec![], env);
    }

    // For each binding set, evaluate consequent
    let mut all_results = Vec::new();
    let mut final_env = env;

    for bindings in binding_sets {
        // Apply bindings to consequent
        let instantiated_consequent = apply_bindings_generic(consequent, &bindings, factory);

        // Check if consequent is a conjunction
        if let Some(goals) = extract_conjunction_goals(&instantiated_consequent) {
            let (conseq_results, conseq_env) = eval_consequent_conjunction_generic(
                goals,
                bindings.clone(),
                final_env.clone(),
                factory,
            );
            all_results.extend(conseq_results);
            final_env = conseq_env;
        } else if let Some(items) = instantiated_consequent.as_sexpr() {
            if matches_operation_generic(items) {
                // Handle operation: (O (+ fact) (- fact) ...)
                let (op_results, op_env) =
                    eval_operation_generic(items, final_env.clone(), factory);
                all_results.extend(op_results);
                final_env = op_env;
            } else {
                let err = factory.error(
                    "exec consequent must be a conjunction or operation (O ...)",
                    instantiated_consequent.clone(),
                );
                all_results.push(err);
            }
        } else {
            let err = factory.error(
                "exec consequent must be a conjunction or operation (O ...)",
                instantiated_consequent.clone(),
            );
            all_results.push(err);
        }
    }

    (all_results, final_env)
}

/// Match conjunction goals with binding threading (generic version).
fn match_conjunction_goals_generic<V, F>(
    goals: &[V],
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Vec<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    if goals.is_empty() {
        return vec![GenericBindings::new()];
    }

    let initial_bindings = vec![GenericBindings::new()];
    thread_bindings_through_goals_generic(goals, initial_bindings, env, factory)
}

/// Thread bindings through conjunction goals (generic version).
fn thread_bindings_through_goals_generic<V, F>(
    goals: &[V],
    current_bindings: Vec<GenericBindings<V>>,
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Vec<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    if goals.is_empty() {
        return current_bindings;
    }

    let goal = &goals[0];
    let remaining_goals = &goals[1..];

    let mut next_bindings = Vec::new();

    for bindings in current_bindings {
        // Apply current bindings to goal
        let instantiated_goal = apply_bindings_generic(goal, &bindings, factory);

        // Get all facts from space using generic method
        let wildcard = factory.atom("$_");
        let all_facts = env.match_space(&wildcard, &wildcard);

        // Try to match against each fact
        for match_result in all_facts.iter() {
            if let Some(new_bindings) = pattern_match_generic(&instantiated_goal, &match_result.value) {
                // Merge bindings
                let mut merged = bindings.clone();
                let mut conflict = false;

                for (name, value) in new_bindings.iter() {
                    if let Some(existing) = merged.get(name) {
                        // Check for conflict using friendly_repr comparison
                        if existing.friendly_repr() != value.friendly_repr() {
                            conflict = true;
                            break;
                        }
                    } else {
                        merged.insert(name.clone(), value.clone());
                    }
                }

                if !conflict {
                    next_bindings.push(merged);
                }
            }
        }
    }

    if next_bindings.is_empty() {
        return vec![];
    }

    thread_bindings_through_goals_generic(remaining_goals, next_bindings, env, factory)
}

/// Evaluate consequent conjunction with binding threading (generic version).
///
/// ## CoW-Safe Implementation
///
/// Uses `add_to_space_shared()` for interior mutability on the shared state,
/// avoiding CoW deep copies that would cause state loss when the environment
/// is cloned in loops.
fn eval_consequent_conjunction_generic<V, F>(
    goals: Vec<V>,
    initial_bindings: GenericBindings<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    if goals.is_empty() {
        return (vec![factory.nil()], env);
    }

    // Pass 1: Collect bindings by matching goals against space
    let mut current_bindings = initial_bindings.clone();

    for goal in goals.iter() {
        let instantiated_goal = apply_bindings_generic(goal, &current_bindings, factory);

        // Skip exec forms in pass 1
        if is_exec_form_generic(&instantiated_goal) {
            continue;
        }

        // If goal has variables, try to match against space
        if has_variables_generic(&instantiated_goal) {
            let matches = env.match_space(&instantiated_goal, &instantiated_goal);
            if let Some(first_match) = matches.first() {
                if let Some(new_bindings) = pattern_match_generic(&instantiated_goal, &first_match.value) {
                    for (name, value) in new_bindings.iter() {
                        current_bindings.insert(name.clone(), value.clone());
                    }
                }
            }
        }
    }

    // Pass 2: Add all goals to space using interior mutability
    // Note: We use add_to_space_shared() to mutate the shared Arc state directly,
    // avoiding CoW copies that would cause state loss in cloned environments.
    let mut all_results = Vec::new();

    for goal in goals.iter() {
        let fully_instantiated = apply_bindings_generic(goal, &current_bindings, factory);

        if is_exec_form_generic(&fully_instantiated) {
            // Use interior mutability - no CoW copy
            env.add_to_space_shared(&fully_instantiated);
            all_results.push(factory.atom("ok"));
        } else if is_operation_form_generic(&fully_instantiated) {
            if let Some(items) = fully_instantiated.as_sexpr() {
                // eval_operation_generic also uses shared methods now
                let (op_results, _) = eval_operation_generic_shared(items, &env, factory);
                all_results.extend(op_results);
            }
        } else {
            // Use interior mutability - no CoW copy
            env.add_to_space_shared(&fully_instantiated);
            all_results.push(fully_instantiated.clone());
        }
    }

    (all_results, env)
}

/// Evaluate operation (O (+ fact) (- fact) ...) (generic version).
fn eval_operation_generic<V, F>(
    items: &[V],
    mut env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let operations = &items[1..]; // Skip "O" operator

    for op in operations {
        if let Some(op_items) = op.as_sexpr() {
            if op_items.len() == 2 {
                if let Some(op_type) = op_items[0].as_atom() {
                    let fact = &op_items[1];
                    match op_type {
                        "+" => {
                            env.add_to_space(fact);
                        }
                        "-" => {
                            env.remove_from_space(fact);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    (vec![factory.atom("ok")], env)
}

/// Evaluate operation using interior mutability (CoW-safe version).
///
/// Uses `add_to_space_shared()` and `remove_from_space_shared()` to avoid
/// triggering CoW deep copies when the environment is cloned.
fn eval_operation_generic_shared<V, F>(
    items: &[V],
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> (Vec<V>, ())
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let operations = &items[1..]; // Skip "O" operator

    for op in operations {
        if let Some(op_items) = op.as_sexpr() {
            if op_items.len() == 2 {
                if let Some(op_type) = op_items[0].as_atom() {
                    let fact = &op_items[1];
                    match op_type {
                        "+" => {
                            // Use interior mutability - no CoW copy
                            env.add_to_space_shared(fact);
                        }
                        "-" => {
                            // Use interior mutability - no CoW copy
                            env.remove_from_space_shared(fact);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    (vec![factory.atom("ok")], ())
}

/// Generic eval_coalg: (coalg <pattern> <templates>)
///
/// Coalgebra patterns for tree transformations using generic types.
pub fn eval_coalg_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let args = &items[1..]; // Skip "coalg" operator

    if args.len() < 2 {
        let err = factory.error(
            "coalg requires 2 arguments: pattern and templates",
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let templates = &args[1];

    // Templates must be a conjunction
    let template_list = match templates.as_conjunction() {
        Some(temps) => temps.to_vec(),
        None => {
            let err = factory.error("coalg templates must be a conjunction (,)", templates.clone());
            return (vec![err], env);
        }
    };

    // Return coalg structure as-is (placeholder implementation)
    let coalg_expr = factory.sexpr(vec![
        factory.atom("coalg"),
        pattern.clone(),
        factory.conjunction(template_list),
    ]);

    (vec![coalg_expr], env)
}

/// Generic eval_lookup: (lookup <pattern> <success-goals> <failure-goals>)
///
/// Conditional execution based on space queries using generic types.
pub fn eval_lookup_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    let args = &items[1..]; // Skip "lookup" operator

    if args.len() < 3 {
        let err = factory.error(
            "lookup requires 3 arguments: pattern, success-goals, and failure-goals",
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let success_goals = &args[1];
    let failure_goals = &args[2];

    // Validate branches are conjunctions
    if success_goals.as_conjunction().is_none() {
        let err = factory.error(
            "lookup success branch must be a conjunction (,)",
            success_goals.clone(),
        );
        return (vec![err], env);
    }

    if failure_goals.as_conjunction().is_none() {
        let err = factory.error(
            "lookup failure branch must be a conjunction (,)",
            failure_goals.clone(),
        );
        return (vec![err], env);
    }

    // Check if pattern is a variable (not found) vs non-variable (found)
    let pattern_found = if let Some(name) = pattern.as_atom() {
        !name.starts_with('$')
    } else {
        true
    };

    if pattern_found {
        // Evaluate success branch
        if let Some(goals) = success_goals.as_conjunction() {
            eval_conjunction_goals_generic(goals.to_vec(), env, factory)
        } else {
            (vec![], env)
        }
    } else {
        // Evaluate failure branch
        if let Some(goals) = failure_goals.as_conjunction() {
            eval_conjunction_goals_generic(goals.to_vec(), env, factory)
        } else {
            (vec![], env)
        }
    }
}

/// Evaluate conjunction goals sequentially (generic version).
///
/// FULLY GENERIC - NO CONVERSIONS REQUIRED.
///
/// For MORK semantics, conjunction goals don't need full MeTTa evaluation.
/// They need:
/// 1. Pattern matching against space
/// 2. Adding/removing facts from space
/// 3. Executing operations (O ...)
///
/// This enables zero-conversion evaluation for both heap and arena modes.
fn eval_conjunction_goals_generic<V, F>(
    goals: Vec<V>,
    mut env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    let mut all_results = Vec::new();

    for goal in goals {
        // Handle different goal types using MORK semantics (no full eval needed)

        // Check if goal is an exec form - add to space
        if is_exec_form_generic(&goal) {
            env.add_to_space(&goal);
            all_results.push(factory.atom("ok"));
            continue;
        }

        // Check if goal is an operation form - execute it
        if is_operation_form_generic(&goal) {
            if let Some(items) = goal.as_sexpr() {
                let (op_results, op_env) = eval_operation_generic(items, env, factory);
                all_results.extend(op_results);
                env = op_env;
            }
            continue;
        }

        // Check if goal is a lookup - evaluate it recursively
        if let Some(items) = goal.as_sexpr() {
            if !items.is_empty() {
                if let Some(op) = items[0].as_atom() {
                    if op == "lookup" {
                        let (lookup_results, lookup_env) =
                            eval_lookup_generic(items.to_vec(), env, factory);
                        all_results.extend(lookup_results);
                        env = lookup_env;
                        continue;
                    }
                    // Handle nested exec in goals
                    if op == "exec" {
                        let (exec_results, exec_env) =
                            eval_exec_generic(items.to_vec(), env, factory);
                        all_results.extend(exec_results);
                        env = exec_env;
                        continue;
                    }
                    // Handle nested coalg in goals
                    if op == "coalg" {
                        let (coalg_results, coalg_env) =
                            eval_coalg_generic(items.to_vec(), env, factory);
                        all_results.extend(coalg_results);
                        env = coalg_env;
                        continue;
                    }
                }
            }
        }

        // For other goals: If it has variables, try to match against space
        // Otherwise, add it to space as a fact
        if has_variables_generic(&goal) {
            // Try to match against space and return the matches
            let matches = env.match_space(&goal, &goal);
            if !matches.is_empty() {
                // Return first match
                all_results.push(matches[0].value.clone());
            } else {
                // No match found - return the goal itself
                all_results.push(goal.clone());
            }
        } else {
            // Ground fact - add to space and return it
            env.add_to_space(&goal);
            all_results.push(goal.clone());
        }
    }

    (all_results, env)
}

/// Generic eval_rulify: (rulify $name (, $p0) (, $t0 ...) <antecedent> <consequent>)
///
/// Generates exec rules from coalgebra definitions using generic types.
pub fn eval_rulify_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let args = &items[1..]; // Skip "rulify" operator

    if args.len() < 5 {
        let err = factory.error(
            "rulify requires 5 arguments: name, pattern, templates, antecedent, consequent",
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let name = &args[0];
    let pattern_conj = &args[1];
    let templates_conj = &args[2];
    let rule_antecedent = &args[3];
    let rule_consequent = &args[4];

    // Extract pattern from unary conjunction
    let pattern = match pattern_conj.as_conjunction() {
        Some(ps) if ps.len() == 1 => ps[0].clone(),
        _ => {
            let err = factory.error(
                "rulify pattern must be a unary conjunction (, $p0)",
                pattern_conj.clone(),
            );
            return (vec![err], env);
        }
    };

    // Extract templates from conjunction
    let templates = match templates_conj.as_conjunction() {
        Some(ts) => ts.to_vec(),
        None => {
            let err = factory.error(
                "rulify templates must be a conjunction (, $t0 ...)",
                templates_conj.clone(),
            );
            return (vec![err], env);
        }
    };

    // Create meta-rule structure
    let meta_rule = factory.sexpr(vec![
        factory.atom("meta-rule"),
        name.clone(),
        pattern,
        factory.conjunction(templates),
        rule_antecedent.clone(),
        rule_consequent.clone(),
    ]);

    (vec![meta_rule], env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::HeapEnvironment;
    use crate::backend::models::{HeapMettaValueFactory, MettaValue};

    #[test]
    fn test_has_variables_generic() {
        let var = MettaValue::Atom("$x".to_string());
        assert!(has_variables_generic(&var));

        let atom = MettaValue::Atom("foo".to_string());
        assert!(!has_variables_generic(&atom));

        let sexpr_with_var = MettaValue::SExpr(vec![
            MettaValue::Atom("f".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        assert!(has_variables_generic(&sexpr_with_var));
    }

    #[test]
    fn test_is_exec_form_generic() {
        let exec = MettaValue::SExpr(vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
        ]);
        assert!(is_exec_form_generic(&exec));

        let not_exec = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);
        assert!(!is_exec_form_generic(&not_exec));
    }

    #[test]
    fn test_eval_exec_generic_empty_antecedent() {
        let env = HeapEnvironment::new(HeapMettaValueFactory);
        let factory = HeapMettaValueFactory;

        let items = vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Conjunction(vec![]), // Empty antecedent
            MettaValue::Conjunction(vec![MettaValue::Long(42)]),
        ];

        let (results, _) = eval_exec_generic(items, env, &factory);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_eval_coalg_generic() {
        let env = HeapEnvironment::new(HeapMettaValueFactory);
        let factory = HeapMettaValueFactory;

        let items = vec![
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
        ];

        let (results, _) = eval_coalg_generic(items, env, &factory);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_eval_lookup_generic_success() {
        let env = HeapEnvironment::new(HeapMettaValueFactory);
        let factory = HeapMettaValueFactory;

        let items = vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("foo".to_string()), // Not a variable
            MettaValue::Conjunction(vec![MettaValue::Atom("T".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("F".to_string())]),
        ];

        let (results, _) = eval_lookup_generic(items, env, &factory);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_eval_rulify_generic() {
        let env = HeapEnvironment::new(HeapMettaValueFactory);
        let factory = HeapMettaValueFactory;

        let items = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("test_rule".to_string()),
            MettaValue::Conjunction(vec![MettaValue::Atom("$p0".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("$t0".to_string())]),
            MettaValue::Conjunction(vec![]),
            MettaValue::Atom("consequent".to_string()),
        ];

        let (results, _) = eval_rulify_generic(items, env, &factory);
        assert!(!results.is_empty());
    }
}
