use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::{apply_bindings, eval, pattern_match, EvalOutput, EvalResult};

/// Evaluate if control flow: (if condition then-branch else-branch)
/// Only evaluates the chosen branch (lazy evaluation)
pub(super) fn eval_if(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    let args = &items[1..];

    if args.len() < 3 {
        let err = MettaValue::Error(
            "if requires 3 arguments: condition, then-branch, else-branch".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let condition = &args[0];
    let then_branch = &args[1];
    let else_branch = &args[2];

    // Evaluate the condition
    let (cond_results, env_after_cond) = eval(condition.clone(), env);

    // Check for error in condition
    if let Some(first) = cond_results.first() {
        if matches!(first, MettaValue::Error(_, _)) {
            return (vec![first.clone()], env_after_cond);
        }

        // Check if condition is true
        let is_true = match first {
            MettaValue::Bool(true) => true,
            MettaValue::Bool(false) => false,
            // Non-boolean values: treat as true if not Nil
            MettaValue::Nil => false,
            _ => true,
        };

        // Evaluate only the chosen branch
        if is_true {
            eval(then_branch.clone(), env_after_cond)
        } else {
            eval(else_branch.clone(), env_after_cond)
        }
    } else {
        // No result from condition - treat as false
        eval(else_branch.clone(), env_after_cond)
    }
}

/// Subsequently tests multiple pattern-matching conditions (second argument) for the
/// given value (first argument)
pub(super) fn eval_case(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!("case", items, env);

    let atom = items[1].clone();
    let cases = items[2].clone();

    let (atom_results, atom_env) = eval(atom, env);
    let mut final_results = Vec::new();

    for atom_result in atom_results {
        let is_empty = match &atom_result {
            MettaValue::Nil => true,
            MettaValue::SExpr(items) if items.is_empty() => true,
            _ => false,
        };

        if is_empty {
            let switch_result = eval_switch_minimal(
                MettaValue::Atom("Empty".to_string()),
                cases.clone(),
                atom_env.clone(),
            );
            final_results.extend(switch_result.0);
        } else {
            let switch_result = eval_switch_minimal(atom_result, cases.clone(), atom_env.clone());
            final_results.extend(switch_result.0);
        }
    }

    return (final_results, atom_env);
}

/// Difference between `switch` and `case` is a way how they interpret `Empty` result.
/// case interprets first argument inside itself and then manually checks whether result is empty.
pub(super) fn eval_switch(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!("switch", items, env);
    let atom = items[1].clone();
    let cases = items[2].clone();
    return eval_switch_minimal(atom, cases, env);
}

pub(super) fn eval_switch_minimal_handler(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!("switch-minimal", items, env);
    let atom = items[1].clone();
    let cases = items[2].clone();
    return eval_switch_minimal(atom, cases, env);
}

/// This function is being called inside switch function to test one of the cases and it
/// calls switch once again if current condition is not met
pub(super) fn eval_switch_internal_handler(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!("switch-internal", items, env);
    let atom = items[1].clone();
    let cases = items[2].clone();
    return eval_switch_internal(atom, cases, env);
}

/// Helper function to implement switch-minimal logic
/// Handles the main switch logic by deconstructing cases and calling switch-internal
fn eval_switch_minimal(atom: MettaValue, cases: MettaValue, env: Environment) -> EvalResult {
    if let MettaValue::SExpr(cases_items) = cases {
        if cases_items.is_empty() {
            return (vec![MettaValue::Atom("NotReducible".to_string())], env);
        }

        let first_case = cases_items[0].clone();
        let remaining_cases = if cases_items.len() > 1 {
            MettaValue::SExpr(cases_items[1..].to_vec())
        } else {
            MettaValue::SExpr(vec![])
        };

        let cases_list = MettaValue::SExpr(vec![first_case, remaining_cases]);
        return eval_switch_internal(atom, cases_list, env);
    }

    let err = MettaValue::Error(
        "switch-minimal expects expression as second argument".to_string(),
        Box::new(cases),
    );
    (vec![err], env)
}

/// Helper function to implement switch-internal logic
/// Tests one case and recursively tries remaining cases if no match
fn eval_switch_internal(atom: MettaValue, cases_data: MettaValue, env: Environment) -> EvalResult {
    if let MettaValue::SExpr(cases_items) = cases_data {
        if cases_items.len() != 2 {
            let err = MettaValue::Error(
                "switch-internal expects exactly 2 arguments".to_string(),
                Box::new(MettaValue::SExpr(cases_items)),
            );
            return (vec![err], env);
        }

        let first_case = cases_items[0].clone();
        let remaining_cases = cases_items[1].clone();

        if let MettaValue::SExpr(case_items) = first_case {
            if case_items.len() != 2 {
                let err = MettaValue::Error(
                    "switch case should be a pattern-template pair".to_string(),
                    Box::new(MettaValue::SExpr(case_items)),
                );
                return (vec![err], env);
            }

            let pattern = case_items[0].clone();
            let template = case_items[1].clone();

            if let Some(bindings) = pattern_match(&pattern, &atom) {
                let instantiated_template = apply_bindings(&template, &bindings);
                return eval(instantiated_template, env);
            } else {
                return eval_switch_minimal(atom, remaining_cases, env);
            }
        } else {
            let err = MettaValue::Error(
                "switch case should be an expression".to_string(),
                Box::new(first_case),
            );
            return (vec![err], env);
        }
    }

    let err = MettaValue::Error(
        "switch-internal expects expression argument".to_string(),
        Box::new(cases_data),
    );
    (vec![err], env)
}

// TODO -> tests
