use std::sync::Arc;

use super::*;
use super::cartesian::Combination;
use crate::backend::models::Rule;

#[test]
fn test_eval_atom() {
    let env = Environment::new();
    let value = MettaValue::Atom("foo".to_string());
    let (results, _) = eval(value.clone(), env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], value);
}

#[test]
fn test_eval_builtin_add() {
    let env = Environment::new();
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));
}

#[test]
fn test_eval_builtin_comparison() {
    let env = Environment::new();
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_eval_logical_and() {
    let env = Environment::new();

    // True and True = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // True and False = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // False and True = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // False and False = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_eval_logical_or() {
    let env = Environment::new();

    // True or True = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // True or False = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // False or True = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // False or False = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_eval_logical_not() {
    let env = Environment::new();

    // not True = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // not False = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_eval_logical_type_error() {
    let env = Environment::new();

    // and with non-boolean should error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Long(1),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));

    // or with non-boolean FIRST arg should error
    // (With short-circuit, (or True "hello") returns True without checking second arg)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::String("hello".to_string()),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));

    // not with non-boolean should error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Long(42),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));
}

#[test]
fn test_eval_logical_arity_error() {
    let env = Environment::new();

    // and with wrong arity
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));

    // not with wrong arity
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));
}

#[test]
fn test_pattern_match_simple() {
    let pattern = MettaValue::Atom("$x".to_string());
    let value = MettaValue::Long(42);
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());
    let bindings = bindings.unwrap();
    assert_eq!(
        bindings
            .iter()
            .find(|(name, _)| name.as_str() == "$x")
            .map(|(_, val)| val),
        Some(&MettaValue::Long(42))
    );
}

#[test]
fn test_pattern_match_sexpr() {
    let pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(2),
    ]);
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());
    let bindings = bindings.unwrap();
    assert_eq!(
        bindings
            .iter()
            .find(|(name, _)| name.as_str() == "$x")
            .map(|(_, val)| val),
        Some(&MettaValue::Long(1))
    );
}

#[test]
fn test_pattern_match_empty_sexpr_matches_empty_only() {
    // Empty S-expression () should ONLY match empty values (Nil, empty S-expr, Unit, Empty atom)
    // Use _ for wildcard pattern to match anything
    let pattern = MettaValue::SExpr(vec![]);

    // Should NOT match Long
    let value = MettaValue::Long(42);
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_none());

    // Should NOT match String
    let value = MettaValue::String("hello".to_string());
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_none());

    // Should NOT match non-empty S-expression
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_none());

    // SHOULD match Nil
    let value = MettaValue::Nil;
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());
    assert!(bindings.unwrap().is_empty());

    // SHOULD match another empty S-expression
    let value = MettaValue::SExpr(vec![]);
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());
    assert!(bindings.unwrap().is_empty());

    // SHOULD match Unit
    let value = MettaValue::Unit;
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());
    assert!(bindings.unwrap().is_empty());

    // SHOULD match Empty atom
    let value = MettaValue::Atom("Empty".to_string());
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());
    assert!(bindings.unwrap().is_empty());

    // Should NOT match Bool
    let value = MettaValue::Bool(true);
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_none());
}

#[test]
fn test_eval_with_rule() {
    let mut env = Environment::new();

    // Add rule: (= (double $x) (mul $x 2))
    let rule = Rule::new(
    MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]),
    MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]),
);
    env.add_rule(rule);

    // Evaluate (double 5)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("double".to_string()),
        MettaValue::Long(5),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(10));
}

// === Integration Test ===

#[test]
fn test_eval_with_quote() {
    let env = Environment::new();

    // (eval (quote (+ 1 2)))
    // Quote prevents evaluation, eval forces it
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("eval".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));
}

