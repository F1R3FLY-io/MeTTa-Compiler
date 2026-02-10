//! Tests for grounded operations.

use super::*;
use crate::backend::models::MettaValueInner;

// Mock eval function for testing
fn mock_eval(value: MettaValue, env: HeapEnvironment) -> (Vec<MettaValue>, HeapEnvironment) {
    // Just return the value as-is (no evaluation)
    (vec![value], env)
}

// Mock eval function that propagates errors
fn mock_eval_with_error(
    value: MettaValue,
    env: HeapEnvironment,
) -> (Vec<MettaValue>, HeapEnvironment) {
    if let MettaValueInner::Atom(name) = value.inner() {
        if name == "error_expr" {
            return (
                vec![MettaValue::Error(
                    "test error".to_string(),
                    MettaValue::Unit(),
                )],
                env,
            );
        }
    }
    (vec![value], env)
}

#[test]
fn test_add_op() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(2), MettaValue::Long(3)];
    let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Long(5));
}

#[test]
fn test_add_float() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(2.5), MettaValue::Float(3.5)];
    let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 6.0).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_comparison_less() {
    let less = LessOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(2), MettaValue::Long(3)];
    let result = less.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_logical_and_short_circuit() {
    let and = AndOp;
    let env = HeapEnvironment::default();

    // false AND <anything> should return false without evaluating second arg
    let args = vec![
        MettaValue::Bool(false),
        MettaValue::Atom("error".to_string()),
    ];
    let result = and.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_equality() {
    let eq = EqualOp;
    let env = HeapEnvironment::default();

    // Test Nil == ()
    let args = vec![MettaValue::Unit(), MettaValue::SExpr(vec![])];
    let result = eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_division_by_zero() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(10), MettaValue::Long(0)];
    let result = div.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Arithmetic(_))));
}

#[test]
fn test_incorrect_arity() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(1)];
    let result = add.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::IncorrectArgument(_))));
}

#[test]
fn test_type_error_on_type_mismatch() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![
        MettaValue::Long(1),
        MettaValue::Atom("not-a-number".to_string()),
    ];
    let result = add.execute_raw(&args, &env, &mock_eval);

    // Type mismatch should return a Runtime error, not NoReduce
    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

// ==========================================================================
// Additional Branch Coverage Tests - Arithmetic
// ==========================================================================

#[test]
fn test_add_integer_overflow() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(i64::MAX), MettaValue::Long(1)];
    let result = add.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

#[test]
fn test_add_mixed_long_float() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(2), MettaValue::Float(3.5)];
    let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 5.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_add_mixed_float_long() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(2.5), MettaValue::Long(3)];
    let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 5.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_add_error_propagation_first_arg() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![
        MettaValue::Atom("error_expr".to_string()),
        MettaValue::Long(5),
    ];
    let result = add.execute_raw(&args, &env, &mock_eval_with_error).unwrap();

    assert_eq!(result.len(), 1);
    assert!(matches!(result[0].0.inner(), MettaValueInner::Error(_, _)));
}

#[test]
fn test_add_error_propagation_second_arg() {
    let add = AddOp;
    let env = HeapEnvironment::default();

    let args = vec![
        MettaValue::Long(5),
        MettaValue::Atom("error_expr".to_string()),
    ];
    let result = add.execute_raw(&args, &env, &mock_eval_with_error).unwrap();

    assert_eq!(result.len(), 1);
    assert!(matches!(result[0].0.inner(), MettaValueInner::Error(_, _)));
}

#[test]
fn test_sub_integer_overflow() {
    let sub = SubOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(i64::MIN), MettaValue::Long(1)];
    let result = sub.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

#[test]
fn test_sub_float() {
    let sub = SubOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(5.5), MettaValue::Float(2.5)];
    let result = sub.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 3.0).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_sub_mixed_long_float() {
    let sub = SubOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(5), MettaValue::Float(2.5)];
    let result = sub.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 2.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_sub_mixed_float_long() {
    let sub = SubOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(5.5), MettaValue::Long(2)];
    let result = sub.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 3.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_sub_type_error() {
    let sub = SubOp;
    let env = HeapEnvironment::default();

    let args = vec![
        MettaValue::Atom("not-a-number".to_string()),
        MettaValue::Long(1),
    ];
    let result = sub.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

#[test]
fn test_mul_integer_overflow() {
    let mul = MulOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(i64::MAX), MettaValue::Long(2)];
    let result = mul.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

#[test]
fn test_mul_float() {
    let mul = MulOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(2.5), MettaValue::Float(4.0)];
    let result = mul.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 10.0).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_mul_mixed_long_float() {
    let mul = MulOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(3), MettaValue::Float(2.5)];
    let result = mul.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 7.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_mul_mixed_float_long() {
    let mul = MulOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(2.5), MettaValue::Long(3)];
    let result = mul.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 7.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_mul_type_error() {
    let mul = MulOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(true), MettaValue::Long(1)];
    let result = mul.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

