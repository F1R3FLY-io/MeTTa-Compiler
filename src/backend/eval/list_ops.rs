use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::eval;

/// Suggest variable format when user provides a plain atom instead of `$var`
/// Returns a suggestion string if the atom looks like it should be a variable
fn suggest_variable_format(atom: &str) -> Option<String> {
    // If it's already a variable, no suggestion needed
    if atom.starts_with('$') || atom.starts_with('&') || atom.starts_with('\'') {
        return None;
    }

    // Don't suggest for obvious non-variables (operators, keywords, etc.)
    if atom.contains('(') || atom.contains(')') || atom.is_empty() {
        return None;
    }

    // Short, lowercase identifiers are likely intended as variables
    let first_char = atom.chars().next()?;
    if first_char.is_lowercase() && atom.len() <= 10 {
        Some(format!(
            "Did you mean: ${}? (variables must start with $)",
            atom
        ))
    } else {
        None
    }
}

/// Map atom: (map-atom $list $var $template)
/// Maps a function over a list of atoms
/// Example: (map-atom (1 2 3 4) $v (+ $v 1)) -> (2 3 4 5)
pub(super) fn eval_map_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_three_args!("map-atom", items, env);

    let list = &items[1];
    let var = &items[2];
    let template = &items[3];

    let var_name = match var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "map-atom: second argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "map-atom: second argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Box::new(var.clone()));
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "map-atom: second argument must be a variable (starting with $)".to_string(),
                Box::new(var.clone()),
            );
            return (vec![err], env);
        }
    };

    let elements = match list {
        MettaValue::SExpr(items) => items.clone(),
        MettaValue::Nil => vec![],
        _ => {
            let err = MettaValue::Error(
                format!(
                    "map-atom: first argument must be a list, got {}. Usage: (map-atom list $var expr)",
                    super::friendly_value_repr(list)
                ),
                Box::new(list.clone()),
            );
            return (vec![err], env);
        }
    };

    let mut mapped_elements = Vec::new();
    let mut final_env = env;

    for element in elements {
        let instantiated_template = substitute_variable(template, &var_name, &element);
        let (results, new_env) = eval(instantiated_template, final_env);
        final_env = new_env;

        if let Some(first_result) = results.first() {
            if matches!(first_result, MettaValue::Error(_, _)) {
                return (vec![first_result.clone()], final_env);
            }
            mapped_elements.push(first_result.clone());
        } else {
            mapped_elements.push(MettaValue::Nil);
        }
    }

    let result = if mapped_elements.is_empty() {
        MettaValue::Nil
    } else {
        MettaValue::SExpr(mapped_elements)
    };

    (vec![result], final_env)
}

/// Filter atom: (filter-atom $list $var $predicate)
/// Filters a list keeping only elements that satisfy the predicate
/// Example: (filter-atom (1 2 3 4) $v (> $v 2)) -> (3 4)
pub(super) fn eval_filter_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_three_args!("filter-atom", items, env);

    let list = &items[1];
    let var = &items[2];
    let predicate = &items[3];

    let var_name = match var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "filter-atom: second argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "filter-atom: second argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Box::new(var.clone()));
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "filter-atom: second argument must be a variable (starting with $)".to_string(),
                Box::new(var.clone()),
            );
            return (vec![err], env);
        }
    };

    let elements = match list {
        MettaValue::SExpr(items) => items.clone(),
        MettaValue::Nil => vec![],
        _ => {
            let err = MettaValue::Error(
                format!(
                    "filter-atom: first argument must be a list, got {}. Usage: (filter-atom list $var predicate)",
                    super::friendly_value_repr(list)
                ),
                Box::new(list.clone()),
            );
            return (vec![err], env);
        }
    };

    let mut filtered_elements = Vec::new();
    let mut final_env = env;

    for element in elements {
        let instantiated_predicate = substitute_variable(predicate, &var_name, &element);

        let (results, new_env) = eval(instantiated_predicate, final_env);
        final_env = new_env;

        if let Some(first_result) = results.first() {
            if matches!(first_result, MettaValue::Error(_, _)) {
                return (vec![first_result.clone()], final_env);
            }

            let should_include = match first_result {
                MettaValue::Bool(true) => true,
                MettaValue::Bool(false) => false,
                _ => !matches!(first_result, MettaValue::Nil),
            };

            if should_include {
                filtered_elements.push(element);
            }
        }
    }

    let result = if filtered_elements.is_empty() {
        MettaValue::Nil
    } else {
        MettaValue::SExpr(filtered_elements)
    };

    (vec![result], final_env)
}