#[test]
fn test_mvp_complete() {
    let mut env = Environment::new();

    // Add a rule: (= (safe-div $x $y) (if (== $y 0) (error "division by zero" $y) (div $x $y)))
    let rule = Rule::new(
    MettaValue::SExpr(vec![
            MettaValue::Atom("safe-div".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]),
    MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("==".to_string()),
                MettaValue::Atom("$y".to_string()),
                MettaValue::Long(0),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("division by zero".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("/".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]),
);
    env.add_rule(rule);

    // Test successful division: (safe-div 10 2) -> 5
    let value1 = MettaValue::SExpr(vec![
        MettaValue::Atom("safe-div".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(2),
    ]);
    let (results1, env1) = eval(value1, env.clone());
    assert_eq!(results1[0], MettaValue::Long(5));

    // Test division by zero: (safe-div 10 0) -> Error
    let value2 = MettaValue::SExpr(vec![
        MettaValue::Atom("safe-div".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(0),
    ]);
    let (results2, _) = eval(value2, env1);
    match &results2[0] {
        MettaValue::Error(msg, _) => {
            assert_eq!(msg, "division by zero");
        }
        other => panic!("Expected error, got {:?}", other),
    }
}

// === Tests adapted from hyperon-experimental ===
// Source: https://github.com/trueagi-io/hyperon-experimental

#[test]
fn test_nested_arithmetic() {
    // From c1_grounded_basic.metta: (+ 2 (* 3 5))
    let env = Environment::new();
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(2),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Long(3),
            MettaValue::Long(5),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Long(17)); // 2 + (3 * 5) = 17
}

#[test]
fn test_comparison_with_arithmetic() {
    // From c1_grounded_basic.metta: (< 4 (+ 2 (* 3 5)))
    let env = Environment::new();
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Long(4),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(5),
            ]),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Bool(true)); // 4 < 17
}

#[test]
fn test_equality_literals() {
    // From c1_grounded_basic.metta: (== 4 (+ 2 2))
    let env = Environment::new();
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Long(4),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(2),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Bool(true)); // 4 == 4
}

#[test]
fn test_equality_sexpr() {
    // From c1_grounded_basic.metta: structural equality tests
    let env = Environment::new();

    // (== (A B) (A B)) should be supported via pattern matching
    // For now we test that equal atoms are equal
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Long(42),
        MettaValue::Long(42),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_factorial_recursive() {
    // From c1_grounded_basic.metta: factorial example with if guard
    // (= (fact $n) (if (> $n 0) (* $n (fact (- $n 1))) 1))
    let mut env = Environment::new();

    let rule = Rule::new(
    MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Atom("$n".to_string()),
        ]),
    MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            // Condition: (> $n 0)
            MettaValue::SExpr(vec![
                MettaValue::Atom(">".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::Long(0),
            ]),
            // Then branch: (* $n (fact (- $n 1)))
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("fact".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Long(1),
                    ]),
                ]),
            ]),
            // Else branch: 1
            MettaValue::Long(1),
        ]),
);
    env.add_rule(rule);

    // Test (fact 3) = 6
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("fact".to_string()),
        MettaValue::Long(3),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(6));
}

#[test]
fn test_factorial_with_compile() {
    // Test factorial using compile() to ensure the compiled version works
    // This complements test_factorial_recursive which uses manual construction
    use crate::backend::compile::compile;

    let input = r#"
        (= (fact $n) (if (> $n 0) (* $n (fact (- $n 1))) 1))
        !(fact 0)
        !(fact 1)
        !(fact 2)
        !(fact 3)
    "#;

    let state = compile(input).unwrap();
    let mut env = state.environment;
    let mut results = Vec::new();

    for expr in state.source {
        let (expr_results, new_env) = eval(expr, env);
        env = new_env;
        // Collect non-empty results (skip rule definitions)
        if !expr_results.is_empty() {
            results.extend(expr_results);
        }
    }

    // Should have 4 results: fact(0)=1, fact(1)=1, fact(2)=2, fact(3)=6
    assert_eq!(results.len(), 4);
    assert_eq!(results[0], MettaValue::Long(1)); // fact(0)
    assert_eq!(results[1], MettaValue::Long(1)); // fact(1)
    assert_eq!(results[2], MettaValue::Long(2)); // fact(2)
    assert_eq!(results[3], MettaValue::Long(6)); // fact(3)
}

#[test]
fn test_incremental_nested_arithmetic() {
    // From test_metta.py: !(+ 1 (+ 2 (+ 3 4)))
    let env = Environment::new();
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Long(10));
}

#[test]
fn test_function_definition_and_call() {
    // From test_run_metta.py: (= (f) (+ 2 3)) !(f)
    let mut env = Environment::new();

    // Define rule: (= (f) (+ 2 3))
    let rule = Rule::new(
    MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
    MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
);
    env.add_rule(rule);

    // Evaluate (f)
    let value = MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Long(5));
}

