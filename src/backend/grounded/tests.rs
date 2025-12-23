//! Tests for grounded operations.

use super::*;

// Mock eval function for testing
fn mock_eval(value: MettaValue, env: Environment) -> (Vec<MettaValue>, Environment) {
    // Just return the value as-is (no evaluation)
    (vec![value], env)
}

#[test]
fn test_add_op() {
    let add = AddOp;
    let env = Environment::new();

    let args = vec![MettaValue::Long(2), MettaValue::Long(3)];
    let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Long(5));
}

#[test]
fn test_add_float() {
    let add = AddOp;
    let env = Environment::new();

    let args = vec![MettaValue::Float(2.5), MettaValue::Float(3.5)];
    let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValue::Float(f) = result[0].0 {
        assert!((f - 6.0).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_comparison_less() {
    let less = LessOp;
    let env = Environment::new();

    let args = vec![MettaValue::Long(2), MettaValue::Long(3)];
    let result = less.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_logical_and_short_circuit() {
    let and = AndOp;
    let env = Environment::new();

    // false AND <anything> should return false without evaluating second arg
    let args = vec![MettaValue::Bool(false), MettaValue::Atom("error".to_string())];
    let result = and.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_equality() {
    let eq = EqualOp;
    let env = Environment::new();

    // Test Nil == ()
    let args = vec![MettaValue::Nil, MettaValue::SExpr(vec![])];
    let result = eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_division_by_zero() {
    let div = DivOp;
    let env = Environment::new();

    let args = vec![MettaValue::Long(10), MettaValue::Long(0)];
    let result = div.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Arithmetic(_))));
}

#[test]
fn test_incorrect_arity() {
    let add = AddOp;
    let env = Environment::new();

    let args = vec![MettaValue::Long(1)];
    let result = add.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::IncorrectArgument(_))));
}

#[test]
fn test_type_error_on_type_mismatch() {
    let add = AddOp;
    let env = Environment::new();

    let args = vec![
        MettaValue::Long(1),
        MettaValue::Atom("not-a-number".to_string()),
    ];
    let result = add.execute_raw(&args, &env, &mock_eval);

    // Type mismatch should return a Runtime error, not NoReduce
    assert!(matches!(result, Err(ExecError::Runtime(_))));
}