/// Fold left atom: (foldl-atom $list $init $acc $item $op)
/// Folds (reduces) a list from left to right using an operation and initial value
/// Example: (foldl-atom (1 2 3) 0 $acc $x (+ $acc $x)) -> 6
pub(super) fn eval_foldl_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    if items.len() != 6 {
        let err = MettaValue::Error(
            "foldl-atom requires exactly 5 arguments: list, init, acc-var, item-var, operation"
                .to_string(),
            Box::new(MettaValue::SExpr(items.to_vec())),
        );
        return (vec![err], env);
    }

    let list = &items[1];
    let init = &items[2];
    let acc_var = &items[3];
    let item_var = &items[4];
    let operation = &items[5];

    let acc_var_name = match acc_var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "foldl-atom: third argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "foldl-atom: third argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Box::new(acc_var.clone()));
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "foldl-atom: third argument must be a variable (starting with $)".to_string(),
                Box::new(acc_var.clone()),
            );
            return (vec![err], env);
        }
    };

    let item_var_name = match item_var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "foldl-atom: fourth argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "foldl-atom: fourth argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Box::new(item_var.clone()));
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "foldl-atom: fourth argument must be a variable (starting with $)".to_string(),
                Box::new(item_var.clone()),
            );
            return (vec![err], env);
        }
    };

    let elements = match list {
        MettaValue::SExpr(items) => items.clone(),
        MettaValue::Nil => vec![],
        _ => {
            let err = MettaValue::Error(
                format!(
                    "foldl-atom: first argument must be a list, got {}. Usage: (foldl-atom list init $acc $elem expr)",
                    super::friendly_value_repr(list)
                ),
                Box::new(list.clone()),
            );
            return (vec![err], env);
        }
    };

    let mut accumulator = init.clone();
    let mut final_env = env;

    for element in elements {
        let mut instantiated_op = substitute_variable(operation, &acc_var_name, &accumulator);
        instantiated_op = substitute_variable(&instantiated_op, &item_var_name, &element);

        let (results, new_env) = eval(instantiated_op, final_env);
        final_env = new_env;

        if let Some(first_result) = results.first() {
            if matches!(first_result, MettaValue::Error(_, _)) {
                return (vec![first_result.clone()], final_env);
            }
            accumulator = first_result.clone();
        }
    }

    (vec![accumulator], final_env)
}