#[test]
fn test_multiple_pattern_variables() {
    // Test pattern matching with multiple variables
    let mut env = Environment::new();

    // (= (add3 $a $b $c) (+ $a (+ $b $c)))
    let rule = Rule::new(
    MettaValue::SExpr(vec![
            MettaValue::Atom("add3".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("$b".to_string()),
            MettaValue::Atom("$c".to_string()),
        ]),
    MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$b".to_string()),
                MettaValue::Atom("$c".to_string()),
            ]),
        ]),
);
    env.add_rule(rule);

    // (add3 10 20 30) = 60
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("add3".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(20),
        MettaValue::Long(30),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Long(60));
}

#[test]
fn test_nested_pattern_matching() {
    // Test nested S-expression pattern matching
    let mut env = Environment::new();

    // (= (eval-pair (pair $x $y)) (+ $x $y))
    let rule = Rule::new(
    MettaValue::SExpr(vec![
            MettaValue::Atom("eval-pair".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pair".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]),
    MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]),
);
    env.add_rule(rule);

    // (eval-pair (pair 5 7)) = 12
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("eval-pair".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("pair".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(7),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Long(12));
}

#[test]
fn test_wildcard_pattern() {
    // Test wildcard matching
    let pattern = MettaValue::Atom("_".to_string());
    let value = MettaValue::Long(42);
    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());

    // Wildcard should not bind the value
    let bindings = bindings.unwrap();
    assert!(bindings.is_empty());
}

#[test]
fn test_variable_consistency_in_pattern() {
    // Test that the same variable in a pattern must match the same value
    let pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("same".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);

    // Should match when both are the same
    let value1 = MettaValue::SExpr(vec![
        MettaValue::Atom("same".to_string()),
        MettaValue::Long(5),
        MettaValue::Long(5),
    ]);
    assert!(pattern_match(&pattern, &value1).is_some());

    // Should not match when they differ
    let value2 = MettaValue::SExpr(vec![
        MettaValue::Atom("same".to_string()),
        MettaValue::Long(5),
        MettaValue::Long(7),
    ]);
    assert!(pattern_match(&pattern, &value2).is_none());
}

#[test]
fn test_conditional_with_pattern_matching() {
    // Test combining if with pattern matching
    let mut env = Environment::new();

    // (= (abs $x) (if (< $x 0) (- 0 $x) $x))
    let rule = Rule::new(
    MettaValue::SExpr(vec![
            MettaValue::Atom("abs".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]),
    MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("<".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(0),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("-".to_string()),
                MettaValue::Long(0),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]),
);
    env.add_rule(rule);

    // abs(-5) = 5
    let value1 = MettaValue::SExpr(vec![
        MettaValue::Atom("abs".to_string()),
        MettaValue::Long(-5),
    ]);
    let (results, env1) = eval(value1, env.clone());
    assert_eq!(results[0], MettaValue::Long(5));

    // abs(7) = 7
    let value2 = MettaValue::SExpr(vec![
        MettaValue::Atom("abs".to_string()),
        MettaValue::Long(7),
    ]);
    let (results, _) = eval(value2, env1);
    assert_eq!(results[0], MettaValue::Long(7));
}

#[test]
fn test_string_values() {
    // Test string value handling
    let env = Environment::new();
    let value = MettaValue::String("test".to_string());
    let (results, _) = eval(value.clone(), env);
    assert_eq!(results[0], value);
}

#[test]
fn test_boolean_values() {
    let env = Environment::new();

    let value_true = MettaValue::Bool(true);
    let (results, _) = eval(value_true.clone(), env.clone());
    assert_eq!(results[0], value_true);

    let value_false = MettaValue::Bool(false);
    let (results, _) = eval(value_false.clone(), env);
    assert_eq!(results[0], value_false);
}

#[test]
fn test_nil_value() {
    let env = Environment::new();
    let value = MettaValue::Nil;
    let (results, _) = eval(value, env);
    assert_eq!(results[0], MettaValue::Nil);
}

// === Fact Database Tests ===

#[test]
fn test_symbol_added_to_fact_database() {
    // Bare atoms should NOT be added to the fact database
    // Only rules, type assertions, and unmatched s-expressions are stored
    let env = Environment::new();

    // Evaluate the symbol "Hello"
    let symbol = MettaValue::Atom("Hello".to_string());
    let (results, new_env) = eval(symbol.clone(), env);

    // Symbol should be returned unchanged
    assert_eq!(results[0], symbol);

    // Bare atoms should NOT be added to fact database (this prevents pollution)
    assert!(!new_env.has_fact("Hello"));
}

#[test]
fn test_variables_not_added_to_fact_database() {
    let env = Environment::new();

    // Test $variable
    let var1 = MettaValue::Atom("$x".to_string());
    let (_, new_env) = eval(var1, env.clone());
    assert!(!new_env.has_fact("$x"));

    // Test &variable
    let var2 = MettaValue::Atom("&y".to_string());
    let (_, new_env) = eval(var2, env.clone());
    assert!(!new_env.has_fact("&y"));

    // Test 'variable
    let var3 = MettaValue::Atom("'z".to_string());
    let (_, new_env) = eval(var3, env.clone());
    assert!(!new_env.has_fact("'z"));

    // Test wildcard
    let wildcard = MettaValue::Atom("_".to_string());
    let (_, new_env) = eval(wildcard, env);
    assert!(!new_env.has_fact("_"));
}

#[test]
fn test_multiple_symbols_in_fact_database() {
    // Bare atoms should NOT be added to fact database
    // This test verifies that evaluating multiple atoms doesn't pollute the environment
    let env = Environment::new();

    // Evaluate multiple symbols
    let symbol1 = MettaValue::Atom("Foo".to_string());
    let (_, env1) = eval(symbol1, env);

    let symbol2 = MettaValue::Atom("Bar".to_string());
    let (_, env2) = eval(symbol2, env1);

    let symbol3 = MettaValue::Atom("Baz".to_string());
    let (_, env3) = eval(symbol3, env2);

    // Bare atoms should NOT be in the fact database
    assert!(!env3.has_fact("Foo"));
    assert!(!env3.has_fact("Bar"));
    assert!(!env3.has_fact("Baz"));
}

#[test]
fn test_sexpr_added_to_fact_database() {
    // Verify official MeTTa ADD mode semantics:
    // When an s-expression like (Hello World) is evaluated, it is automatically added to the space
    // This matches: `(leaf1 leaf2)` in REPL -> auto-added, queryable via `!(match &self ...)`
    let env = Environment::new();

    // Evaluate the s-expression (Hello World)
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("Hello".to_string()),
        MettaValue::Atom("World".to_string()),
    ]);
    let expected_result = MettaValue::SExpr(vec![
        MettaValue::Atom("Hello".to_string()),
        MettaValue::Atom("World".to_string()),
    ]);

    let (results, new_env) = eval(sexpr.clone(), env);

    // S-expression should be returned (with evaluated elements)
    assert_eq!(results[0], expected_result);

    // S-expression should be added to fact database (ADD mode behavior)
    assert!(new_env.has_sexpr_fact(&expected_result));

    // Individual atoms are NOT stored separately
    // Only the full s-expression is stored in MORK format
    assert!(!new_env.has_fact("Hello"));
    assert!(!new_env.has_fact("World"));
}

