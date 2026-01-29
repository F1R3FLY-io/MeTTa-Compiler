//! Tests for list operations.
//!
//! These tests use the full evaluation path (via eval) since the higher-order
//! operations now return EvalStep to enable trampoline-based iteration.

use super::basic::*;
use super::helpers::substitute_variable;
use crate::backend::environment::Environment;
use crate::backend::eval::eval;
use crate::backend::models::{MettaValue, MettaValueInner};

#[test]
fn test_map_atom_simple() {
    let env = Environment::new();

    // (map-atom (1 2 3) $v (+ $v 1))
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(mapped) => {
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
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("map-atom".to_string()),
        MettaValue::SExpr(vec![]),
        MettaValue::Atom("$v".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$v".to_string()),
            MettaValue::Long(1),
        ]),
    ]);

    let (results, _) = eval(expr, env);

    // HE-compatible: empty SExpr () evaluates to itself, not Nil
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::SExpr(vec![]));
}

#[test]
fn test_map_atom_invalid_variable() {
    let env = Environment::new();

    // (map-atom (1 2 3) invalid-var (+ $v 1))
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);

    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].inner(), MettaValueInner::Error(_, _)));
}

#[test]
fn test_substitute_variable() {
    let template = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Atom("$v".to_string()),
        MettaValue::Long(1),
    ]);

    let result = substitute_variable(&template, "$v", &MettaValue::Long(5));

    match result.inner() {
        MettaValueInner::SExpr(items) => {
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

    match result.inner() {
        MettaValueInner::SExpr(outer) => {
            assert_eq!(outer.len(), 3);
            assert_eq!(outer[0], MettaValue::Atom("*".to_string()));
            match outer[1].inner() {
                MettaValueInner::SExpr(inner) => {
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
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(filtered) => {
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
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("filter-atom".to_string()),
        MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]),
        MettaValue::Atom("$v".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom(">".to_string()),
            MettaValue::Atom("$v".to_string()),
            MettaValue::Long(5),
        ]),
    ]);

    let (results, _) = eval(expr, env);

    // HE-compatible: empty SExpr () evaluates to itself, not Nil
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::SExpr(vec![]));
}

#[test]
fn test_filter_atom_empty_list() {
    let env = Environment::new();

    // (filter-atom () $v (> $v 2))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("filter-atom".to_string()),
        MettaValue::SExpr(vec![]),
        MettaValue::Atom("$v".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom(">".to_string()),
            MettaValue::Atom("$v".to_string()),
            MettaValue::Long(2),
        ]),
    ]);

    let (results, _) = eval(expr, env);

    // HE-compatible: empty SExpr () evaluates to itself, not Nil
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::SExpr(vec![]));
}

// === Fold Tests ===

#[test]
fn test_foldl_atom_sum() {
    let env = Environment::new();

    // (foldl-atom (1 2 3 4) 0 $acc $x (+ $acc $x))
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(10)); // 0 + 1 + 2 + 3 + 4 = 10
}

#[test]
fn test_foldl_atom_product() {
    let env = Environment::new();

    // (foldl-atom (2 3 4) 1 $acc $x (* $acc $x))
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(24)); // 1 * 2 * 3 * 4 = 24
}

#[test]
fn test_foldl_atom_empty_list() {
    let env = Environment::new();

    // (foldl-atom () 42 $acc $x (+ $acc $x))
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42)); // Should return initial value
}

#[test]
fn test_foldl_atom_wrong_arity() {
    let env = Environment::new();

    // (foldl-atom (1 2 3) 0) - missing arguments
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("foldl-atom".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
        MettaValue::Long(0),
    ]);

    let (results, _) = eval(expr, env);

    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].inner(), MettaValueInner::Error(_, _)));
}

// === Integration Tests ===

#[test]
fn test_map_filter_compose() {
    let env = Environment::new();

    // First map: (map-atom (1 2 3 4) $v (* $v 2)) -> (2 4 6 8)
    let map_expr = MettaValue::SExpr(vec![
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
    ]);

    let (map_results, env1) = eval(map_expr, env);
    assert_eq!(map_results.len(), 1);

    // Then filter: (filter-atom (2 4 6 8) $v (> $v 4)) -> (6 8)
    let filter_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("filter-atom".to_string()),
        map_results[0].clone(), // Use result from map
        MettaValue::Atom("$v".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom(">".to_string()),
            MettaValue::Atom("$v".to_string()),
            MettaValue::Long(4),
        ]),
    ]);

    let (filter_results, _) = eval(filter_expr, env1);
    assert_eq!(filter_results.len(), 1);

    match filter_results[0].inner() {
        MettaValueInner::SExpr(filtered) => {
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
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("map-atom".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(mapped) => {
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
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("map-atom".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(42),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(mapped) => {
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
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("map-atom".to_string()),
        MettaValue::SExpr(vec![MettaValue::Long(1)]),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].inner(), MettaValueInner::Error(_, _)));
}

#[test]
fn test_map_atom_non_list_input() {
    let env = Environment::new();

    // (map-atom 42 $x (+ $x 1)) - non-list as first argument
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("map-atom".to_string()),
        MettaValue::Long(42),
        MettaValue::Atom("$x".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].inner(), MettaValueInner::Error(_, _)));
}

#[test]
fn test_map_atom_nil_input() {
    let env = Environment::new();

    // (map-atom nil $x (+ $x 1))
    // Nil is treated as an empty list, and returns empty list (HE-compatible)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("map-atom".to_string()),
        MettaValue::Nil(),
        MettaValue::Atom("$x".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]),
    ]);

    let (results, _) = eval(expr, env);
    // HE-compatible: empty SExpr () evaluates to itself, not Nil
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::SExpr(vec![]));
}

#[test]
fn test_map_atom_mixed_types() {
    let env = Environment::new();

    // (map-atom (1 "hello" true) $x $x) - mixed type list
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("map-atom".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::String("hello".to_string()),
            MettaValue::Bool(true),
        ]),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(mapped) => {
            assert_eq!(mapped.len(), 3);
            assert_eq!(mapped[0], MettaValue::Long(1));
            assert_eq!(mapped[1], MettaValue::String("hello".to_string()));
            assert_eq!(mapped[2], MettaValue::Bool(true));
        }
        _ => panic!("Expected S-expression result"),
    }
}

// === Variable Name Edge Cases ===

#[test]
fn test_variable_with_underscores() {
    let env = Environment::new();

    // (map-atom (1 2 3) $_var_name (+ $_var_name 1))
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(mapped) => {
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
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(mapped) => {
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
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::Error(msg, _) => {
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
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::Error(msg, _) => {
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
    let expr = MettaValue::SExpr(vec![
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
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::Error(msg, _) => {
            assert!(
                msg.contains("Did you mean: $acc"),
                "Expected suggestion '$acc' in: {}",
                msg
            );
        }
        _ => panic!("Expected error with variable suggestion"),
    }
}