/// Substitute a variable in an expression with a value
/// This is a simplified version of atom-subst
fn substitute_variable(expr: &MettaValue, var_name: &str, value: &MettaValue) -> MettaValue {
    match expr {
        MettaValue::Atom(name) if name == var_name => value.clone(),
        MettaValue::SExpr(items) => {
            let substituted_items: Vec<MettaValue> = items
                .iter()
                .map(|item| substitute_variable(item, var_name, value))
                .collect();
            MettaValue::SExpr(substituted_items)
        }
        MettaValue::Error(msg, details) => {
            let substituted_details = substitute_variable(details, var_name, value);
            MettaValue::Error(msg.clone(), Box::new(substituted_details))
        }
        _ => expr.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::Environment;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_map_atom_simple() {
        let env = Environment::new();

        // (map-atom (1 2 3) $v (+ $v 1))
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$v".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(mapped) => {
                assert_eq!(mapped.len(), 3);
                assert_eq!(mapped[0], MettaValue::Long(2));
                assert_eq!(mapped[1], MettaValue::Long(3));
                assert_eq!(mapped[2], MettaValue::Long(4));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_map_atom_empty_list() {
        let env = Environment::new();

        // (map-atom () $v (+ $v 1))
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![]),
            MettaValue::Atom("$v".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_map_atom_invalid_variable() {
        let env = Environment::new();

        // (map-atom (1 2 3) invalid-var (+ $v 1))
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("invalid-var".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_substitute_variable() {
        let template = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$v".to_string()),
            MettaValue::Long(1),
        ]);

        let result = substitute_variable(&template, "$v", &MettaValue::Long(5));

        match result {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::Atom("+".to_string()));
                assert_eq!(items[1], MettaValue::Long(5));
                assert_eq!(items[2], MettaValue::Long(1));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_substitute_variable_nested() {
        let template = MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(1),
            ]),
            MettaValue::Long(2),
        ]);

        let result = substitute_variable(&template, "$v", &MettaValue::Long(3));

        match result {
            MettaValue::SExpr(outer) => {
                assert_eq!(outer.len(), 3);
                assert_eq!(outer[0], MettaValue::Atom("*".to_string()));
                match &outer[1] {
                    MettaValue::SExpr(inner) => {
                        assert_eq!(inner[1], MettaValue::Long(3)); // $v substituted
                    }
                    _ => panic!("Expected nested S-expression"),
                }
                assert_eq!(outer[2], MettaValue::Long(2));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    // === Filter Tests ===

    #[test]
    fn test_filter_atom_simple() {
        let env = Environment::new();

        // (filter-atom (1 2 3 4) $v (> $v 2))
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
            MettaValue::Atom("$v".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(2),
            ]),
        ];

        let (results, _) = eval_filter_atom(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(filtered) => {
                assert_eq!(filtered.len(), 2);
                assert_eq!(filtered[0], MettaValue::Long(3));
                assert_eq!(filtered[1], MettaValue::Long(4));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_filter_atom_all_filtered_out() {
        let env = Environment::new();

        // (filter-atom (1 2) $v (> $v 5))
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]),
            MettaValue::Atom("$v".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(5),
            ]),
        ];

        let (results, _) = eval_filter_atom(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_filter_atom_empty_list() {
        let env = Environment::new();

        // (filter-atom () $v (> $v 2))
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![]),
            MettaValue::Atom("$v".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(2),
            ]),
        ];

        let (results, _) = eval_filter_atom(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    // === Fold Tests ===

    #[test]
    fn test_foldl_atom_sum() {
        let env = Environment::new();

        // (foldl-atom (1 2 3 4) 0 $acc $x (+ $acc $x))
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
            MettaValue::Long(0), // initial value
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10)); // 0 + 1 + 2 + 3 + 4 = 10
    }

    #[test]
    fn test_foldl_atom_product() {
        let env = Environment::new();

        // (foldl-atom (2 3 4) 1 $acc $x (* $acc $x))
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(2),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
            MettaValue::Long(1), // initial value
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(24)); // 1 * 2 * 3 * 4 = 24
    }

    #[test]
    fn test_foldl_atom_empty_list() {
        let env = Environment::new();

        // (foldl-atom () 42 $acc $x (+ $acc $x))
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![]),
            MettaValue::Long(42), // initial value
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42)); // Should return initial value
    }

    #[test]
    fn test_foldl_atom_wrong_arity() {
        let env = Environment::new();

        // (foldl-atom (1 2 3) 0) - missing arguments
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Long(0),
        ];

        let (results, _) = eval_foldl_atom(items, env);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    // === Integration Tests ===

    #[test]
    fn test_map_filter_compose() {
        let env = Environment::new();

        // First map: (map-atom (1 2 3 4) $v (* $v 2)) -> (2 4 6 8)
        let map_items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
            MettaValue::Atom("$v".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(2),
            ]),
        ];

        let (map_results, env1) = eval_map_atom(map_items, env);
        assert_eq!(map_results.len(), 1);

        // Then filter: (filter-atom (2 4 6 8) $v (> $v 4)) -> (6 8)
        let filter_items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            map_results[0].clone(), // Use result from map
            MettaValue::Atom("$v".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$v".to_string()),
                MettaValue::Long(4),
            ]),
        ];

        let (filter_results, _) = eval_filter_atom(filter_items, env1);
        assert_eq!(filter_results.len(), 1);

        match &filter_results[0] {
            MettaValue::SExpr(filtered) => {
                assert_eq!(filtered.len(), 2);
                assert_eq!(filtered[0], MettaValue::Long(6));
                assert_eq!(filtered[1], MettaValue::Long(8));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    // === Comprehensive Map-Atom Tests ===

    #[test]
    fn test_map_atom_identity_function() {
        let env = Environment::new();

        // (map-atom (1 2 3) $x $x) - identity function
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(mapped) => {
                assert_eq!(mapped.len(), 3);
                assert_eq!(mapped[0], MettaValue::Long(1));
                assert_eq!(mapped[1], MettaValue::Long(2));
                assert_eq!(mapped[2], MettaValue::Long(3));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_map_atom_constant_function() {
        let env = Environment::new();

        // (map-atom (a b c) $x 42) - constant function
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(42),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(mapped) => {
                assert_eq!(mapped.len(), 3);
                assert_eq!(mapped[0], MettaValue::Long(42));
                assert_eq!(mapped[1], MettaValue::Long(42));
                assert_eq!(mapped[2], MettaValue::Long(42));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_map_atom_wrong_arity() {
        let env = Environment::new();

        // Test with too few arguments
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(1)]),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_map_atom_non_list_input() {
        let env = Environment::new();

        // (map-atom 42 $x (+ $x 1)) - non-list as first argument
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_map_atom_nil_input() {
        let env = Environment::new();

        // (map-atom nil $x (+ $x 1))
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::Nil,
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_map_atom_mixed_types() {
        let env = Environment::new();

        // (map-atom (1 "hello" true) $x $x) - mixed type list
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::String("hello".to_string()),
                MettaValue::Bool(true),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(mapped) => {
                assert_eq!(mapped.len(), 3);
                assert_eq!(mapped[0], MettaValue::Long(1));
                assert_eq!(mapped[1], MettaValue::String("hello".to_string()));
                assert_eq!(mapped[2], MettaValue::Bool(true));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_map_atom_nested_lists() {
        let env = Environment::new();

        // (map-atom ((1 2) (3 4)) $pair $pair) - nested S-expressions
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]),
                MettaValue::SExpr(vec![MettaValue::Long(3), MettaValue::Long(4)]),
            ]),
            MettaValue::Atom("$pair".to_string()),
            MettaValue::Atom("$pair".to_string()),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(mapped) => {
                assert_eq!(mapped.len(), 2);
                assert!(matches!(mapped[0], MettaValue::SExpr(_)));
                assert!(matches!(mapped[1], MettaValue::SExpr(_)));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    // === Comprehensive Filter-Atom Tests ===

    #[test]
    fn test_filter_atom_boolean_predicate() {
        let env = Environment::new();

        // (filter-atom (true false true false) $b $b) - filter by boolean value
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Bool(true),
                MettaValue::Bool(false),
                MettaValue::Bool(true),
                MettaValue::Bool(false),
            ]),
            MettaValue::Atom("$b".to_string()),
            MettaValue::Atom("$b".to_string()),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(filtered) => {
                assert_eq!(filtered.len(), 2);
                assert_eq!(filtered[0], MettaValue::Bool(true));
                assert_eq!(filtered[1], MettaValue::Bool(true));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_filter_atom_truthy_values() {
        let env = Environment::new();

        // Test truthy/falsy behavior with non-boolean values
        // (filter-atom (42 nil "hello" 0) $x $x)
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(42),
                MettaValue::Nil,
                MettaValue::String("hello".to_string()),
                MettaValue::Long(0),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(filtered) => {
                // Should filter out only nil (falsy), keep everything else (truthy)
                assert_eq!(filtered.len(), 3);
                assert_eq!(filtered[0], MettaValue::Long(42));
                assert_eq!(filtered[1], MettaValue::String("hello".to_string()));
                assert_eq!(filtered[2], MettaValue::Long(0));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_filter_atom_wrong_arity() {
        let env = Environment::new();

        // Test with wrong number of arguments
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(1)]),
            MettaValue::Atom("$x".to_string()),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_filter_atom_invalid_variable() {
        let env = Environment::new();

        // (filter-atom (1 2 3) not-a-var (> $x 1))
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("not-a-var".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_filter_atom_non_list_input() {
        let env = Environment::new();

        // (filter-atom 42 $x (> $x 1)) - non-list input
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_filter_atom_always_true_predicate() {
        let env = Environment::new();

        // (filter-atom (1 2 3) $x true) - always true predicate
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Bool(true),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(filtered) => {
                assert_eq!(filtered.len(), 3);
                assert_eq!(filtered[0], MettaValue::Long(1));
                assert_eq!(filtered[1], MettaValue::Long(2));
                assert_eq!(filtered[2], MettaValue::Long(3));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_filter_atom_always_false_predicate() {
        let env = Environment::new();

        // (filter-atom (1 2 3) $x false) - always false predicate
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Bool(false),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    // === Comprehensive Foldl-Atom Tests ===

    #[test]
    fn test_foldl_atom_complex_operation() {
        let env = Environment::new();

        // (foldl-atom (1 2 3 4) 0 $acc $x (+ $acc (* $x $x))) - sum of squares
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$x".to_string()),
                ]),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30)); // 0 + 1² + 2² + 3² + 4² = 30
    }

    #[test]
    fn test_foldl_atom_single_element() {
        let env = Environment::new();

        // (foldl-atom (42) 0 $acc $x (+ $acc $x))
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(42)]),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_foldl_atom_invalid_acc_variable() {
        let env = Environment::new();

        // Test with invalid accumulator variable
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(1)]),
            MettaValue::Long(0),
            MettaValue::Atom("not-a-var".to_string()), // Invalid variable
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_foldl_atom_invalid_item_variable() {
        let env = Environment::new();

        // Test with invalid item variable
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(1)]),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("not-a-var".to_string()), // Invalid variable
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_foldl_atom_too_many_args() {
        let env = Environment::new();

        // Test with too many arguments
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Long(1)]),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("extra".to_string()), // Extra argument
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_foldl_atom_non_list_input() {
        let env = Environment::new();

        // (foldl-atom 42 0 $acc $x (+ $acc $x)) - non-list input
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::Long(42),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_foldl_atom_division() {
        let env = Environment::new();

        // (foldl-atom (1 2 4) 32 $acc $x (/ $acc $x)) - successive division: 32/1/2/4 = 4
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(4),
            ]),
            MettaValue::Long(32),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("/".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(4)); // 32 / 1 / 2 / 4 = 4
    }

    // === Advanced Integration Tests ===

    #[test]
    fn test_map_fold_compose() {
        let env = Environment::new();

        // First map: (map-atom (1 2 3) $x (* $x $x)) -> (1 4 9)
        let map_items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (map_results, env1) = eval_map_atom(map_items, env);
        assert_eq!(map_results.len(), 1);

        // Then fold: (foldl-atom (1 4 9) 0 $acc $x (+ $acc $x)) -> 14
        let fold_items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            map_results[0].clone(),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (fold_results, _) = eval_foldl_atom(fold_items, env1);
        assert_eq!(fold_results.len(), 1);
        assert_eq!(fold_results[0], MettaValue::Long(14)); // 1 + 4 + 9 = 14
    }

    #[test]
    fn test_filter_fold_compose() {
        let env = Environment::new();

        // First filter: (filter-atom (1 2 3 4 5 6) $x (> $x 3)) -> (4 5 6)
        let filter_items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
                MettaValue::Long(4),
                MettaValue::Long(5),
                MettaValue::Long(6),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(3),
            ]),
        ];

        let (filter_results, env1) = eval_filter_atom(filter_items, env);
        assert_eq!(filter_results.len(), 1);

        // Then fold: (foldl-atom (4 5 6) 1 $acc $x (* $acc $x)) -> 120
        let fold_items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            filter_results[0].clone(),
            MettaValue::Long(1),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (fold_results, _) = eval_foldl_atom(fold_items, env1);
        assert_eq!(fold_results.len(), 1);
        assert_eq!(fold_results[0], MettaValue::Long(120)); // 4 * 5 * 6 = 120
    }

    #[test]
    fn test_all_three_compose() {
        let env = Environment::new();

        // Complex composition: map -> filter -> fold
        // Start with (1 2 3 4 5)
        // Map: multiply by 2 -> (2 4 6 8 10)
        // Filter: keep > 5 -> (6 8 10)
        // Fold: sum -> 24

        let map_items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
                MettaValue::Long(4),
                MettaValue::Long(5),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ];

        let (map_results, env1) = eval_map_atom(map_items, env);

        let filter_items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            map_results[0].clone(),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(5),
            ]),
        ];

        let (filter_results, env2) = eval_filter_atom(filter_items, env1);

        let fold_items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            filter_results[0].clone(),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (fold_results, _) = eval_foldl_atom(fold_items, env2);
        assert_eq!(fold_results.len(), 1);
        assert_eq!(fold_results[0], MettaValue::Long(24)); // 6 + 8 + 10 = 24
    }

    // === Stress Tests ===

    #[test]
    fn test_large_list_performance() {
        let env = Environment::new();

        // Create a large list (100 elements)
        let large_list: Vec<MettaValue> = (1..=100).map(MettaValue::Long).collect();

        // Test map with large list
        let map_items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(large_list.clone()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ];

        let (map_results, env1) = eval_map_atom(map_items, env);
        assert_eq!(map_results.len(), 1);

        if let MettaValue::SExpr(mapped) = &map_results[0] {
            assert_eq!(mapped.len(), 100);
            assert_eq!(mapped[0], MettaValue::Long(2)); // 1 * 2
            assert_eq!(mapped[99], MettaValue::Long(200)); // 100 * 2
        }

        // Test filter with large list
        let filter_items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            map_results[0].clone(),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(100),
            ]),
        ];

        let (filter_results, env2) = eval_filter_atom(filter_items, env1);
        assert_eq!(filter_results.len(), 1);

        if let MettaValue::SExpr(filtered) = &filter_results[0] {
            assert_eq!(filtered.len(), 50); // Numbers > 100: 102, 104, ..., 200
        }

        // Test fold with filtered list
        let fold_items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            filter_results[0].clone(),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (fold_results, _) = eval_foldl_atom(fold_items, env2);
        assert_eq!(fold_results.len(), 1);

        // Sum of 102 + 104 + ... + 200 = 50 * (102 + 200) / 2 = 7550
        assert_eq!(fold_results[0], MettaValue::Long(7550));
    }

    // === Variable Name Edge Cases ===

    #[test]
    fn test_variable_with_underscores() {
        let env = Environment::new();

        // (map-atom (1 2 3) $_var_name (+ $_var_name 1))
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$_var_name".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$_var_name".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(mapped) => {
                assert_eq!(mapped.len(), 3);
                assert_eq!(mapped[0], MettaValue::Long(2));
                assert_eq!(mapped[1], MettaValue::Long(3));
                assert_eq!(mapped[2], MettaValue::Long(4));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_variable_with_numbers() {
        let env = Environment::new();

        // (map-atom (1 2 3) $x1 (+ $x1 1))
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$x1".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x1".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(mapped) => {
                assert_eq!(mapped.len(), 3);
                assert_eq!(mapped[0], MettaValue::Long(2));
                assert_eq!(mapped[1], MettaValue::Long(3));
                assert_eq!(mapped[2], MettaValue::Long(4));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    // === Tests for "Did You Mean" variable format suggestions ===

    #[test]
    fn test_map_atom_variable_format_suggestion() {
        let env = Environment::new();

        // (map-atom (1 2 3) x (+ x 1)) - missing $ prefix on variable
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("x".to_string()), // Missing $ prefix
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("x".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Did you mean: $x"),
                    "Expected suggestion '$x' in: {}",
                    msg
                );
                assert!(
                    msg.contains("variables must start with $"),
                    "Expected explanation in: {}",
                    msg
                );
            }
            _ => panic!("Expected error with variable suggestion"),
        }
    }

    #[test]
    fn test_filter_atom_variable_format_suggestion() {
        let env = Environment::new();

        // (filter-atom (1 2 3) v (> v 1)) - missing $ prefix
        let items = vec![
            MettaValue::Atom("filter-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("v".to_string()), // Missing $ prefix
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("v".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_filter_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Did you mean: $v"),
                    "Expected suggestion '$v' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error with variable suggestion"),
        }
    }

    #[test]
    fn test_foldl_atom_variable_format_suggestion_acc() {
        let env = Environment::new();

        // (foldl-atom (1 2 3) 0 acc $x (+ acc $x)) - missing $ prefix on acc
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Long(0),
            MettaValue::Atom("acc".to_string()), // Missing $ prefix
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("acc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Did you mean: $acc"),
                    "Expected suggestion '$acc' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error with variable suggestion"),
        }
    }

    #[test]
    fn test_foldl_atom_variable_format_suggestion_item() {
        let env = Environment::new();

        // (foldl-atom (1 2 3) 0 $acc item (+ $acc item)) - missing $ prefix on item
        let items = vec![
            MettaValue::Atom("foldl-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Long(0),
            MettaValue::Atom("$acc".to_string()),
            MettaValue::Atom("item".to_string()), // Missing $ prefix
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$acc".to_string()),
                MettaValue::Atom("item".to_string()),
            ]),
        ];

        let (results, _) = eval_foldl_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Did you mean: $item"),
                    "Expected suggestion '$item' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error with variable suggestion"),
        }
    }

    #[test]
    fn test_map_atom_no_suggestion_for_uppercase() {
        let env = Environment::new();

        // (map-atom (1 2 3) X (+ X 1)) - uppercase X shouldn't get suggestion
        let items = vec![
            MettaValue::Atom("map-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("X".to_string()), // Uppercase - not variable-like
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("X".to_string()),
                MettaValue::Long(1),
            ]),
        ];

        let (results, _) = eval_map_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                // Should NOT have "Did you mean" for uppercase identifiers
                assert!(
                    !msg.contains("Did you mean"),
                    "Should not suggest for uppercase identifier: {}",
                    msg
                );
            }
            _ => panic!("Expected error without suggestion"),
        }
    }
}