#[test]
fn test_nested_sexpr_in_fact_database() {
    // Official MeTTa semantics: only the top-level expression is stored
    // Nested sub-expressions are NOT extracted and stored separately
    let env = Environment::new();

    // Evaluate a nested s-expression
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("Outer".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]),
    ]);

    let (_, new_env) = eval(sexpr, env);

    // CORRECT: Outer s-expression should be in fact database
    let expected_outer = MettaValue::SExpr(vec![
        MettaValue::Atom("Outer".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]),
    ]);
    assert!(new_env.has_sexpr_fact(&expected_outer));

    // CORRECT: Inner s-expression should NOT be in fact database (not recursively stored)
    // Official MeTTa only stores the top-level expression passed to add-atom
    let expected_inner = MettaValue::SExpr(vec![
        MettaValue::Atom("Inner".to_string()),
        MettaValue::Atom("Nested".to_string()),
    ]);
    assert!(!new_env.has_sexpr_fact(&expected_inner));

    // Individual atoms are NOT stored separately
    assert!(!new_env.has_fact("Outer"));
    assert!(!new_env.has_fact("Inner"));
    assert!(!new_env.has_fact("Nested"));
}

#[test]
fn test_pattern_matching_extracts_nested_sexpr() {
    // Demonstrates that while nested s-expressions are NOT stored separately,
    // they can still be accessed via pattern matching with variables.
    // This is how official MeTTa handles nested data extraction.
    let mut env = Environment::new();

    // Store a nested s-expression: (Outer (Inner Nested))
    let nested_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("Outer".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]),
    ]);

    // Evaluate to add to space (ADD mode behavior)
    let (_, env1) = eval(nested_expr.clone(), env);
    env = env1;

    // Verify only the outer expression is stored
    assert!(env.has_sexpr_fact(&nested_expr));
    let inner_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("Inner".to_string()),
        MettaValue::Atom("Nested".to_string()),
    ]);
    assert!(!env.has_sexpr_fact(&inner_expr)); // NOT stored separately

    // Use pattern matching to extract the nested part: (match & self (Outer $x) $x)
    let match_query = MettaValue::SExpr(vec![
        MettaValue::Atom("match".to_string()),
        MettaValue::Atom("&".to_string()),
        MettaValue::Atom("self".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::Atom("$x".to_string()), // Variable to capture nested part
        ]),
        MettaValue::Atom("$x".to_string()), // Template: return the captured value
    ]);

    let (results, _) = eval(match_query, env);

    // Should return the nested s-expression even though it wasn't stored separately
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], inner_expr); // Pattern matching extracts (Inner Nested)
}

