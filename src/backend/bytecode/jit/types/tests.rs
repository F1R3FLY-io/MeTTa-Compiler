//! Tests for JIT types.

use super::*;
use crate::backend::models::MettaValue;

#[test]
fn test_nan_boxing_long() {
    // Test positive integers
    let v = JitValue::from_long(42);
    assert!(v.is_long());
    assert!(!v.is_bool());
    assert_eq!(v.as_long(), 42);

    // Test zero
    let v = JitValue::from_long(0);
    assert!(v.is_long());
    assert_eq!(v.as_long(), 0);

    // Test negative integers
    let v = JitValue::from_long(-123);
    assert!(v.is_long());
    assert_eq!(v.as_long(), -123);

    // Test max 48-bit positive
    let max_48 = (1i64 << 47) - 1;
    let v = JitValue::from_long(max_48);
    assert!(v.is_long());
    assert_eq!(v.as_long(), max_48);

    // Test min 48-bit negative
    let min_48 = -(1i64 << 47);
    let v = JitValue::from_long(min_48);
    assert!(v.is_long());
    assert_eq!(v.as_long(), min_48);
}

#[test]
fn test_nan_boxing_bool() {
    let t = JitValue::from_bool(true);
    assert!(t.is_bool());
    assert!(!t.is_long());
    assert!(t.as_bool());

    let f = JitValue::from_bool(false);
    assert!(f.is_bool());
    assert!(!f.as_bool());
}

#[test]
fn test_nan_boxing_nil_unit() {
    let nil = JitValue::nil();
    assert!(nil.is_nil());
    assert!(!nil.is_unit());

    let unit = JitValue::unit();
    assert!(unit.is_unit());
    assert!(!unit.is_nil());
}

#[test]
fn test_nan_boxing_constants() {
    assert!(JitValue::TRUE.is_bool());
    assert!(JitValue::TRUE.as_bool());

    assert!(JitValue::FALSE.is_bool());
    assert!(!JitValue::FALSE.as_bool());

    assert!(JitValue::NIL.is_nil());
    assert!(JitValue::UNIT.is_unit());

    assert!(JitValue::ZERO.is_long());
    assert_eq!(JitValue::ZERO.as_long(), 0);

    assert!(JitValue::ONE.is_long());
    assert_eq!(JitValue::ONE.as_long(), 1);
}

#[test]
fn test_try_from_metta() {
    // Long
    let v = JitValue::try_from_metta(&MettaValue::Long(42));
    assert!(v.is_some());
    assert_eq!(v.unwrap().as_long(), 42);

    // Bool
    let v = JitValue::try_from_metta(&MettaValue::Bool(true));
    assert!(v.is_some());
    assert!(v.unwrap().as_bool());

    // Nil
    let v = JitValue::try_from_metta(&MettaValue::Nil);
    assert!(v.is_some());
    assert!(v.unwrap().is_nil());

    // Unit
    let v = JitValue::try_from_metta(&MettaValue::Unit);
    assert!(v.is_some());
    assert!(v.unwrap().is_unit());
}

#[test]
fn test_to_metta_roundtrip() {
    // Long
    let orig = MettaValue::Long(42);
    let jit = JitValue::try_from_metta(&orig).unwrap();
    let back = unsafe { jit.to_metta() };
    assert_eq!(back, orig);

    // Bool
    let orig = MettaValue::Bool(true);
    let jit = JitValue::try_from_metta(&orig).unwrap();
    let back = unsafe { jit.to_metta() };
    assert_eq!(back, orig);

    // Nil
    let orig = MettaValue::Nil;
    let jit = JitValue::try_from_metta(&orig).unwrap();
    let back = unsafe { jit.to_metta() };
    assert_eq!(back, orig);
}

#[test]
fn test_tag_extraction() {
    assert_eq!(JitValue::from_long(42).tag(), TAG_LONG);
    assert_eq!(JitValue::from_bool(true).tag(), TAG_BOOL);
    assert_eq!(JitValue::nil().tag(), TAG_NIL);
    assert_eq!(JitValue::unit().tag(), TAG_UNIT);
}