#[test]
fn test_div_float() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(10.0), MettaValue::Float(4.0)];
    let result = div.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 2.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_div_float_by_zero() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(10.0), MettaValue::Float(0.0)];
    let result = div.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Arithmetic(_))));
}

#[test]
fn test_div_mixed_long_float() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(10), MettaValue::Float(4.0)];
    let result = div.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 2.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_div_mixed_long_float_by_zero() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(10), MettaValue::Float(0.0)];
    let result = div.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Arithmetic(_))));
}

#[test]
fn test_div_mixed_float_long() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(10.0), MettaValue::Long(4)];
    let result = div.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    if let MettaValueInner::Float(f) = result[0].0.inner() {
        assert!((f - 2.5).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float");
    }
}

#[test]
fn test_div_mixed_float_long_by_zero() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(10.0), MettaValue::Long(0)];
    let result = div.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Arithmetic(_))));
}

#[test]
fn test_div_type_error() {
    let div = DivOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::String("test".to_string()), MettaValue::Long(1)];
    let result = div.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

#[test]
fn test_mod_by_zero() {
    let mod_op = ModOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(10), MettaValue::Long(0)];
    let result = mod_op.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Arithmetic(_))));
}

#[test]
fn test_mod_normal() {
    let mod_op = ModOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(10), MettaValue::Long(3)];
    let result = mod_op.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Long(1));
}

#[test]
fn test_mod_type_error() {
    let mod_op = ModOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Float(10.0), MettaValue::Long(3)];
    let result = mod_op.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

#[test]
fn test_mod_second_arg_type_error() {
    let mod_op = ModOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(10), MettaValue::Float(3.0)];
    let result = mod_op.execute_raw(&args, &env, &mock_eval);

    assert!(matches!(result, Err(ExecError::Runtime(_))));
}

// ==========================================================================
// Additional Branch Coverage Tests - Comparisons
// ==========================================================================

#[test]
fn test_less_false() {
    let less = LessOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(5), MettaValue::Long(3)];
    let result = less.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_less_equal_values() {
    let less = LessOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(3), MettaValue::Long(3)];
    let result = less.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_less_eq_true() {
    let less_eq = LessEqOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(3), MettaValue::Long(3)];
    let result = less_eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_less_eq_false() {
    let less_eq = LessEqOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(5), MettaValue::Long(3)];
    let result = less_eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_greater_true() {
    let greater = GreaterOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(5), MettaValue::Long(3)];
    let result = greater.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_greater_false() {
    let greater = GreaterOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(3), MettaValue::Long(5)];
    let result = greater.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_greater_eq_true() {
    let greater_eq = GreaterEqOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(5), MettaValue::Long(5)];
    let result = greater_eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_greater_eq_false() {
    let greater_eq = GreaterEqOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(3), MettaValue::Long(5)];
    let result = greater_eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_not_equal_true() {
    let not_eq = NotEqualOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(3), MettaValue::Long(5)];
    let result = not_eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_not_equal_false() {
    let not_eq = NotEqualOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(5), MettaValue::Long(5)];
    let result = not_eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_equality_different_types() {
    let eq = EqualOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Long(5), MettaValue::Float(5.0)];
    let result = eq.execute_raw(&args, &env, &mock_eval).unwrap();

    // Different types should be unequal (Long vs Float)
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_equality_atoms() {
    let eq = EqualOp;
    let env = HeapEnvironment::default();

    let args = vec![
        MettaValue::Atom("test".to_string()),
        MettaValue::Atom("test".to_string()),
    ];
    let result = eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_equality_bools() {
    let eq = EqualOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(true), MettaValue::Bool(true)];
    let result = eq.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

// ==========================================================================
// Additional Branch Coverage Tests - Logical Operations
// ==========================================================================

#[test]
fn test_and_true_true() {
    let and = AndOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(true), MettaValue::Bool(true)];
    let result = and.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_and_true_false() {
    let and = AndOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(true), MettaValue::Bool(false)];
    let result = and.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_or_false_false() {
    let or = OrOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(false), MettaValue::Bool(false)];
    let result = or.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_or_true_false() {
    let or = OrOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(true), MettaValue::Bool(false)];
    let result = or.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_or_short_circuit() {
    let or = OrOp;
    let env = HeapEnvironment::default();

    // true OR <anything> should return true without evaluating second arg
    let args = vec![
        MettaValue::Bool(true),
        MettaValue::Atom("error".to_string()),
    ];
    let result = or.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

#[test]
fn test_not_true() {
    let not = NotOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(true)];
    let result = not.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(false));
}

#[test]
fn test_not_false() {
    let not = NotOp;
    let env = HeapEnvironment::default();

    let args = vec![MettaValue::Bool(false)];
    let result = not.execute_raw(&args, &env, &mock_eval).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, MettaValue::Bool(true));
}

// ==========================================================================
// Additional Branch Coverage Tests - Registry
// ==========================================================================

#[test]
fn test_registry_new() {
    let registry = GroundedRegistry::new();
    assert!(registry.get("+").is_none());
}