#[test]
fn test_grounded_operations_not_added_to_sexpr_facts() {
    let env = Environment::new();

    // Evaluate an arithmetic operation (add 1 2)
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);

    let (results, new_env) = eval(sexpr.clone(), env);

    // Result should be 3
    assert_eq!(results[0], MettaValue::Long(3));

    // The s-expression should NOT be in the fact database
    // because it was reduced to a value by a grounded operation
    assert!(!new_env.has_sexpr_fact(&sexpr));
}

#[test]
fn test_rule_definition_added_to_fact_database() {
    let env = Environment::new();

    // Define a rule: (= (double $x) (* $x 2))
    let rule_def = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]),
    ]);

    let (result, new_env) = eval(rule_def.clone(), env);

    // Rule definition should return empty list
    assert!(result.is_empty());

    // Rule definition should also be in the fact database
    assert!(new_env.has_sexpr_fact(&rule_def));
}

// ========================================================================
// Conjunction Pattern Tests
// ========================================================================

#[test]
fn test_empty_conjunction() {
    let env = Environment::new();

    // Empty conjunction: (,) → Nil
    let value = MettaValue::Conjunction(vec![]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Nil);
}

#[test]
fn test_unary_conjunction() {
    let env = Environment::new();

    // Unary conjunction: (, expr) → evaluates expr directly
    let value = MettaValue::Conjunction(vec![MettaValue::Long(42)]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

#[test]
fn test_unary_conjunction_with_expression() {
    let env = Environment::new();

    // Unary conjunction with expression: (, (+ 2 3)) → 5
    let value = MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ])]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(5));
}

#[test]
fn test_binary_conjunction() {
    let env = Environment::new();

    // Binary conjunction: (, (+ 1 1) (+ 2 2)) → 2, 4
    let value = MettaValue::Conjunction(vec![
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(1),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(2),
        ]),
    ]);

    let (results, _) = eval(value, env);
    // Binary conjunction returns results from the last goal
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(4));
}

#[test]
fn test_nary_conjunction() {
    let env = Environment::new();

    // N-ary conjunction: (, (+ 1 1) (+ 2 2) (+ 3 3)) → 2, 4, 6 (returns last)
    let value = MettaValue::Conjunction(vec![
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(1),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(2),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(3),
            MettaValue::Long(3),
        ]),
    ]);

    let (results, _) = eval(value, env);
    // N-ary conjunction returns results from the last goal
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(6));
}

#[test]
fn test_conjunction_pattern_match() {
    // Test that conjunctions can be pattern matched
    let pattern = MettaValue::Conjunction(vec![
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);

    let value = MettaValue::Conjunction(vec![MettaValue::Long(1), MettaValue::Long(2)]);

    let bindings = pattern_match(&pattern, &value);
    assert!(bindings.is_some());

    let bindings = bindings.unwrap();
    assert_eq!(
        bindings
            .iter()
            .find(|(name, _)| name.as_str() == "$x")
            .map(|(_, val)| val),
        Some(&MettaValue::Long(1))
    );
    assert_eq!(
        bindings
            .iter()
            .find(|(name, _)| name.as_str() == "$y")
            .map(|(_, val)| val),
        Some(&MettaValue::Long(2))
    );
}

#[test]
fn test_conjunction_with_error_propagation() {
    let env = Environment::new();

    // Conjunction with error should propagate the error
    let value = MettaValue::Conjunction(vec![
        MettaValue::Long(42),
        MettaValue::Error("test error".to_string(), Arc::new(MettaValue::Nil)),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    // Error should propagate from the conjunction
    assert!(matches!(results[0], MettaValue::Error(_, _)));
}

#[test]
fn test_nested_conjunction() {
    let env = Environment::new();

    // Nested conjunction: (, (+ 1 2) (, (+ 3 4)))
    let value = MettaValue::Conjunction(vec![
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
        MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(3),
            MettaValue::Long(4),
        ])]),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    // Nested conjunction should evaluate to the last result
    assert_eq!(results[0], MettaValue::Long(7));
}

#[test]
fn test_arithmetic_type_error_bool() {
    let env = Environment::new();

    // Test: !(* true false) - booleans not valid for arithmetic
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, _details) => {
            assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
            assert!(
                msg.contains("expected Number"),
                "Expected type info in: {}",
                msg
            );
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_string_comparison() {
    let env = Environment::new();

    // Test: !(< "a" "b") - lexicographic string comparison
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::String("a".to_string()),
        MettaValue::String("b".to_string()),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true)); // "a" < "b" lexicographically
}

#[test]
fn test_comparison_mixed_type_error() {
    let env = Environment::new();

    // Test: !(< "hello" 42) - mixed types should error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::String("hello".to_string()),
        MettaValue::Long(42),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, _details) => {
            // The error should indicate incompatible types
            assert!(
                msg.contains("type") || msg.contains("Cannot compare"),
                "Expected type mismatch error in: {}",
                msg
            );
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_arithmetic_wrong_arity() {
    let env = Environment::new();

    // Test: !(+ 1) - wrong number of arguments
    let value = MettaValue::SExpr(vec![MettaValue::Atom("+".to_string()), MettaValue::Long(1)]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, _) => {
            assert!(msg.contains("2 arguments"));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

// ========================================================================
// Fuzzy Matching / Typo Detection Tests
// ========================================================================

#[test]
fn test_misspelled_special_form() {
    // Issue #51: When a misspelled special form is detected, a warning is printed
    // to stderr but the expression is returned as-is (ADD mode semantics).
    // This allows intentional data constructors like `lit` to work without errors.
    let env = Environment::new();

    // Try to use "mach" instead of "match" (4 chars, passes min length check)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("mach".to_string()),
        MettaValue::Atom("&self".to_string()),
        MettaValue::Atom("pattern".to_string()),
    ]);

    let (results, _) = eval(expr.clone(), env);
    assert_eq!(results.len(), 1);

    // Per issue #51: undefined symbols are treated as data (ADD mode)
    // A warning is printed to stderr, but the expression is returned as data
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Atom("mach".to_string()));
            // &self is now resolved to a Space reference
            assert!(
                matches!(items[1], MettaValue::Space(_)),
                "Expected &self to be resolved to Space, got {:?}",
                items[1]
            );
            assert_eq!(items[2], MettaValue::Atom("pattern".to_string()));
        }
        other => panic!("Expected SExpr (data), got {:?}", other),
    }
}

#[test]
fn test_undefined_symbol_with_rule_suggestion() {
    // Issue #51: When a misspelled function is detected, a warning is printed
    // to stderr but the expression is returned as-is (ADD mode semantics).
    let mut env = Environment::new();

    // Add a rule for "fibonacci"
    let rule = Rule::new(
        MettaValue::SExpr(vec![
            MettaValue::Atom("fibonacci".to_string()),
            MettaValue::Atom("$n".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$n".to_string()),
            MettaValue::Atom("$n".to_string()),
        ]),
    );
    env.add_rule(rule);

    // Try to call "fibonaci" (misspelled - missing 'n')
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("fibonaci".to_string()),
        MettaValue::Long(5),
    ]);

    let (results, _) = eval(expr.clone(), env);
    assert_eq!(results.len(), 1);

    // Per issue #51: Should return the expression unchanged (ADD mode)
    // A warning is printed to stderr, but no error is returned
    if let MettaValue::SExpr(items) = &results[0] {
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], MettaValue::Atom("fibonaci".to_string()));
        assert_eq!(items[1], MettaValue::Long(5));
    } else {
        panic!(
            "Expected SExpr returned unchanged (ADD mode), got: {:?}",
            results[0]
        );
    }
}