#[test]
fn test_registry_with_standard_ops() {
    let registry = GroundedRegistry::with_standard_ops();
    assert!(registry.get("+").is_some());
    assert!(registry.get("-").is_some());
    assert!(registry.get("*").is_some());
    assert!(registry.get("/").is_some());
    assert!(registry.get("%").is_some());
    assert!(registry.get("<").is_some());
    assert!(registry.get("<=").is_some());
    assert!(registry.get(">").is_some());
    assert!(registry.get(">=").is_some());
    assert!(registry.get("==").is_some());
    assert!(registry.get("!=").is_some());
    assert!(registry.get("and").is_some());
    assert!(registry.get("or").is_some());
    assert!(registry.get("not").is_some());
}

#[test]
fn test_registry_default() {
    let registry = GroundedRegistry::default();
    assert!(registry.get("+").is_some());
}

#[test]
fn test_registry_clone() {
    let registry = GroundedRegistry::with_standard_ops();
    let cloned = registry.clone();
    assert!(cloned.get("+").is_some());
}

#[test]
fn test_registry_tco_new() {
    let registry = GroundedRegistryTCO::new();
    assert!(registry.get("+").is_none());
}

#[test]
fn test_registry_tco_with_standard_ops() {
    let registry = GroundedRegistryTCO::with_standard_ops();
    assert!(registry.get("+").is_some());
    assert!(registry.get("-").is_some());
    assert!(registry.get("*").is_some());
    assert!(registry.get("/").is_some());
    assert!(registry.get("%").is_some());
}

#[test]
fn test_registry_tco_default() {
    let registry = GroundedRegistryTCO::default();
    assert!(registry.get("+").is_some());
}

#[test]
fn test_registry_tco_clone() {
    let registry = GroundedRegistryTCO::with_standard_ops();
    let cloned = registry.clone();
    assert!(cloned.get("+").is_some());
}

// ==========================================================================
// Additional Branch Coverage Tests - Error Display
// ==========================================================================

#[test]
fn test_exec_error_display_no_reduce() {
    let err = ExecError::NoReduce;
    assert_eq!(format!("{}", err), "NoReduce");
}

#[test]
fn test_exec_error_display_runtime() {
    let err = ExecError::Runtime("test error".to_string());
    assert_eq!(format!("{}", err), "Runtime error: test error");
}

#[test]
fn test_exec_error_display_arithmetic() {
    let err = ExecError::Arithmetic("division by zero".to_string());
    assert_eq!(format!("{}", err), "Arithmetic error: division by zero");
}

#[test]
fn test_exec_error_display_incorrect_argument() {
    let err = ExecError::IncorrectArgument("wrong arity".to_string());
    assert_eq!(format!("{}", err), "Incorrect argument: wrong arity");
}

// ==========================================================================
// Additional Branch Coverage Tests - Helper Functions
// ==========================================================================

#[test]
fn test_friendly_type_name_all_types() {
    assert_eq!(friendly_type_name(&MettaValue::Long(1)), "Number (integer)");
    assert_eq!(friendly_type_name(&MettaValue::Float(1.0)), "Number (float)");
    assert_eq!(friendly_type_name(&MettaValue::Bool(true)), "Bool");
    assert_eq!(
        friendly_type_name(&MettaValue::String("test".to_string())),
        "String"
    );
    assert_eq!(
        friendly_type_name(&MettaValue::Atom("test".to_string())),
        "Symbol"
    );
    assert_eq!(
        friendly_type_name(&MettaValue::SExpr(vec![])),
        "Expression"
    );
    // Unit type name is "Expression" (matches MeTTa HE where () is an expression)
    assert_eq!(friendly_type_name(&MettaValue::Unit()), "Expression");
    assert_eq!(
        friendly_type_name(&MettaValue::Error("err".to_string(), MettaValue::Unit())),
        "Error"
    );
    assert_eq!(
        friendly_type_name(&MettaValue::Type(MettaValue::Unit())),
        "Type"
    );
    assert_eq!(
        friendly_type_name(&MettaValue::Conjunction(vec![])),
        "Conjunction"
    );
    assert_eq!(friendly_type_name(&MettaValue::State(1)), "State");
    assert_eq!(friendly_type_name(&MettaValue::Empty()), "Empty");
}

#[test]
fn test_find_error_with_error() {
    let results = vec![
        MettaValue::Long(1),
        MettaValue::Error("test".to_string(), MettaValue::Unit()),
        MettaValue::Long(2),
    ];
    let error = find_error(&results);
    assert!(error.is_some());
    assert!(matches!(error.unwrap().inner(), MettaValueInner::Error(_, _)));
}

#[test]
fn test_find_error_no_error() {
    let results = vec![MettaValue::Long(1), MettaValue::Long(2)];
    let error = find_error(&results);
    assert!(error.is_none());
}

#[test]
fn test_find_error_empty() {
    let results: Vec<MettaValue> = vec![];
    let error = find_error(&results);
    assert!(error.is_none());
}