#[test]
fn test_unknown_symbol_returns_as_is() {
    let env = Environment::new();

    // Completely unknown symbols (not similar to any known term)
    // should be returned as-is per ADD mode semantics
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("xyzzy".to_string()),
        MettaValue::Long(1),
    ]);

    let (results, _) = eval(expr.clone(), env);
    assert_eq!(results.len(), 1);

    // Should return the expression as-is (ADD mode), not an error
    assert_eq!(results[0], expr, "Expected expression to be returned as-is");
}

#[test]
fn test_short_symbol_not_flagged_as_typo() {
    let env = Environment::new();

    // Short symbols like "a" should NOT be flagged as typos even if
    // they're close to special forms like "=" (edit distance 1)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("a".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);

    let (results, _) = eval(expr.clone(), env);
    assert_eq!(results.len(), 1);

    // Should return the expression as-is (ADD mode), not an error
    assert_eq!(
        results[0], expr,
        "Short symbols should not be flagged as typos"
    );
}

// ==========================================================================
// Lazy Cartesian Product Iterator Tests
// ==========================================================================

#[test]
fn test_cartesian_product_iter_basic() {
    // Test basic 2x2 cartesian product
    let results = vec![
        vec![MettaValue::Long(1), MettaValue::Long(2)],
        vec![MettaValue::Long(10), MettaValue::Long(20)],
    ];
    let iter = CartesianProductIter::new(results).expect("Should create iterator");

    let combos: Vec<Combination> = iter.collect();

    assert_eq!(combos.len(), 4);
    assert_eq!(combos[0].as_slice(), &[MettaValue::Long(1), MettaValue::Long(10)]);
    assert_eq!(combos[1].as_slice(), &[MettaValue::Long(1), MettaValue::Long(20)]);
    assert_eq!(combos[2].as_slice(), &[MettaValue::Long(2), MettaValue::Long(10)]);
    assert_eq!(combos[3].as_slice(), &[MettaValue::Long(2), MettaValue::Long(20)]);
}

#[test]
fn test_cartesian_product_iter_single_element() {
    // All single-element lists - deterministic case
    let results = vec![
        vec![MettaValue::Long(1)],
        vec![MettaValue::Long(2)],
        vec![MettaValue::Long(3)],
    ];
    let iter = CartesianProductIter::new(results).expect("Should create iterator");

    let combos: Vec<Combination> = iter.collect();

    assert_eq!(combos.len(), 1);
    assert_eq!(
        combos[0].as_slice(),
        &[MettaValue::Long(1), MettaValue::Long(2), MettaValue::Long(3)]
    );
}

#[test]
fn test_cartesian_product_iter_empty_input() {
    // Empty outer vector - the iterator is created but produces no combinations
    // Note: cartesian_product_lazy handles this case specially by returning Single(vec![])
    let results: Vec<Vec<MettaValue>> = vec![];
    let iter = CartesianProductIter::new(results);

    // Iterator is created (Some) but produces no items because results.is_empty() check in next()
    assert!(iter.is_some());
    let combos: Vec<Combination> = iter.unwrap().collect();
    assert!(combos.is_empty());
}

#[test]
fn test_cartesian_product_iter_empty_list() {
    // One empty list should return None
    let results = vec![
        vec![MettaValue::Long(1), MettaValue::Long(2)],
        vec![], // Empty list
        vec![MettaValue::Long(10), MettaValue::Long(20)],
    ];
    let iter = CartesianProductIter::new(results);

    assert!(iter.is_none());
}

#[test]
fn test_cartesian_product_iter_3x3x3() {
    // Test 3x3x3 = 27 combinations
    let results = vec![
        vec![MettaValue::Long(1), MettaValue::Long(2), MettaValue::Long(3)],
        vec![
            MettaValue::Atom("a".into()),
            MettaValue::Atom("b".into()),
            MettaValue::Atom("c".into()),
        ],
        vec![MettaValue::Bool(true), MettaValue::Bool(false), MettaValue::Nil],
    ];
    let iter = CartesianProductIter::new(results).expect("Should create iterator");

    let combos: Vec<Combination> = iter.collect();

    assert_eq!(combos.len(), 27);

    // Verify first and last combinations
    assert_eq!(
        combos[0].as_slice(),
        &[MettaValue::Long(1), MettaValue::Atom("a".into()), MettaValue::Bool(true)]
    );
    assert_eq!(
        combos[26].as_slice(),
        &[MettaValue::Long(3), MettaValue::Atom("c".into()), MettaValue::Nil]
    );
}

#[test]
fn test_cartesian_product_lazy_count() {
    // Verify iterator is lazy by checking memory usage pattern
    let results = cartesian_product_lazy(vec![
        vec![MettaValue::Long(1), MettaValue::Long(2)],
        vec![MettaValue::Long(10), MettaValue::Long(20), MettaValue::Long(30)],
    ]);

    match results {
        CartesianProductResult::Lazy(iter) => {
            // Count combinations without storing them all
            let count = iter.count();
            assert_eq!(count, 6);
        }
        _ => panic!("Expected Lazy variant for nondeterministic case"),
    }
}

#[test]
fn test_cartesian_product_lazy_single_returns_single() {
    // Fast path: single combination returns Single variant
    let results = cartesian_product_lazy(vec![
        vec![MettaValue::Long(1)],
        vec![MettaValue::Long(2)],
    ]);

    match results {
        CartesianProductResult::Single(combo) => {
            assert_eq!(combo.as_slice(), &[MettaValue::Long(1), MettaValue::Long(2)]);
        }
        _ => panic!("Expected Single variant for deterministic case"),
    }
}

#[test]
fn test_cartesian_product_lazy_empty_returns_single_empty() {
    // Empty input returns Single(vec![]) - the identity element
    // The Cartesian product of nothing is a single empty tuple
    let results = cartesian_product_lazy(vec![]);

    match results {
        CartesianProductResult::Single(combo) => {
            assert!(combo.is_empty(), "Should be empty tuple");
        }
        _ => panic!("Expected Single(vec![]) for empty input"),
    }
}

#[test]
fn test_cartesian_product_lazy_with_empty_list_returns_empty() {
    // Empty list in results returns Empty variant
    let results = cartesian_product_lazy(vec![
        vec![MettaValue::Long(1)],
        vec![], // Empty
    ]);

    match results {
        CartesianProductResult::Empty => {}
        _ => panic!("Expected Empty variant when one list is empty"),
    }
}

#[test]
fn test_cartesian_product_ordering_preserved() {
    // Verify outer-product ordering is preserved (rightmost index varies fastest)
    let results = vec![
        vec![MettaValue::Long(1), MettaValue::Long(2)],     // First dimension
        vec![MettaValue::Long(10), MettaValue::Long(20)],   // Second dimension
    ];
    let iter = CartesianProductIter::new(results).expect("Should create iterator");

    let combos: Vec<Combination> = iter.collect();

    // Ordering: (1,10), (1,20), (2,10), (2,20)
    // Rightmost index varies fastest
    assert_eq!(combos[0].as_slice(), &[MettaValue::Long(1), MettaValue::Long(10)]);
    assert_eq!(combos[1].as_slice(), &[MettaValue::Long(1), MettaValue::Long(20)]);
    assert_eq!(combos[2].as_slice(), &[MettaValue::Long(2), MettaValue::Long(10)]);
    assert_eq!(combos[3].as_slice(), &[MettaValue::Long(2), MettaValue::Long(20)]);
}

#[test]
fn test_nondeterministic_cartesian_product() {
    // Integration test: nondeterministic evaluation using lazy Cartesian product
    // (= (a) 1)
    // (= (a) 2)
    // (= (b) 10)
    // (= (b) 20)
    // !(+ (a) (b))
    // Expected: [11, 21, 12, 22]

    let mut env = Environment::new();

    // Add rules for (a) -> 1 and (a) -> 2
    env.add_rule(Rule::new(
    MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
    MettaValue::Long(1),
));
    env.add_rule(Rule::new(
    MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
    MettaValue::Long(2),
));

    // Add rules for (b) -> 10 and (b) -> 20
    env.add_rule(Rule::new(
    MettaValue::SExpr(vec![MettaValue::Atom("b".to_string())]),
    MettaValue::Long(10),
));
    env.add_rule(Rule::new(
    MettaValue::SExpr(vec![MettaValue::Atom("b".to_string())]),
    MettaValue::Long(20),
));

    // Evaluate (+ (a) (b))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
        MettaValue::SExpr(vec![MettaValue::Atom("b".to_string())]),
    ]);

    let (results, _) = eval(expr, env);

    // Should have 4 results: 1+10=11, 1+20=21, 2+10=12, 2+20=22
    assert_eq!(results.len(), 4);

    let mut result_values: Vec<i64> = results
        .iter()
        .filter_map(|v| match v {
            MettaValue::Long(n) => Some(*n),
            _ => None,
        })
        .collect();
    result_values.sort();

    assert_eq!(result_values, vec![11, 12, 21, 22]);
}
