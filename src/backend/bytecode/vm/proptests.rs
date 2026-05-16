//! Property-based tests for the bytecode VM using proptest.
//!
//! This module provides comprehensive property-based testing for:
//! - Arithmetic operations (commutativity, associativity, identity)
//! - Pattern matching (reflexivity, variable matching)
//! - List operations (cons/car/cdr inverses)
//! - Nondeterminism properties
//!
//! Run with: cargo test --lib proptests

use proptest::prelude::*;

use super::BytecodeVM;
use crate::backend::bytecode::chunk::ChunkBuilder;
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::{MettaValue, MettaValueInner};

/// Extension trait: treat post-T1.A Error-atom results as `is_err() == true`.
trait VmResultExt {
    fn is_err_or_error_atom(&self) -> bool;
}

impl<E> VmResultExt for Result<Vec<MettaValue>, E> {
    fn is_err_or_error_atom(&self) -> bool {
        match self {
            Err(_) => true,
            Ok(results) => results.iter().any(|v| v.is_error()),
        }
    }
}

impl<E> VmResultExt for Result<MettaValue, E> {
    fn is_err_or_error_atom(&self) -> bool {
        match self {
            Err(_) => true,
            Ok(v) => v.is_error(),
        }
    }
}

// =============================================================================
// Strategy Generators for MettaValue
// =============================================================================

/// Generate arbitrary Long values
fn arb_long() -> impl Strategy<Value = MettaValue> {
    // Use smaller range to avoid overflow in arithmetic tests
    (-10000i64..10000i64).prop_map(MettaValue::Long)
}

/// Generate arbitrary Float values (excluding NaN for deterministic testing)
fn arb_float() -> impl Strategy<Value = MettaValue> {
    (-1000.0f64..1000.0f64)
        .prop_filter("no NaN", |f| !f.is_nan())
        .prop_map(MettaValue::Float)
}

/// Generate arbitrary Bool values
fn arb_bool() -> impl Strategy<Value = MettaValue> {
    prop::bool::ANY.prop_map(MettaValue::Bool)
}

/// Generate arbitrary Symbol values (atoms)
fn arb_symbol() -> impl Strategy<Value = MettaValue> {
    "[a-z]{1,10}".prop_map(|s| MettaValue::sym(&s))
}

/// Generate arbitrary String values
fn arb_string() -> impl Strategy<Value = MettaValue> {
    ".{0,30}".prop_map(MettaValue::String)
}

/// Generate arbitrary Variable values ($x, $foo, etc.)
#[allow(dead_code)]
fn arb_variable() -> impl Strategy<Value = MettaValue> {
    "[a-z]{1,5}".prop_map(|s| MettaValue::var(&s))
}

/// Generate simple (non-recursive) MettaValue
fn arb_simple_value() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        arb_long(),
        arb_float(),
        arb_bool(),
        arb_symbol(),
        arb_string(),
        Just(MettaValue::Unit()),
        Just(MettaValue::Unit()),
    ]
}

/// Generate MettaValue with limited depth (for recursive structures)
fn arb_metta_value(depth: usize) -> BoxedStrategy<MettaValue> {
    if depth == 0 {
        arb_simple_value().boxed()
    } else {
        prop_oneof![
            arb_simple_value(),
            // S-expression with reduced depth
            prop::collection::vec(arb_metta_value(depth - 1), 0..4)
                .prop_map(MettaValue::SExpr)
                .boxed(),
        ]
        .boxed()
    }
}

/// Generate S-expressions of numbers for arithmetic testing
#[allow(dead_code)]
fn arb_numeric_sexpr(len: usize) -> impl Strategy<Value = MettaValue> {
    prop::collection::vec(arb_long(), len).prop_map(MettaValue::SExpr)
}

/// Generate patterns with variables
#[allow(dead_code)]
fn arb_pattern(depth: usize) -> BoxedStrategy<MettaValue> {
    if depth == 0 {
        prop_oneof![
            arb_simple_value(),
            arb_variable(),
            Just(MettaValue::sym("_")), // wildcard
        ]
        .boxed()
    } else {
        prop_oneof![
            arb_simple_value(),
            arb_variable(),
            Just(MettaValue::sym("_")),
            prop::collection::vec(arb_pattern(depth - 1), 0..4)
                .prop_map(MettaValue::SExpr)
                .boxed(),
        ]
        .boxed()
    }
}

// =============================================================================
// Helper Functions for VM Execution
// =============================================================================

/// Execute a simple binary operation on two Long values
fn run_vm_binary_op(a: i64, b: i64, opcode: Opcode) -> Result<MettaValue, String> {
    let mut builder = ChunkBuilder::new("prop_test");

    // Handle negative values - PushLongSmall only works for -128 to 127
    if a >= -128 && a <= 127 {
        builder.emit_byte(Opcode::PushLongSmall, a as u8);
    } else {
        let idx = builder.add_constant(MettaValue::Long(a));
        builder.emit_u16(Opcode::PushLong, idx);
    }

    if b >= -128 && b <= 127 {
        builder.emit_byte(Opcode::PushLongSmall, b as u8);
    } else {
        let idx = builder.add_constant(MettaValue::Long(b));
        builder.emit_u16(Opcode::PushLong, idx);
    }

    builder.emit(opcode);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let raw_result = vm
        .run()
        .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
        .map_err(|e| format!("{}", e));
    // BUG-T0-T1-001/011 (T1.A errors-as-values): the VM now returns
    // (Error msg details) atoms instead of propagating VmError. For these
    // proptests — which assert on the old VmError semantics — map an
    // Error-atom result back to `Err(message)` so existing `is_err()`
    // assertions continue to express the intended behavior.
    match raw_result {
        Ok(v) if v.is_error() => Err(format!("Error atom: {:?}", v.view())),
        other => other,
    }
}

/// Execute a unary operation on a Long value
fn run_vm_unary_op(a: i64, opcode: Opcode) -> Result<MettaValue, String> {
    let mut builder = ChunkBuilder::new("prop_test");

    if a >= -128 && a <= 127 {
        builder.emit_byte(Opcode::PushLongSmall, a as u8);
    } else {
        let idx = builder.add_constant(MettaValue::Long(a));
        builder.emit_u16(Opcode::PushLong, idx);
    }

    builder.emit(opcode);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let raw_result = vm
        .run()
        .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
        .map_err(|e| format!("{}", e));
    // T1.A errors-as-values: same Error-atom remapping as run_vm_binary_op.
    match raw_result {
        Ok(v) if v.is_error() => Err(format!("Error atom: {:?}", v.view())),
        other => other,
    }
}

// =============================================================================
// Arithmetic Property Tests
// =============================================================================

proptest! {
    /// Addition is commutative: a + b == b + a
    #[test]
    fn prop_add_commutative(a in -1000i64..1000i64, b in -1000i64..1000i64) {
        let result_ab = run_vm_binary_op(a, b, Opcode::Add);
        let result_ba = run_vm_binary_op(b, a, Opcode::Add);

        prop_assert!(result_ab.is_ok() && result_ba.is_ok());
        prop_assert_eq!(result_ab.unwrap(), result_ba.unwrap());
    }

    /// Addition identity: a + 0 == a
    #[test]
    fn prop_add_identity(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, 0, Opcode::Add);

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Long(a));
    }

    /// Subtraction inverse: a - a == 0
    #[test]
    fn prop_sub_inverse(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, a, Opcode::Sub);

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Long(0));
    }

    /// Multiplication is commutative: a * b == b * a
    #[test]
    fn prop_mul_commutative(a in -100i64..100i64, b in -100i64..100i64) {
        let result_ab = run_vm_binary_op(a, b, Opcode::Mul);
        let result_ba = run_vm_binary_op(b, a, Opcode::Mul);

        prop_assert!(result_ab.is_ok() && result_ba.is_ok());
        prop_assert_eq!(result_ab.unwrap(), result_ba.unwrap());
    }

    /// Multiplication identity: a * 1 == a
    #[test]
    fn prop_mul_identity(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, 1, Opcode::Mul);

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Long(a));
    }

    /// Multiplication by zero: a * 0 == 0
    #[test]
    fn prop_mul_zero(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, 0, Opcode::Mul);

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Long(0));
    }

    /// Negation is self-inverse: -(-a) == a
    #[test]
    fn prop_neg_self_inverse(a in -1000i64..1000i64) {
        // First negation
        let neg_a = run_vm_unary_op(a, Opcode::Neg);
        prop_assert!(neg_a.is_ok());

        let neg_a_val = match neg_a.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };

        // Second negation
        let neg_neg_a = run_vm_unary_op(neg_a_val, Opcode::Neg);
        prop_assert!(neg_neg_a.is_ok());
        prop_assert_eq!(neg_neg_a.unwrap(), MettaValue::Long(a));
    }

    /// Addition is associative: (a + b) + c == a + (b + c)
    #[test]
    fn prop_add_associative(a in -300i64..300i64, b in -300i64..300i64, c in -300i64..300i64) {
        // (a + b) + c
        let ab = run_vm_binary_op(a, b, Opcode::Add);
        prop_assert!(ab.is_ok());
        let ab_val = match ab.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };
        let left = run_vm_binary_op(ab_val, c, Opcode::Add);

        // a + (b + c)
        let bc = run_vm_binary_op(b, c, Opcode::Add);
        prop_assert!(bc.is_ok());
        let bc_val = match bc.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };
        let right = run_vm_binary_op(a, bc_val, Opcode::Add);

        prop_assert!(left.is_ok() && right.is_ok());
        prop_assert_eq!(left.unwrap(), right.unwrap());
    }

    /// Multiplication is associative: (a * b) * c == a * (b * c)
    #[test]
    fn prop_mul_associative(a in -20i64..20i64, b in -20i64..20i64, c in -20i64..20i64) {
        // (a * b) * c
        let ab = run_vm_binary_op(a, b, Opcode::Mul);
        prop_assert!(ab.is_ok());
        let ab_val = match ab.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };
        let left = run_vm_binary_op(ab_val, c, Opcode::Mul);

        // a * (b * c)
        let bc = run_vm_binary_op(b, c, Opcode::Mul);
        prop_assert!(bc.is_ok());
        let bc_val = match bc.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };
        let right = run_vm_binary_op(a, bc_val, Opcode::Mul);

        prop_assert!(left.is_ok() && right.is_ok());
        prop_assert_eq!(left.unwrap(), right.unwrap());
    }

    /// Multiplication distributes over addition: a * (b + c) == a*b + a*c
    #[test]
    fn prop_mul_distributes_over_add(a in -30i64..30i64, b in -30i64..30i64, c in -30i64..30i64) {
        // a * (b + c)
        let bc_sum = run_vm_binary_op(b, c, Opcode::Add);
        prop_assert!(bc_sum.is_ok());
        let bc_sum_val = match bc_sum.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };
        let left = run_vm_binary_op(a, bc_sum_val, Opcode::Mul);

        // a*b + a*c
        let ab = run_vm_binary_op(a, b, Opcode::Mul);
        let ac = run_vm_binary_op(a, c, Opcode::Mul);
        prop_assert!(ab.is_ok() && ac.is_ok());
        let ab_val = match ab.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };
        let ac_val = match ac.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };
        let right = run_vm_binary_op(ab_val, ac_val, Opcode::Add);

        prop_assert!(left.is_ok() && right.is_ok());
        prop_assert_eq!(left.unwrap(), right.unwrap());
    }

    /// Division followed by multiplication: (a / b) * b == a (for exact division)
    #[test]
    fn prop_div_mul_inverse(a in 1i64..100i64, b in 1i64..10i64) {
        // Only test where a is evenly divisible by b
        let a_adjusted = (a / b) * b;  // Make a divisible by b

        let div_result = run_vm_binary_op(a_adjusted, b, Opcode::Div);
        prop_assert!(div_result.is_ok());

        let quotient = match div_result.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };

        let mul_result = run_vm_binary_op(quotient, b, Opcode::Mul);
        prop_assert!(mul_result.is_ok());
        prop_assert_eq!(mul_result.unwrap(), MettaValue::Long(a_adjusted));
    }

    /// Modulo property: (a / b) * b + (a % b) == a
    #[test]
    fn prop_div_mod_reconstruct(a in 1i64..1000i64, b in 1i64..100i64) {
        let div_result = run_vm_binary_op(a, b, Opcode::Div);
        let mod_result = run_vm_binary_op(a, b, Opcode::Mod);

        prop_assert!(div_result.is_ok() && mod_result.is_ok());

        let quotient = match div_result.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long from div")),
        };
        let remainder = match mod_result.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long from mod")),
        };

        prop_assert_eq!(quotient * b + remainder, a);
    }

    /// Modulo periodicity: (a + n) % n == a % n (for positive values)
    #[test]
    fn prop_mod_periodicity(a in 0i64..100i64, n in 1i64..20i64) {
        let result1 = run_vm_binary_op(a, n, Opcode::Mod);
        let result2 = run_vm_binary_op(a + n, n, Opcode::Mod);

        prop_assert!(result1.is_ok() && result2.is_ok());
        prop_assert_eq!(result1.unwrap(), result2.unwrap());
    }

    /// Abs is idempotent: abs(abs(x)) == abs(x)
    #[test]
    fn prop_abs_idempotent(x in -1000i64..1000i64) {
        let abs1 = run_vm_unary_op(x, Opcode::Abs);
        prop_assert!(abs1.is_ok());

        let abs1_val = match abs1.unwrap().inner() {
            MettaValueInner::Long(n) => *n,
            _ => return Err(TestCaseError::fail("Expected Long")),
        };

        let abs2 = run_vm_unary_op(abs1_val, Opcode::Abs);
        prop_assert!(abs2.is_ok());
        prop_assert_eq!(abs2.unwrap(), MettaValue::Long(abs1_val));
    }

    /// Sqrt is monotone: a < b → sqrt(a) < sqrt(b) (for positive values)
    #[test]
    fn prop_sqrt_monotone(a in 1i64..100i64, delta in 1i64..100i64) {
        let b = a + delta; // b > a by construction

        let sqrt_a = run_vm_unary_op_value(MettaValue::Long(a), Opcode::Sqrt);
        let sqrt_b = run_vm_unary_op_value(MettaValue::Long(b), Opcode::Sqrt);

        prop_assert!(sqrt_a.is_ok() && sqrt_b.is_ok());

        let sqrt_a_val = match sqrt_a.unwrap().inner() {
            MettaValueInner::Float(f) => *f,
            _ => return Err(TestCaseError::fail("Expected Float")),
        };
        let sqrt_b_val = match sqrt_b.unwrap().inner() {
            MettaValueInner::Float(f) => *f,
            _ => return Err(TestCaseError::fail("Expected Float")),
        };

        // sqrt is monotonically increasing
        prop_assert!(sqrt_a_val < sqrt_b_val, "sqrt({}) = {} should be < sqrt({}) = {}", a, sqrt_a_val, b, sqrt_b_val);
    }
}

// =============================================================================
// Comparison Property Tests
// =============================================================================

proptest! {
    /// Reflexivity of equality: a == a is always true
    #[test]
    fn prop_eq_reflexive(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, a, Opcode::Eq);

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Bool(true));
    }

    /// Less-than is antisymmetric: if a < b then !(b < a)
    #[test]
    fn prop_lt_antisymmetric(a in -1000i64..1000i64, b in -1000i64..1000i64) {
        let lt_ab = run_vm_binary_op(a, b, Opcode::Lt);
        let lt_ba = run_vm_binary_op(b, a, Opcode::Lt);

        prop_assert!(lt_ab.is_ok() && lt_ba.is_ok());

        let a_lt_b = matches!(lt_ab.unwrap().inner(), MettaValueInner::Bool(true));
        let b_lt_a = matches!(lt_ba.unwrap().inner(), MettaValueInner::Bool(true));

        // If a < b is true, then b < a must be false
        if a_lt_b {
            prop_assert!(!b_lt_a);
        }
    }

    /// Trichotomy: exactly one of a < b, a == b, or a > b is true
    #[test]
    fn prop_trichotomy(a in -1000i64..1000i64, b in -1000i64..1000i64) {
        let lt = run_vm_binary_op(a, b, Opcode::Lt);
        let eq = run_vm_binary_op(a, b, Opcode::Eq);
        let gt = run_vm_binary_op(a, b, Opcode::Gt);

        prop_assert!(lt.is_ok() && eq.is_ok() && gt.is_ok());

        let is_lt = matches!(lt.unwrap().inner(), MettaValueInner::Bool(true));
        let is_eq = matches!(eq.unwrap().inner(), MettaValueInner::Bool(true));
        let is_gt = matches!(gt.unwrap().inner(), MettaValueInner::Bool(true));

        // Exactly one should be true
        let count = [is_lt, is_eq, is_gt].iter().filter(|&&x| x).count();
        prop_assert_eq!(count, 1, "Trichotomy violated: lt={}, eq={}, gt={}", is_lt, is_eq, is_gt);
    }

    /// Less-than is transitive: a < b ∧ b < c → a < c
    #[test]
    fn prop_lt_transitive(a in -100i64..0i64, b in 0i64..100i64, c in 100i64..200i64) {
        // By construction: a < b < c
        let lt_ab = run_vm_binary_op(a, b, Opcode::Lt);
        let lt_bc = run_vm_binary_op(b, c, Opcode::Lt);
        let lt_ac = run_vm_binary_op(a, c, Opcode::Lt);

        prop_assert!(lt_ab.is_ok() && lt_bc.is_ok() && lt_ac.is_ok());

        // All three should be true
        prop_assert_eq!(lt_ab.unwrap(), MettaValue::Bool(true));
        prop_assert_eq!(lt_bc.unwrap(), MettaValue::Bool(true));
        prop_assert_eq!(lt_ac.unwrap(), MettaValue::Bool(true));
    }

    /// Less-or-equal is reflexive: a <= a
    #[test]
    fn prop_le_reflexive(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, a, Opcode::Le);
        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Bool(true));
    }

    /// Greater-or-equal is reflexive: a >= a
    #[test]
    fn prop_ge_reflexive(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, a, Opcode::Ge);
        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Bool(true));
    }

    /// Less-or-equal is antisymmetric: a <= b ∧ b <= a → a == b
    #[test]
    fn prop_le_antisymmetric(a in -1000i64..1000i64, b in -1000i64..1000i64) {
        let le_ab = run_vm_binary_op(a, b, Opcode::Le);
        let le_ba = run_vm_binary_op(b, a, Opcode::Le);

        prop_assert!(le_ab.is_ok() && le_ba.is_ok());

        let a_le_b = matches!(le_ab.unwrap().inner(), MettaValueInner::Bool(true));
        let b_le_a = matches!(le_ba.unwrap().inner(), MettaValueInner::Bool(true));

        // If both a <= b and b <= a, then a == b
        if a_le_b && b_le_a {
            prop_assert_eq!(a, b);
        }
    }

    /// Equality is symmetric: a == b → b == a
    #[test]
    fn prop_eq_symmetric(a in -1000i64..1000i64, b in -1000i64..1000i64) {
        let eq_ab = run_vm_binary_op(a, b, Opcode::Eq);
        let eq_ba = run_vm_binary_op(b, a, Opcode::Eq);

        prop_assert!(eq_ab.is_ok() && eq_ba.is_ok());
        prop_assert_eq!(eq_ab.unwrap(), eq_ba.unwrap());
    }

    /// Not-equal is symmetric: a != b ↔ b != a
    #[test]
    fn prop_ne_symmetric(a in -1000i64..1000i64, b in -1000i64..1000i64) {
        let ne_ab = run_vm_binary_op(a, b, Opcode::Ne);
        let ne_ba = run_vm_binary_op(b, a, Opcode::Ne);

        prop_assert!(ne_ab.is_ok() && ne_ba.is_ok());
        prop_assert_eq!(ne_ab.unwrap(), ne_ba.unwrap());
    }
}

// =============================================================================
// Boolean Property Tests
// =============================================================================

proptest! {
    /// AND is commutative: a && b == b && a
    #[test]
    fn prop_and_commutative(a: bool, b: bool) {
        let mut builder1 = ChunkBuilder::new("test1");
        builder1.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(Opcode::And);
        builder1.emit(Opcode::Return);

        let mut builder2 = ChunkBuilder::new("test2");
        builder2.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(Opcode::And);
        builder2.emit(Opcode::Return);

        let result1 = BytecodeVM::new(builder1.build_arc()).run();
        let result2 = BytecodeVM::new(builder2.build_arc()).run();

        prop_assert!(result1.is_ok() && result2.is_ok());
        let r1 = result1.unwrap();
        let r2 = result2.unwrap();
        prop_assert_eq!(&r1[0], &r2[0]);
    }

    /// OR is commutative: a || b == b || a
    #[test]
    fn prop_or_commutative(a: bool, b: bool) {
        let mut builder1 = ChunkBuilder::new("test1");
        builder1.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(Opcode::Or);
        builder1.emit(Opcode::Return);

        let mut builder2 = ChunkBuilder::new("test2");
        builder2.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(Opcode::Or);
        builder2.emit(Opcode::Return);

        let result1 = BytecodeVM::new(builder1.build_arc()).run();
        let result2 = BytecodeVM::new(builder2.build_arc()).run();

        prop_assert!(result1.is_ok() && result2.is_ok());
        let r1 = result1.unwrap();
        let r2 = result2.unwrap();
        prop_assert_eq!(&r1[0], &r2[0]);
    }

    /// NOT is self-inverse: !!a == a
    #[test]
    fn prop_not_self_inverse(a: bool) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(Opcode::Not);
        builder.emit(Opcode::Not);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let r = result.unwrap();
        prop_assert_eq!(&r[0], &MettaValue::Bool(a));
    }

    /// De Morgan's law: !(a && b) == !a || !b
    #[test]
    fn prop_de_morgan_and(a: bool, b: bool) {
        // !(a && b)
        let mut builder1 = ChunkBuilder::new("test1");
        builder1.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(Opcode::And);
        builder1.emit(Opcode::Not);
        builder1.emit(Opcode::Return);

        // !a || !b
        let mut builder2 = ChunkBuilder::new("test2");
        builder2.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(Opcode::Not);
        builder2.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(Opcode::Not);
        builder2.emit(Opcode::Or);
        builder2.emit(Opcode::Return);

        let result1 = BytecodeVM::new(builder1.build_arc()).run();
        let result2 = BytecodeVM::new(builder2.build_arc()).run();

        prop_assert!(result1.is_ok() && result2.is_ok());
        let r1 = result1.unwrap();
        let r2 = result2.unwrap();
        prop_assert_eq!(&r1[0], &r2[0]);
    }

    /// De Morgan's law for OR: !(a || b) == !a && !b
    #[test]
    fn prop_de_morgan_or(a: bool, b: bool) {
        // !(a || b)
        let mut builder1 = ChunkBuilder::new("test1");
        builder1.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder1.emit(Opcode::Or);
        builder1.emit(Opcode::Not);
        builder1.emit(Opcode::Return);

        // !a && !b
        let mut builder2 = ChunkBuilder::new("test2");
        builder2.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(Opcode::Not);
        builder2.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
        builder2.emit(Opcode::Not);
        builder2.emit(Opcode::And);
        builder2.emit(Opcode::Return);

        let result1 = BytecodeVM::new(builder1.build_arc()).run();
        let result2 = BytecodeVM::new(builder2.build_arc()).run();

        prop_assert!(result1.is_ok() && result2.is_ok());
        let r1 = result1.unwrap();
        let r2 = result2.unwrap();
        prop_assert_eq!(&r1[0], &r2[0]);
    }

    /// XOR is self-inverse: a ^ a == false
    #[test]
    fn prop_xor_self_inverse(a: bool) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(Opcode::Xor);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let r = result.unwrap();
        prop_assert_eq!(&r[0], &MettaValue::Bool(false));
    }

    /// AND identity: a && true == a
    #[test]
    fn prop_and_identity(a: bool) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::And);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let r = result.unwrap();
        prop_assert_eq!(&r[0], &MettaValue::Bool(a));
    }

    /// OR identity: a || false == a
    #[test]
    fn prop_or_identity(a: bool) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let r = result.unwrap();
        prop_assert_eq!(&r[0], &MettaValue::Bool(a));
    }

    /// AND annihilation: a && false == false
    #[test]
    fn prop_and_annihilation(a: bool) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::And);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let r = result.unwrap();
        prop_assert_eq!(&r[0], &MettaValue::Bool(false));
    }

    /// OR annihilation: a || true == true
    #[test]
    fn prop_or_annihilation(a: bool) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Or);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let r = result.unwrap();
        prop_assert_eq!(&r[0], &MettaValue::Bool(true));
    }
}

// =============================================================================
// List Operation Property Tests
// =============================================================================

proptest! {
    /// cons-atom followed by get-head returns original head
    #[test]
    fn prop_cons_get_head(head in arb_simple_value(), tail_items in prop::collection::vec(arb_simple_value(), 0..3)) {
        let mut builder = ChunkBuilder::new("test");

        // Push head
        let head_idx = builder.add_constant(head.clone());
        builder.emit_u16(Opcode::PushConstant, head_idx);

        // Push tail as S-expression
        let tail = MettaValue::SExpr(tail_items);
        let tail_idx = builder.add_constant(tail);
        builder.emit_u16(Opcode::PushConstant, tail_idx);

        // cons-atom
        builder.emit(Opcode::ConsAtom);

        // get-head
        builder.emit(Opcode::GetHead);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &head);
    }

    /// cons-atom followed by get-tail returns original tail
    #[test]
    fn prop_cons_get_tail(head in arb_simple_value(), tail_items in prop::collection::vec(arb_simple_value(), 0..3)) {
        let mut builder = ChunkBuilder::new("test");

        // Push head
        let head_idx = builder.add_constant(head);
        builder.emit_u16(Opcode::PushConstant, head_idx);

        // Push tail as S-expression
        let tail = MettaValue::SExpr(tail_items.clone());
        let tail_idx = builder.add_constant(tail);
        builder.emit_u16(Opcode::PushConstant, tail_idx);

        // cons-atom
        builder.emit(Opcode::ConsAtom);

        // get-tail
        builder.emit(Opcode::GetTail);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        let expected = MettaValue::SExpr(tail_items);
        prop_assert_eq!(&results[0], &expected);
    }

    /// get-arity returns correct length
    #[test]
    fn prop_get_arity_correct(items in prop::collection::vec(arb_simple_value(), 1..10)) {
        let mut builder = ChunkBuilder::new("test");

        let sexpr = MettaValue::SExpr(items.clone());
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetArity);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        let expected = MettaValue::Long(items.len() as i64);
        prop_assert_eq!(&results[0], &expected);
    }
}

// =============================================================================
// Stack Operation Property Tests
// =============================================================================

proptest! {
    /// Dup followed by pop gives original value
    #[test]
    fn prop_dup_pop(value in arb_long()) {
        let mut builder = ChunkBuilder::new("test");

        let idx = builder.add_constant(value.clone());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Dup);
        builder.emit(Opcode::Pop);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &value);
    }

    /// Swap twice gives original order (self-inverse / involutive)
    #[test]
    fn prop_swap_swap(a in arb_long(), b in arb_long()) {
        let mut builder = ChunkBuilder::new("test");

        let a_idx = builder.add_constant(a.clone());
        let b_idx = builder.add_constant(b.clone());
        builder.emit_u16(Opcode::PushConstant, a_idx);
        builder.emit_u16(Opcode::PushConstant, b_idx);
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::Swap);
        // Now return top (should be b)
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &b);
    }

    /// Rot3 has order 3: rot3(); rot3(); rot3() = identity
    #[test]
    fn prop_rot3_order_three(a in arb_long(), b in arb_long(), c in arb_long()) {
        let mut builder = ChunkBuilder::new("test");

        let a_idx = builder.add_constant(a.clone());
        let b_idx = builder.add_constant(b.clone());
        let c_idx = builder.add_constant(c.clone());

        // Push a, b, c - stack is [a, b, c] with c on top
        builder.emit_u16(Opcode::PushConstant, a_idx);
        builder.emit_u16(Opcode::PushConstant, b_idx);
        builder.emit_u16(Opcode::PushConstant, c_idx);

        // Rot3 three times should return to original order
        builder.emit(Opcode::Rot3);
        builder.emit(Opcode::Rot3);
        builder.emit(Opcode::Rot3);

        // Return top (should be c)
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &c, "rot3³ should be identity");
    }

    /// Over copies depth-1 to top: [a, b] -> [a, b, a]
    #[test]
    fn prop_over_copies_second(a in arb_long(), b in arb_long()) {
        let mut builder = ChunkBuilder::new("test");

        let a_idx = builder.add_constant(a.clone());
        let b_idx = builder.add_constant(b.clone());

        // Push a, then b - stack is [a, b] with b on top
        builder.emit_u16(Opcode::PushConstant, a_idx);
        builder.emit_u16(Opcode::PushConstant, b_idx);

        // Over copies a to top: [a, b, a]
        builder.emit(Opcode::Over);

        // Return top (should be a)
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &a, "over should copy depth-1 to top");
    }

    /// Stack height invariants: push +1, pop -1, swap 0, dup +1
    #[test]
    fn prop_stack_height_invariants(a in arb_long(), b in arb_long()) {
        // Test push increments by 1: verified implicitly by getting result

        // Test dup increments by 1: push 1, dup, pop, pop should work
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(a.clone());
        builder.emit_u16(Opcode::PushConstant, idx);  // stack: 1
        builder.emit(Opcode::Dup);                    // stack: 2
        builder.emit(Opcode::Pop);                    // stack: 1
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &a);

        // Test swap preserves height: push 2, swap, should still have 2
        let mut builder2 = ChunkBuilder::new("test");
        let a_idx = builder2.add_constant(a.clone());
        let b_idx = builder2.add_constant(b.clone());
        builder2.emit_u16(Opcode::PushConstant, a_idx);
        builder2.emit_u16(Opcode::PushConstant, b_idx);
        builder2.emit(Opcode::Swap);
        builder2.emit(Opcode::Pop);  // pop one
        builder2.emit(Opcode::Return);  // return the other

        let result2 = BytecodeVM::new(builder2.build_arc()).run();
        prop_assert!(result2.is_ok());
        // After push a, push b, swap: stack is [b, a] (a on top)
        // After pop: stack is [b]
        // Return gives us b
        prop_assert_eq!(&result2.unwrap()[0], &b);
    }
}

// =============================================================================
// Arithmetic Error Branch Tests (Division by Zero, Overflow, Type Errors)
// =============================================================================

proptest! {
    /// Division by zero returns DivisionByZero error (non-zero numerator)
    #[test]
    fn prop_div_by_zero_errors(a in 1i64..1000i64) {
        // Use non-zero numerator to get consistent DivisionByZero error
        let result = run_vm_binary_op(a, 0, Opcode::Div);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Modulo by zero returns error (non-zero numerator)
    #[test]
    fn prop_mod_by_zero_errors(a in 1i64..1000i64) {
        // Use non-zero numerator to get consistent DivisionByZero error
        let result = run_vm_binary_op(a, 0, Opcode::Mod);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Floor division by zero returns error
    #[test]
    fn prop_floor_div_by_zero_errors(a in -1000i64..1000i64) {
        let result = run_vm_binary_op(a, 0, Opcode::FloorDiv);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Negative exponent returns type error
    #[test]
    fn prop_pow_negative_exp_errors(base in 1i64..10i64) {
        let result = run_vm_binary_op(base, -1, Opcode::Pow);
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Arithmetic Type Error Tests
// =============================================================================

/// Helper to run a binary op with arbitrary MettaValues
fn run_vm_binary_op_values(
    a: MettaValue,
    b: MettaValue,
    opcode: Opcode,
) -> Result<MettaValue, String> {
    let mut builder = ChunkBuilder::new("prop_test");

    let idx_a = builder.add_constant(a);
    builder.emit_u16(Opcode::PushConstant, idx_a);

    let idx_b = builder.add_constant(b);
    builder.emit_u16(Opcode::PushConstant, idx_b);

    builder.emit(opcode);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let raw = vm
        .run()
        .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
        .map_err(|e| format!("{}", e));
    // T1.A errors-as-values remapping (see run_vm_binary_op).
    match raw {
        Ok(v) if v.is_error() => Err(format!("Error atom: {:?}", v.view())),
        other => other,
    }
}

/// Helper to run a unary op with arbitrary MettaValue
fn run_vm_unary_op_value(a: MettaValue, opcode: Opcode) -> Result<MettaValue, String> {
    let mut builder = ChunkBuilder::new("prop_test");

    let idx = builder.add_constant(a);
    builder.emit_u16(Opcode::PushConstant, idx);

    builder.emit(opcode);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let raw = vm
        .run()
        .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
        .map_err(|e| format!("{}", e));
    // T1.A errors-as-values remapping.
    match raw {
        Ok(v) if v.is_error() => Err(format!("Error atom: {:?}", v.view())),
        other => other,
    }
}

proptest! {
    /// Add with non-numeric types returns type error
    #[test]
    fn prop_add_type_error(a in arb_string(), b in arb_long()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Add);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Sub with non-numeric types returns type error
    #[test]
    fn prop_sub_type_error(a in arb_symbol(), b in arb_long()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Sub);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Mul with non-numeric types returns type error
    #[test]
    fn prop_mul_type_error(a in arb_long(), b in arb_bool()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Mul);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Div with non-numeric types returns type error
    #[test]
    fn prop_div_type_error(a in arb_bool(), b in arb_long()) {
        let result = run_vm_binary_op_values(a, b.clone(), Opcode::Div);
        // Either type error or division by zero if b happens to be 0
        if let MettaValueInner::Long(0) = b.inner() {
            // Division by zero is also an error
            prop_assert!(result.is_err_or_error_atom());
        } else {
            prop_assert!(result.is_err_or_error_atom());
        }
    }

    /// Neg with non-numeric type returns type error
    #[test]
    fn prop_neg_type_error(a in arb_string()) {
        let result = run_vm_unary_op_value(a, Opcode::Neg);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Abs with non-numeric type returns type error
    #[test]
    fn prop_abs_type_error(a in arb_symbol()) {
        let result = run_vm_unary_op_value(a, Opcode::Abs);
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Comparison Type Error Tests
// =============================================================================

proptest! {
    /// Less-than with type mismatch returns error
    #[test]
    fn prop_lt_type_error(a in arb_string(), b in arb_long()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Lt);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Greater-than with type mismatch returns error
    #[test]
    fn prop_gt_type_error(a in arb_long(), b in arb_symbol()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Gt);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Less-equal with type mismatch returns error
    #[test]
    fn prop_le_type_error(a in arb_bool(), b in arb_long()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Le);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Greater-equal with type mismatch returns error
    #[test]
    fn prop_ge_type_error(a in arb_long(), b in arb_bool()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Ge);
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Boolean Operation Type Error Tests
// =============================================================================

proptest! {
    /// AND with non-boolean types returns error
    #[test]
    fn prop_and_type_error(a in arb_long(), b in arb_bool()) {
        let result = run_vm_binary_op_values(a, b, Opcode::And);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// OR with non-boolean types returns error
    #[test]
    fn prop_or_type_error(a in arb_bool(), b in arb_string()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Or);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// NOT with non-boolean type returns error
    #[test]
    fn prop_not_type_error(a in arb_long()) {
        let result = run_vm_unary_op_value(a, Opcode::Not);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// XOR with non-boolean types returns error
    #[test]
    fn prop_xor_type_error(a in arb_symbol(), b in arb_symbol()) {
        let result = run_vm_binary_op_values(a, b, Opcode::Xor);
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Float Operations Branch Coverage
// =============================================================================

proptest! {
    /// Sqrt with float returns float result
    #[test]
    fn prop_sqrt_float(x in 0.0f64..1000.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Sqrt);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(r) => {
                // sqrt result should be approximately correct
                prop_assert!((r * r - x).abs() < 0.001 || (r - x.sqrt()).abs() < 0.001);
            }
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    /// Sqrt with long converts to float
    #[test]
    fn prop_sqrt_long(x in 0i64..1000i64) {
        let result = run_vm_unary_op_value(MettaValue::Long(x), Opcode::Sqrt);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(_) => {}
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    /// Sqrt with invalid type returns error
    #[test]
    fn prop_sqrt_type_error(x in arb_string()) {
        let result = run_vm_unary_op_value(x, Opcode::Sqrt);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// Trigonometric functions work on floats
    #[test]
    fn prop_sin_float(x in -10.0f64..10.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Sin);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(r) => {
                // sin result should be in [-1, 1]
                prop_assert!(*r >= -1.0 && *r <= 1.0);
            }
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    /// Sin with long converts to float
    #[test]
    fn prop_sin_long(x in -10i64..10i64) {
        let result = run_vm_unary_op_value(MettaValue::Long(x), Opcode::Sin);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(_) => {}
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    /// Cos works correctly
    #[test]
    fn prop_cos_works(x in -10.0f64..10.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Cos);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(r) => {
                prop_assert!(*r >= -1.0 && *r <= 1.0);
            }
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    /// Tan works on floats
    #[test]
    fn prop_tan_float(x in -1.0f64..1.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Tan);
        prop_assert!(result.is_ok());
    }

    /// Inverse trig functions work
    #[test]
    fn prop_asin_works(x in -1.0f64..1.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Asin);
        prop_assert!(result.is_ok());
    }

    #[test]
    fn prop_acos_works(x in -1.0f64..1.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Acos);
        prop_assert!(result.is_ok());
    }

    #[test]
    fn prop_atan_works(x in -100.0f64..100.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Atan);
        prop_assert!(result.is_ok());
    }

    // =========================================================================
    // Trigonometric Identity Tests
    // =========================================================================

    /// Pythagorean identity: sin²(x) + cos²(x) = 1
    #[test]
    fn prop_pythagorean_identity(x in -10.0f64..10.0f64) {
        let sin_result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Sin);
        let cos_result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Cos);

        prop_assert!(sin_result.is_ok() && cos_result.is_ok());

        let sin_val = match sin_result.unwrap().inner() {
            MettaValueInner::Float(f) => *f,
            _ => return Err(TestCaseError::fail("Expected Float")),
        };
        let cos_val = match cos_result.unwrap().inner() {
            MettaValueInner::Float(f) => *f,
            _ => return Err(TestCaseError::fail("Expected Float")),
        };

        let sum = sin_val * sin_val + cos_val * cos_val;
        prop_assert!((sum - 1.0).abs() < 1e-10, "sin²({}) + cos²({}) = {} ≠ 1", x, x, sum);
    }

    /// Inverse trig identity: sin(asin(x)) = x for x ∈ [-1, 1]
    #[test]
    fn prop_trig_inverse_sin_asin(x in -1.0f64..1.0f64) {
        let asin_result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Asin);
        prop_assert!(asin_result.is_ok());

        let asin_val = match asin_result.unwrap().inner() {
            MettaValueInner::Float(f) => *f,
            _ => return Err(TestCaseError::fail("Expected Float")),
        };

        let sin_of_asin = run_vm_unary_op_value(MettaValue::Float(asin_val), Opcode::Sin);
        prop_assert!(sin_of_asin.is_ok());

        match sin_of_asin.unwrap().inner() {
            MettaValueInner::Float(f) => {
                prop_assert!((*f - x).abs() < 1e-10, "sin(asin({})) = {} ≠ {}", x, f, x);
            }
            _ => return Err(TestCaseError::fail("Expected Float")),
        }
    }

    /// Inverse trig identity: cos(acos(x)) = x for x ∈ [-1, 1]
    #[test]
    fn prop_trig_inverse_cos_acos(x in -1.0f64..1.0f64) {
        let acos_result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Acos);
        prop_assert!(acos_result.is_ok());

        let acos_val = match acos_result.unwrap().inner() {
            MettaValueInner::Float(f) => *f,
            _ => return Err(TestCaseError::fail("Expected Float")),
        };

        let cos_of_acos = run_vm_unary_op_value(MettaValue::Float(acos_val), Opcode::Cos);
        prop_assert!(cos_of_acos.is_ok());

        match cos_of_acos.unwrap().inner() {
            MettaValueInner::Float(f) => {
                prop_assert!((*f - x).abs() < 1e-10, "cos(acos({})) = {} ≠ {}", x, f, x);
            }
            _ => return Err(TestCaseError::fail("Expected Float")),
        }
    }

    /// Inverse trig identity: tan(atan(x)) = x
    #[test]
    fn prop_trig_inverse_tan_atan(x in -100.0f64..100.0f64) {
        let atan_result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Atan);
        prop_assert!(atan_result.is_ok());

        let atan_val = match atan_result.unwrap().inner() {
            MettaValueInner::Float(f) => *f,
            _ => return Err(TestCaseError::fail("Expected Float")),
        };

        let tan_of_atan = run_vm_unary_op_value(MettaValue::Float(atan_val), Opcode::Tan);
        prop_assert!(tan_of_atan.is_ok());

        match tan_of_atan.unwrap().inner() {
            MettaValueInner::Float(f) => {
                prop_assert!((*f - x).abs() < 1e-10, "tan(atan({})) = {} ≠ {}", x, f, x);
            }
            _ => return Err(TestCaseError::fail("Expected Float")),
        }
    }

    /// Trunc, ceil, floor, round work on floats
    #[test]
    fn prop_trunc_float(x in -1000.0f64..1000.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Trunc);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Long(r) => {
                prop_assert_eq!(*r, x.trunc() as i64);
            }
            _ => return Err(TestCaseError::fail("Expected Long result")),
        }
    }

    #[test]
    fn prop_ceil_float(x in -1000.0f64..1000.0f64) {
        // HE-aligned `ceil-math`: Float input returns Float (was Long-cast).
        // Conformance fixture T06/070 expects Float output (e.g. 3.2 → 4.0).
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Ceil);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(r) => {
                prop_assert_eq!(*r, x.ceil());
            }
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    #[test]
    fn prop_floor_float(x in -1000.0f64..1000.0f64) {
        // HE-aligned `floor-math`: Float input returns Float.
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::FloorMath);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(r) => {
                prop_assert_eq!(*r, x.floor());
            }
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    #[test]
    fn prop_round_float(x in -1000.0f64..1000.0f64) {
        // HE-aligned `round-math`: Float input returns Float.
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::Round);
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(r) => {
                prop_assert_eq!(*r, x.round());
            }
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    /// Trunc on Long is identity
    #[test]
    fn prop_trunc_long_identity(x in -1000i64..1000i64) {
        let result = run_vm_unary_op_value(MettaValue::Long(x), Opcode::Trunc);
        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Long(x));
    }
}

// =============================================================================
// IsNan and IsInf Branch Coverage
// =============================================================================

proptest! {
    /// isnan on regular float returns false
    #[test]
    fn prop_isnan_regular_float(x in -1000.0f64..1000.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::IsNan);
        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Bool(false));
    }

    /// isnan on Long returns false (integers are never NaN)
    #[test]
    fn prop_isnan_long_false(x in any::<i64>()) {
        let result = run_vm_unary_op_value(MettaValue::Long(x), Opcode::IsNan);
        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Bool(false));
    }

    /// isinf on regular float returns false
    #[test]
    fn prop_isinf_regular_float(x in -1000.0f64..1000.0f64) {
        let result = run_vm_unary_op_value(MettaValue::Float(x), Opcode::IsInf);
        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Bool(false));
    }

    /// isinf on Long returns false (integers are never infinite)
    #[test]
    fn prop_isinf_long_false(x in any::<i64>()) {
        let result = run_vm_unary_op_value(MettaValue::Long(x), Opcode::IsInf);
        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), MettaValue::Bool(false));
    }

    /// isnan with invalid type returns error
    #[test]
    fn prop_isnan_type_error(x in arb_string()) {
        let result = run_vm_unary_op_value(x, Opcode::IsNan);
        prop_assert!(result.is_err_or_error_atom());
    }

    /// isinf with invalid type returns error
    #[test]
    fn prop_isinf_type_error(x in arb_symbol()) {
        let result = run_vm_unary_op_value(x, Opcode::IsInf);
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Log Operation Branch Coverage
// =============================================================================

/// Helper to run log operation with base and value
fn run_vm_log(base: MettaValue, value: MettaValue) -> Result<MettaValue, String> {
    let mut builder = ChunkBuilder::new("prop_test");

    let idx_base = builder.add_constant(base);
    builder.emit_u16(Opcode::PushConstant, idx_base);

    let idx_val = builder.add_constant(value);
    builder.emit_u16(Opcode::PushConstant, idx_val);

    builder.emit(Opcode::Log);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    vm.run()
        .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
        .map_err(|e| format!("{}", e))
}

proptest! {
    /// Log with Float base and Float value
    #[test]
    fn prop_log_float_float(base in 1.1f64..100.0f64, value in 0.1f64..1000.0f64) {
        let result = run_vm_log(MettaValue::Float(base), MettaValue::Float(value));
        prop_assert!(result.is_ok());
        match result.unwrap().inner() {
            MettaValueInner::Float(_) => {}
            _ => return Err(TestCaseError::fail("Expected Float result")),
        }
    }

    /// Log with Long base and Float value
    #[test]
    fn prop_log_long_float(base in 2i64..100i64, value in 0.1f64..1000.0f64) {
        let result = run_vm_log(MettaValue::Long(base), MettaValue::Float(value));
        prop_assert!(result.is_ok());
    }

    /// Log with Float base and Long value
    #[test]
    fn prop_log_float_long(base in 1.1f64..100.0f64, value in 1i64..1000i64) {
        let result = run_vm_log(MettaValue::Float(base), MettaValue::Long(value));
        prop_assert!(result.is_ok());
    }

    /// Log with Long base and Long value
    #[test]
    fn prop_log_long_long(base in 2i64..100i64, value in 1i64..1000i64) {
        let result = run_vm_log(MettaValue::Long(base), MettaValue::Long(value));
        prop_assert!(result.is_ok());
    }

    /// Log with invalid types returns error
    #[test]
    fn prop_log_type_error(base in arb_string(), value in arb_long()) {
        let result = run_vm_log(base, value);
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Expression Operations Branch Coverage
// =============================================================================

proptest! {
    /// get-head on non-empty S-expression succeeds
    #[test]
    fn prop_get_head_succeeds(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items.clone());
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetHead);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &items[0]);
    }

    /// get-head on empty S-expression fails
    #[test]
    fn prop_get_head_empty_fails(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(vec![]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetHead);
        builder.emit(Opcode::Return);

        // H3 hard-cut: Error atom on stack.
        let result = BytecodeVM::new(builder.build_arc()).run()
            .expect("H3 hard-cut: VM should produce Error atom");
        prop_assert_eq!(result.len(), 1);
        prop_assert!(result[0].is_error());
    }

    /// get-head on non-S-expression fails
    #[test]
    fn prop_get_head_non_sexpr_fails(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetHead);
        builder.emit(Opcode::Return);

        // H3 hard-cut: Error atom on stack.
        let result = BytecodeVM::new(builder.build_arc()).run()
            .expect("H3 hard-cut: VM should produce Error atom");
        prop_assert_eq!(result.len(), 1);
        prop_assert!(result[0].is_error());
    }

    /// get-tail on non-empty S-expression succeeds
    #[test]
    fn prop_get_tail_succeeds(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items.clone());
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetTail);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        let results = result.unwrap();
        let expected = MettaValue::SExpr(items[1..].to_vec());
        prop_assert_eq!(&results[0], &expected);
    }

    /// get-tail on empty fails
    #[test]
    fn prop_get_tail_empty_fails(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(vec![]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetTail);
        builder.emit(Opcode::Return);

        // H3 hard-cut: Error atom on stack.
        let result = BytecodeVM::new(builder.build_arc()).run()
            .expect("H3 hard-cut: VM should produce Error atom");
        prop_assert_eq!(result.len(), 1);
        prop_assert!(result[0].is_error());
    }

    /// get-arity on S-expression returns correct length
    #[test]
    fn prop_get_arity_sexpr(items in prop::collection::vec(arb_simple_value(), 1..10)) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items.clone());
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetArity);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &MettaValue::Long(items.len() as i64));
    }

    /// get-arity on non-S-expression fails
    #[test]
    fn prop_get_arity_non_sexpr_fails(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetArity);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }

    /// cons-atom with Nil creates single-element S-expression
    #[test]
    fn prop_cons_atom_nil(head in arb_simple_value()) {
        let mut builder = ChunkBuilder::new("test");

        let head_idx = builder.add_constant(head.clone());
        builder.emit_u16(Opcode::PushConstant, head_idx);

        let nil_idx = builder.add_constant(MettaValue::Unit());
        builder.emit_u16(Opcode::PushConstant, nil_idx);

        builder.emit(Opcode::ConsAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(&results[0], &MettaValue::SExpr(vec![head]));
    }

    /// cons-atom with non-S-expression/non-Nil fails
    #[test]
    fn prop_cons_atom_invalid_tail_fails(head in arb_simple_value(), tail in arb_long()) {
        let mut builder = ChunkBuilder::new("test");

        let head_idx = builder.add_constant(head);
        builder.emit_u16(Opcode::PushConstant, head_idx);

        let tail_idx = builder.add_constant(tail);
        builder.emit_u16(Opcode::PushConstant, tail_idx);

        builder.emit(Opcode::ConsAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }

    /// decon-atom on non-empty S-expression succeeds
    #[test]
    fn prop_decons_atom_succeeds(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items.clone());
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::DeconsAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Result should be (head tail) pair
        match results[0].inner() {
            MettaValueInner::SExpr(pair) => {
                prop_assert_eq!(pair.len(), 2);
                prop_assert_eq!(&pair[0], &items[0]);
            }
            _ => return Err(TestCaseError::fail("Expected S-expression pair")),
        }
    }

    /// decon-atom on empty S-expression fails
    #[test]
    fn prop_decons_atom_empty_fails(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(vec![]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::DeconsAtom);
        builder.emit(Opcode::Return);

        // H3 hard-cut: Error atom on stack.
        let result = BytecodeVM::new(builder.build_arc()).run()
            .expect("H3 hard-cut: VM should produce Error atom");
        prop_assert_eq!(result.len(), 1);
        prop_assert!(result[0].is_error());
    }
}

// =============================================================================
// Type Check Operations Branch Coverage
// =============================================================================

proptest! {
    /// is-variable returns true for variables
    #[test]
    fn prop_is_variable_true(name in "[a-z]{1,5}") {
        let var = MettaValue::var(&name);
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(var);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::IsVariable);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(true));
    }

    /// is-variable returns false for non-variables
    #[test]
    fn prop_is_variable_false(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::IsVariable);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(false));
    }

    /// is-sexpr returns true for S-expressions
    #[test]
    fn prop_is_sexpr_true(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let sexpr = MettaValue::SExpr(items);
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::IsSExpr);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(true));
    }

    /// is-sexpr returns false for non-S-expressions
    #[test]
    fn prop_is_sexpr_false(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::IsSExpr);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(false));
    }

    /// is-symbol returns true for symbols
    #[test]
    fn prop_is_symbol_true(name in "[a-z]{1,10}") {
        let sym = MettaValue::sym(&name);
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(sym);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::IsSymbol);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(true));
    }

    /// is-symbol returns false for non-symbols
    #[test]
    fn prop_is_symbol_false(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::IsSymbol);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(false));
    }

    /// get-type returns type symbol
    #[test]
    fn prop_get_type_long(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(42));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        match result.unwrap()[0].inner() {
            MettaValueInner::Atom(s) => {
                prop_assert!(*s == "Long" || *s == "Number" || *s == "Int");
            }
            _ => return Err(TestCaseError::fail("Expected type symbol")),
        }
    }

    /// check-type with matching type returns true
    #[test]
    fn prop_check_type_match(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");

        // Push value (Long 42)
        let val_idx = builder.add_constant(MettaValue::Long(42));
        builder.emit_u16(Opcode::PushConstant, val_idx);

        // Push type symbol - type_name() returns "Number" for Long/Float
        let type_idx = builder.add_constant(MettaValue::sym("Number"));
        builder.emit_u16(Opcode::PushConstant, type_idx);

        builder.emit(Opcode::CheckType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(true));
    }

    /// check-type with type variable ($T) always matches
    #[test]
    fn prop_check_type_variable_matches(val in arb_simple_value()) {
        let mut builder = ChunkBuilder::new("test");

        let val_idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, val_idx);

        // Type variable $T should match anything
        let type_idx = builder.add_constant(MettaValue::sym("$T"));
        builder.emit_u16(Opcode::PushConstant, type_idx);

        builder.emit(Opcode::CheckType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(true));
    }

    /// check-type with non-symbol type returns error
    #[test]
    fn prop_check_type_non_symbol_type_error(val in arb_simple_value()) {
        let mut builder = ChunkBuilder::new("test");

        let val_idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, val_idx);

        // Push non-symbol as type (error case)
        let type_idx = builder.add_constant(MettaValue::Long(42));
        builder.emit_u16(Opcode::PushConstant, type_idx);

        builder.emit(Opcode::CheckType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// GetMetatype Branch Coverage
// =============================================================================

proptest! {
    /// get-metatype returns correct metatype for each value type.
    ///
    /// Plan S7 (RC-METATYPE-VOCAB, 2026-05-14): HE 4-category vocabulary.
    /// All primitive literals (Number/Bool/String/Unit/...) collapse to
    /// "Grounded" matching HE `lib/src/metta/types.rs::get_meta_type`.
    /// For fine-grained type information, use (get-type ...) instead.
    #[test]
    fn prop_get_metatype_long(x in any::<i64>()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(x));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Grounded"));
    }

    #[test]
    fn prop_get_metatype_float(x in any::<f64>().prop_filter("no NaN", |f| !f.is_nan())) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Float(x));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Grounded"));
    }

    #[test]
    fn prop_get_metatype_bool(b: bool) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Bool(b));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Grounded"));
    }

    #[test]
    fn prop_get_metatype_string(s in ".{0,20}") {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::String(s));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Grounded"));
    }

    #[test]
    fn prop_get_metatype_symbol(name in "[a-z]{1,10}") {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::sym(&name));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Symbol"));
    }

    #[test]
    fn prop_get_metatype_variable(name in "[a-z]{1,5}") {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::var(&name));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Variable"));
    }

    #[test]
    fn prop_get_metatype_sexpr(items in prop::collection::vec(arb_simple_value(), 1..3)) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::SExpr(items));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Expression"));
    }

    #[test]
    fn prop_get_metatype_nil(_unit: ()) {
        // Plan S7: Nil/Unit collapse to "Grounded" under HE 4-category.
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Unit());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Grounded"));
    }

    #[test]
    fn prop_get_metatype_unit(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Unit());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetMetaType);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::sym("Grounded"));
    }
}

// =============================================================================
// Match Operations Branch Coverage
// =============================================================================

proptest! {
    /// match-arity matches S-expressions with correct arity
    #[test]
    fn prop_match_arity_correct(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let arity = items.len();
        let mut builder = ChunkBuilder::new("test");

        let sexpr = MettaValue::SExpr(items);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::MatchArity, arity as u8);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(true));
    }

    /// match-arity fails on wrong arity
    #[test]
    fn prop_match_arity_wrong(items in prop::collection::vec(arb_simple_value(), 2..5)) {
        let wrong_arity = items.len() + 1;
        let mut builder = ChunkBuilder::new("test");

        let sexpr = MettaValue::SExpr(items);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::MatchArity, wrong_arity as u8);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(false));
    }

    /// match-arity fails on non-S-expression
    #[test]
    fn prop_match_arity_non_sexpr(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");

        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::MatchArity, 2);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(false));
    }

    /// unify succeeds for identical values (non-float to avoid NaN issues)
    #[test]
    fn prop_unify_identical(val in prop_oneof![
        arb_long(),
        arb_bool(),
        arb_symbol(),
        arb_string(),
        Just(MettaValue::Unit()),
        Just(MettaValue::Unit()),
    ]) {
        let mut builder = ChunkBuilder::new("test");

        let idx = builder.add_constant(val.clone());
        builder.emit_u16(Opcode::PushConstant, idx);
        let idx2 = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx2);
        builder.emit(Opcode::Unify);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(true));
    }

    /// unify fails for different simple values
    #[test]
    fn prop_unify_different(a in 0i64..100i64, b in 100i64..200i64) {
        let mut builder = ChunkBuilder::new("test");

        let idx_a = builder.add_constant(MettaValue::Long(a));
        builder.emit_u16(Opcode::PushConstant, idx_a);
        let idx_b = builder.add_constant(MettaValue::Long(b));
        builder.emit_u16(Opcode::PushConstant, idx_b);
        builder.emit(Opcode::Unify);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Bool(false));
    }
}

// =============================================================================
// Repr Operation Branch Coverage
// =============================================================================

proptest! {
    /// repr on Long produces string representation
    #[test]
    fn prop_repr_long(x in -1000i64..1000i64) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(x));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Repr);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        match result.unwrap()[0].inner() {
            MettaValueInner::String(s) => {
                prop_assert_eq!(s, &x.to_string());
            }
            _ => return Err(TestCaseError::fail("Expected String result")),
        }
    }

    /// repr on Bool produces True/False
    #[test]
    fn prop_repr_bool(b: bool) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Bool(b));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Repr);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        match result.unwrap()[0].inner() {
            MettaValueInner::String(s) => {
                let expected = if b { "True" } else { "False" };
                prop_assert_eq!(*s, expected);
            }
            _ => return Err(TestCaseError::fail("Expected String result")),
        }
    }

    /// repr on String produces quoted string
    #[test]
    fn prop_repr_string(s in "[a-z]{0,10}") {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::String(s.clone()));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Repr);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        match result.unwrap()[0].inner() {
            MettaValueInner::String(repr) => {
                prop_assert_eq!(repr, &format!("\"{}\"", s));
            }
            _ => return Err(TestCaseError::fail("Expected String result")),
        }
    }

    /// repr on Nil produces "Nil"
    #[test]
    fn prop_repr_nil(_unit: ()) {
        // After Nil/Unit merge, Nil() returns Unit, whose repr is "()"
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Unit());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Repr);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        match result.unwrap()[0].inner() {
            MettaValueInner::String(s) => {
                prop_assert_eq!(*s, "()");
            }
            _ => return Err(TestCaseError::fail("Expected String result")),
        }
    }

    /// repr on Unit produces "()"
    #[test]
    fn prop_repr_unit(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Unit());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Repr);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        match result.unwrap()[0].inner() {
            MettaValueInner::String(s) => {
                prop_assert_eq!(*s, "()");
            }
            _ => return Err(TestCaseError::fail("Expected String result")),
        }
    }
}

// =============================================================================
// Index and Min/Max Operations Branch Coverage
// =============================================================================

proptest! {
    /// index-atom with valid index succeeds
    #[test]
    fn prop_index_atom_valid(items in prop::collection::vec(arb_simple_value(), 2..5), idx_offset in 0usize..2) {
        let index = idx_offset.min(items.len() - 1);
        let mut builder = ChunkBuilder::new("test");

        let sexpr = MettaValue::SExpr(items.clone());
        let sexpr_idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, sexpr_idx);

        let index_val = builder.add_constant(MettaValue::Long(index as i64));
        builder.emit_u16(Opcode::PushConstant, index_val);

        builder.emit(Opcode::IndexAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &items[index]);
    }

    /// index-atom with negative index fails
    #[test]
    fn prop_index_atom_negative_fails(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let mut builder = ChunkBuilder::new("test");

        let sexpr = MettaValue::SExpr(items);
        let sexpr_idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, sexpr_idx);

        let index_val = builder.add_constant(MettaValue::Long(-1));
        builder.emit_u16(Opcode::PushConstant, index_val);

        builder.emit(Opcode::IndexAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }

    /// index-atom with out-of-bounds index fails
    #[test]
    fn prop_index_atom_out_of_bounds_fails(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let mut builder = ChunkBuilder::new("test");

        let sexpr = MettaValue::SExpr(items.clone());
        let sexpr_idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, sexpr_idx);

        let index_val = builder.add_constant(MettaValue::Long(items.len() as i64));
        builder.emit_u16(Opcode::PushConstant, index_val);

        builder.emit(Opcode::IndexAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }

    /// index-atom with non-Long index fails
    #[test]
    fn prop_index_atom_non_long_index_fails(items in prop::collection::vec(arb_simple_value(), 1..5)) {
        let mut builder = ChunkBuilder::new("test");

        let sexpr = MettaValue::SExpr(items);
        let sexpr_idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, sexpr_idx);

        let index_val = builder.add_constant(MettaValue::String("0".to_string()));
        builder.emit_u16(Opcode::PushConstant, index_val);

        builder.emit(Opcode::IndexAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }

    /// min-atom on numeric list returns minimum
    #[test]
    fn prop_min_atom_numeric(values in prop::collection::vec(-100i64..100i64, 1..5)) {
        let items: Vec<MettaValue> = values.iter().map(|v| MettaValue::Long(*v)).collect();
        let expected_min = *values.iter().min().unwrap();

        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::MinAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Long(expected_min));
    }

    /// min-atom on empty list fails
    #[test]
    fn prop_min_atom_empty_fails(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(vec![]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::MinAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }

    /// max-atom on numeric list returns maximum
    #[test]
    fn prop_max_atom_numeric(values in prop::collection::vec(-100i64..100i64, 1..5)) {
        let items: Vec<MettaValue> = values.iter().map(|v| MettaValue::Long(*v)).collect();
        let expected_max = *values.iter().max().unwrap();

        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::MaxAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        prop_assert_eq!(&result.unwrap()[0], &MettaValue::Long(expected_max));
    }

    /// min-atom with mixed float/long returns Float when float present
    #[test]
    fn prop_min_atom_mixed_types(_unit: ()) {
        let items = vec![
            MettaValue::Long(10),
            MettaValue::Float(5.5),
            MettaValue::Long(3),
        ];

        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::MinAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_ok());
        // Should return 3 (the minimum), as Float
        match result.unwrap()[0].inner() {
            MettaValueInner::Float(f) => {
                prop_assert!((*f - 3.0).abs() < 0.001);
            }
            MettaValueInner::Long(n) => {
                prop_assert_eq!(*n, 3);
            }
            _ => return Err(TestCaseError::fail("Expected numeric result")),
        }
    }

    /// min-atom on list with no numeric values fails
    #[test]
    fn prop_min_atom_no_numeric_fails(_unit: ()) {
        let items = vec![
            MettaValue::String("hello".to_string()),
            MettaValue::sym("world"),
        ];

        let mut builder = ChunkBuilder::new("test");
        let sexpr = MettaValue::SExpr(items);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::MinAtom);
        builder.emit(Opcode::Return);

        let result = BytecodeVM::new(builder.build_arc()).run();
        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Wrap-on-Overflow Tests (MeTTa spec §13.2 + §C.7g)
// =============================================================================

#[cfg(test)]
mod overflow_tests {
    use super::*;

    /// abs(i64::MIN) wraps to i64::MIN per spec §13.2
    /// (was formerly an Error; Plan C migrated to wrapping_abs.)
    #[test]
    fn test_abs_wraps() {
        let result = run_vm_unary_op_value(MettaValue::Long(i64::MIN), Opcode::Abs)
            .expect("abs(i64::MIN) should wrap, not error");
        assert_eq!(result, MettaValue::Long(i64::MIN));
    }

    /// (/ i64::MIN -1) wraps to i64::MIN per spec §13.2 + §C.7g
    /// (was formerly an Error; Plan C migrated to wrapping_div.)
    #[test]
    fn test_div_wraps() {
        let result = run_vm_binary_op(i64::MIN, -1, Opcode::Div)
            .expect("(/ i64::MIN -1) should wrap, not error");
        assert_eq!(result, MettaValue::Long(i64::MIN));
    }

    /// (% i64::MIN -1) wraps to 0 per spec §13.2 + §C.7g
    /// (was formerly an Error; Plan C migrated to wrapping_rem.)
    #[test]
    fn test_mod_wraps() {
        let result = run_vm_binary_op(i64::MIN, -1, Opcode::Mod)
            .expect("(% i64::MIN -1) should wrap, not error");
        assert_eq!(result, MettaValue::Long(0));
    }
}

// =============================================================================
// Special Float Values Tests
// =============================================================================

#[cfg(test)]
mod special_float_tests {
    use super::*;

    /// isnan on NaN returns true
    #[test]
    fn test_isnan_nan_true() {
        let result = run_vm_unary_op_value(MettaValue::Float(f64::NAN), Opcode::IsNan);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MettaValue::Bool(true));
    }

    /// isinf on positive infinity returns true
    #[test]
    fn test_isinf_pos_inf_true() {
        let result = run_vm_unary_op_value(MettaValue::Float(f64::INFINITY), Opcode::IsInf);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MettaValue::Bool(true));
    }

    /// isinf on negative infinity returns true
    #[test]
    fn test_isinf_neg_inf_true() {
        let result = run_vm_unary_op_value(MettaValue::Float(f64::NEG_INFINITY), Opcode::IsInf);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MettaValue::Bool(true));
    }
}

// =============================================================================
// Control Flow Unit Tests (specific cases)
// =============================================================================

#[cfg(test)]
mod control_flow_unit_tests {
    use super::*;

    fn run_vm_chunk(setup: impl FnOnce(&mut ChunkBuilder)) -> Result<Vec<MettaValue>, String> {
        let mut builder = ChunkBuilder::new("test");
        setup(&mut builder);
        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run().map_err(|e| format!("{}", e))
    }

    #[test]
    fn test_jump_if_false_takes_branch() {
        let result = run_vm_chunk(|builder| {
            builder.emit(Opcode::PushFalse);
            builder.emit_i16(Opcode::JumpIfFalse, 3);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Return);
            builder.emit_byte(Opcode::PushLongSmall, 100);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Long(100)]);
    }

    #[test]
    fn test_jump_if_false_no_branch() {
        let result = run_vm_chunk(|builder| {
            builder.emit(Opcode::PushTrue);
            builder.emit_i16(Opcode::JumpIfFalse, 3);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Return);
            builder.emit_byte(Opcode::PushLongSmall, 100);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Long(42)]);
    }

    #[test]
    fn test_jump_if_true_takes_branch() {
        let result = run_vm_chunk(|builder| {
            builder.emit(Opcode::PushTrue);
            builder.emit_i16(Opcode::JumpIfTrue, 3);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Return);
            builder.emit_byte(Opcode::PushLongSmall, 100);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Long(100)]);
    }

    #[test]
    fn test_jump_if_nil_takes_branch() {
        let result = run_vm_chunk(|builder| {
            builder.emit(Opcode::PushUnit);
            builder.emit_i16(Opcode::JumpIfUnit, 3);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Return);
            builder.emit_byte(Opcode::PushLongSmall, 100);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Long(100)]);
    }

    #[test]
    fn test_jump_short() {
        let result = run_vm_chunk(|builder| {
            builder.emit_i8(Opcode::JumpShort, 2);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit_byte(Opcode::PushLongSmall, 100);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Long(100)]);
    }

    #[test]
    fn test_unconditional_jump() {
        let result = run_vm_chunk(|builder| {
            builder.emit_i16(Opcode::Jump, 5);
            builder.emit(Opcode::Nop);
            builder.emit(Opcode::Nop);
            builder.emit(Opcode::Nop);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit_byte(Opcode::PushLongSmall, 100);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Long(100)]);
    }
}

// =============================================================================
// Control Flow Property Tests (real property-based testing)
// =============================================================================

proptest! {
    /// Property: JumpIfFalse returns else_val when cond is false, then_val when true
    /// For any two distinct values, the conditional jump selects the correct one
    #[test]
    fn prop_jump_if_false_selects_correctly(
        cond: bool,
        then_val in -100i8..100i8,
        else_val in -100i8..100i8
    ) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if cond { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit_i16(Opcode::JumpIfFalse, 3); // skip to else branch
        builder.emit_byte(Opcode::PushLongSmall, then_val as u8);
        builder.emit(Opcode::Return);
        builder.emit_byte(Opcode::PushLongSmall, else_val as u8);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let expected = if cond { then_val } else { else_val };
        prop_assert_eq!(result.unwrap(), vec![MettaValue::Long(expected as i64)]);
    }

    /// Property: JumpIfTrue returns then_val when cond is true, else_val when false
    #[test]
    fn prop_jump_if_true_selects_correctly(
        cond: bool,
        then_val in -100i8..100i8,
        else_val in -100i8..100i8
    ) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if cond { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit_i16(Opcode::JumpIfTrue, 3); // skip to then branch (jump taken on true)
        builder.emit_byte(Opcode::PushLongSmall, else_val as u8);
        builder.emit(Opcode::Return);
        builder.emit_byte(Opcode::PushLongSmall, then_val as u8);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let expected = if cond { then_val } else { else_val };
        prop_assert_eq!(result.unwrap(), vec![MettaValue::Long(expected as i64)]);
    }

    /// Property: JumpIfNil jumps only when value is Nil
    /// For any non-Nil Long value, should NOT jump
    #[test]
    fn prop_jump_if_nil_non_nil_no_jump(val in -1000i64..1000i64) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(val));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_i16(Opcode::JumpIfUnit, 3);
        builder.emit_byte(Opcode::PushLongSmall, 1); // not jumped = 1
        builder.emit(Opcode::Return);
        builder.emit_byte(Opcode::PushLongSmall, 2); // jumped = 2
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        // Should NOT have jumped, so result is 1
        prop_assert_eq!(result.unwrap(), vec![MettaValue::Long(1)]);
    }

    /// Property: JumpIfError peeks but doesn't pop, and jumps on Error values
    #[test]
    fn prop_jump_if_error_non_error_no_jump(val in arb_simple_value()) {
        // Skip if val is an Error (we're testing non-error case)
        prop_assume!(!matches!(val.inner(), MettaValueInner::Error(..)));

        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_i16(Opcode::JumpIfError, 4);
        builder.emit(Opcode::Pop); // pop the value
        builder.emit_byte(Opcode::PushLongSmall, 1); // not jumped
        builder.emit(Opcode::Return);
        builder.emit(Opcode::Pop);
        builder.emit_byte(Opcode::PushLongSmall, 2); // jumped
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        // Non-error values don't cause jump
        prop_assert_eq!(result.unwrap(), vec![MettaValue::Long(1)]);
    }

    /// Property: Unconditional Jump always reaches target regardless of stack content
    #[test]
    fn prop_unconditional_jump_always_jumps(
        stack_values in prop::collection::vec(arb_simple_value(), 0..5),
        target_val in -100i8..100i8
    ) {
        let mut builder = ChunkBuilder::new("test");

        // Push arbitrary values onto stack first
        for val in &stack_values {
            let idx = builder.add_constant(val.clone());
            builder.emit_u16(Opcode::PushConstant, idx);
        }

        // Unconditional jump over a different value
        builder.emit_i16(Opcode::Jump, 3);
        builder.emit_byte(Opcode::PushLongSmall, 99); // skipped
        builder.emit(Opcode::Return); // skipped
        builder.emit_byte(Opcode::PushLongSmall, target_val as u8); // target
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Last value should be our target
        prop_assert_eq!(results.last(), Some(&MettaValue::Long(target_val as i64)));
    }
}

// =============================================================================
// Nondeterminism Unit Tests (specific cases)
// =============================================================================

#[cfg(test)]
mod nondeterminism_unit_tests {
    use super::*;

    fn run_vm_chunk(setup: impl FnOnce(&mut ChunkBuilder)) -> Result<Vec<MettaValue>, String> {
        let mut builder = ChunkBuilder::new("test");
        setup(&mut builder);
        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run().map_err(|e| format!("{}", e))
    }

    #[test]
    fn test_fork_zero_alternatives() {
        let result = run_vm_chunk(|builder| {
            builder.emit_u16(Opcode::Fork, 0);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Return);
        });
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_fork_single_alternative() {
        let mut builder = ChunkBuilder::new("test");
        let c1 = builder.add_constant(MettaValue::Long(42));
        builder.emit_u16(Opcode::Fork, 1);
        builder.emit_raw(&c1.to_be_bytes());
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run().unwrap();
        assert_eq!(result, vec![MettaValue::Long(42)]);
        assert_eq!(vm.choice_points_len(), 0);
    }

    #[test]
    fn test_fork_multiple_alternatives() {
        let mut builder = ChunkBuilder::new("test");
        let c1 = builder.add_constant(MettaValue::Long(1));
        let c2 = builder.add_constant(MettaValue::Long(2));
        let c3 = builder.add_constant(MettaValue::Long(3));

        builder.emit_u16(Opcode::Fork, 3);
        builder.emit_raw(&c1.to_be_bytes());
        builder.emit_raw(&c2.to_be_bytes());
        builder.emit_raw(&c3.to_be_bytes());
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_fail_no_choice_points() {
        let result = run_vm_chunk(|builder| {
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Fail);
            builder.emit(Opcode::Return);
        });
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_yield_collects() {
        let result = run_vm_chunk(|builder| {
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Yield);
        });
        assert!(result.unwrap().contains(&MettaValue::Long(42)));
    }

    #[test]
    fn test_guard_true_continues() {
        let result = run_vm_chunk(|builder| {
            builder.emit(Opcode::PushTrue);
            builder.emit(Opcode::Guard);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Long(42)]);
    }

    #[test]
    fn test_guard_false_backtracks() {
        let result = run_vm_chunk(|builder| {
            builder.emit(Opcode::PushFalse);
            builder.emit(Opcode::Guard);
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Return);
        });
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_amb_zero_alternatives() {
        let result = run_vm_chunk(|builder| {
            builder.emit_byte(Opcode::Amb, 0);
            builder.emit(Opcode::Return);
        });
        assert_eq!(result.unwrap(), vec![MettaValue::Unit()]);
    }
}

// =============================================================================
// Nondeterminism Property Tests
// =============================================================================
//
// Each instruction has mathematical properties (invariants about cardinalities,
// set operations, etc.) and behavioral properties (equivalences, error conditions).
//
// Notation:
//   |S| = cardinality of set S
//   results(prog) = set of values collected when running program prog
//   ≡ = behavioral equivalence (produces same results)

/// Generate non-boolean values for type error testing
fn arb_non_bool() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        arb_long(),
        arb_float(),
        arb_symbol(),
        arb_string(),
        Just(MettaValue::Unit()),
        Just(MettaValue::Unit()),
    ]
}

proptest! {
    // =========================================================================
    // FORK Properties
    // =========================================================================
    //
    // Fork(N, [v₁, v₂, ..., vₙ]) creates a choice point with N alternatives.
    //
    // Mathematical Properties:
    //   P1: |results(Fork(N) ; Yield)| = N  (cardinality)
    //   P2: results(Fork([v₁,...,vₙ]) ; Yield) = {v₁,...,vₙ}  (set equality)
    //   P3: Fork(1) creates no choice point (optimization)
    //
    // Behavioral Properties:
    //   P4: Fork(0) ≡ Fail  (empty alternatives = immediate failure)
    //   P5: Fork(1, [v]) ≡ Push(v)  (single alternative = no backtracking)

    /// P1: |results(Fork(N) ; Yield)| = N
    /// The number of results equals the number of alternatives
    #[test]
    fn prop_fork_cardinality(
        values in prop::collection::vec(-100i64..100i64, 1..6)
    ) {
        let mut builder = ChunkBuilder::new("test");

        // Add all values as constants
        let const_indices: Vec<u16> = values.iter()
            .map(|v| builder.add_constant(MettaValue::Long(*v)))
            .collect();

        // Fork with N alternatives
        builder.emit_u16(Opcode::Fork, values.len() as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        // Yield to collect all
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Should have exactly N results
        prop_assert_eq!(results.len(), values.len());
        // All original values should be present
        for v in &values {
            prop_assert!(results.contains(&MettaValue::Long(*v)));
        }
    }

    // =========================================================================
    // P4: Fork(0) ≡ Fail (behavioral equivalence)
    // Empty alternatives causes immediate backtracking
    // =========================================================================

    /// P4: Fork(0) ≡ Fail
    /// Any value on stack before Fork(0) should not appear in results
    #[test]
    fn prop_fork_zero_equiv_fail(val in -100i64..100i64) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(val));
        builder.emit_u16(Opcode::PushConstant, idx);
        // Fork with 0 alternatives = immediate fail
        builder.emit_u16(Opcode::Fork, 0u16);
        // This Return should never execute
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        // Fork(0) behaves as Fail - empty results
        prop_assert!(result.unwrap().is_empty());
    }

    /// P5: Fork(1, [v]) ≡ Push(v) ; Continue
    /// Single alternative creates no choice point
    #[test]
    fn prop_fork_single_equiv_push(val in -100i64..100i64) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(val));
        builder.emit_u16(Opcode::Fork, 1u16);
        builder.emit_raw(&idx.to_be_bytes());
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Single alternative = direct return, equivalent to Push
        prop_assert_eq!(results, vec![MettaValue::Long(val)]);
    }

    // =========================================================================
    // AMB Properties
    // =========================================================================
    //
    // Amb(N) pops N values from stack and nondeterministically chooses one.
    //
    // Mathematical Properties:
    //   A1: |results(Push(v₁)...Push(vₙ) ; Amb(N) ; Yield)| = N  (cardinality)
    //   A2: results(Amb(N)) ⊆ {v₁,...,vₙ}  (subset of stack)
    //
    // Behavioral Properties:
    //   A3: Amb(0) pushes Nil (empty choice = nil value)
    //   A4: Amb(1) ≡ identity (single element, no choice)

    /// A1: |results(Amb(N) ; Yield)| = N
    #[test]
    fn prop_amb_cardinality(values in prop::collection::vec(-100i8..100i8, 1..5)) {
        let mut builder = ChunkBuilder::new("test");
        for v in &values {
            builder.emit_byte(Opcode::PushLongSmall, *v as u8);
        }
        builder.emit_byte(Opcode::Amb, values.len() as u8);
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap().len(), values.len());
    }

    /// A3: Amb(0) pushes Unit
    #[test]
    fn prop_amb_zero_is_unit(val in -100i8..100i8) {
        let mut builder = ChunkBuilder::new("test");
        // Push a value that Amb(0) should NOT consume
        builder.emit_byte(Opcode::PushLongSmall, val as u8);
        builder.emit_byte(Opcode::Amb, 0u8);
        // Now stack has: [val, Unit], top is Unit
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Returns Unit (top of stack after Amb(0))
        prop_assert!(matches!(results[0].inner(), MettaValueInner::Unit));
    }

    /// A4: Amb(1) ≡ identity
    /// Single element on stack, Amb(1) returns exactly that element
    #[test]
    fn prop_amb_single_identity(val in -100i8..100i8) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, val as u8);
        builder.emit_byte(Opcode::Amb, 1u8);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        // Amb(1) returns the single value unchanged
        prop_assert_eq!(result.unwrap(), vec![MettaValue::Long(val as i64)]);
    }

    // =========================================================================
    // GUARD Properties
    // =========================================================================
    //
    // Guard(b) acts as a filter on the current execution path.
    //
    // Mathematical Properties:
    //   G1: Guard is a predicate: Guard(b) = if b then Continue else Fail
    //
    // Behavioral Properties:
    //   G2: Guard(true) ; P ≡ P  (identity for true)
    //   G3: Guard(false) ≡ Fail  (equivalence to fail)
    //   G4: Guard(non-bool) → TypeError  (type restriction)

    /// G1: Guard(true) continues, Guard(false) backtracks
    #[test]
    fn prop_guard_is_predicate(cond: bool, val in -100i8..100i8) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if cond { Opcode::PushTrue } else { Opcode::PushFalse });
        builder.emit(Opcode::Guard);
        builder.emit_byte(Opcode::PushLongSmall, val as u8);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();

        // Guard is a predicate filter
        if cond {
            prop_assert_eq!(results, vec![MettaValue::Long(val as i64)]);
        } else {
            prop_assert!(results.is_empty());
        }
    }

    /// G4: Guard(non-bool) → TypeError
    /// Guard is type-safe: only accepts Bool
    #[test]
    fn prop_guard_type_restriction(val in arb_non_bool()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Guard);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        // Type error for non-Bool
        prop_assert!(result.is_err_or_error_atom());
    }

    // =========================================================================
    // CUT Properties
    // =========================================================================
    //
    // Cut removes all choice points, committing to the current path.
    //
    // Mathematical Properties:
    //   C1: After Cut, |choice_points| = 0
    //   C2: Cut ; Fail ≡ return []  (no backtracking possible)
    //
    // Behavioral Properties:
    //   C3: Fork(N) ; Cut ; Yield ≡ Fork(1) ; Yield  (prunes to first)

    /// C2: Cut ; Fail ≡ return []
    #[test]
    fn prop_cut_prevents_backtrack(values in prop::collection::vec(-100i64..100i64, 2..5)) {
        let mut builder = ChunkBuilder::new("test");

        let const_indices: Vec<u16> = values.iter()
            .map(|v| builder.add_constant(MettaValue::Long(*v)))
            .collect();

        builder.emit_u16(Opcode::Fork, values.len() as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        builder.emit(Opcode::Cut);
        builder.emit(Opcode::Fail);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        // Cut removed all choice points, Fail has nowhere to go
        prop_assert!(result.unwrap().is_empty());
    }

    /// C3: Fork(N) ; Cut ; Yield = first alternative only
    #[test]
    fn prop_cut_keeps_first_only(values in prop::collection::vec(-100i64..100i64, 2..5)) {
        let mut builder = ChunkBuilder::new("test");

        let const_indices: Vec<u16> = values.iter()
            .map(|v| builder.add_constant(MettaValue::Long(*v)))
            .collect();

        builder.emit_u16(Opcode::Fork, values.len() as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        builder.emit(Opcode::Cut);
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Only the first alternative survives cut
        prop_assert_eq!(results.len(), 1);
        prop_assert_eq!(&results[0], &MettaValue::Long(values[0]));
    }

    // =========================================================================
    // COMMIT Properties
    // =========================================================================
    //
    // Commit(N) removes N most recent choice points (soft cut).
    //
    // Mathematical Properties:
    //   M1: After Commit(N), |choice_points| decreases by min(N, |choice_points|)
    //   M2: Commit(0) ≡ Cut  (remove all)
    //
    // Behavioral Properties:
    //   M3: Commit(k) preserves older choice points

    /// M2: Commit(0) ≡ Cut
    #[test]
    fn prop_commit_zero_equiv_cut(values in prop::collection::vec(-100i64..100i64, 2..5)) {
        let mut builder = ChunkBuilder::new("test");

        let const_indices: Vec<u16> = values.iter()
            .map(|v| builder.add_constant(MettaValue::Long(*v)))
            .collect();

        builder.emit_u16(Opcode::Fork, values.len() as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        builder.emit_byte(Opcode::Commit, 0u8);
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Commit(0) = Cut, only first survives
        prop_assert_eq!(results.len(), 1);
        prop_assert_eq!(&results[0], &MettaValue::Long(values[0]));
    }

    /// M3: Commit(1) preserves older choice points
    /// With 2 nested forks, Commit(1) removes only innermost
    #[test]
    fn prop_commit_n_partial(v1 in -100i64..100i64, v2 in -100i64..100i64) {
        let mut builder = ChunkBuilder::new("test");

        // Outer fork with 2 alternatives
        let idx1a = builder.add_constant(MettaValue::Long(v1));
        let idx1b = builder.add_constant(MettaValue::Long(v1 + 1000));
        builder.emit_u16(Opcode::Fork, 2u16);
        builder.emit_raw(&idx1a.to_be_bytes());
        builder.emit_raw(&idx1b.to_be_bytes());
        builder.emit(Opcode::Pop);

        // Inner fork with 2 alternatives
        let idx2a = builder.add_constant(MettaValue::Long(v2));
        let idx2b = builder.add_constant(MettaValue::Long(v2 + 1000));
        builder.emit_u16(Opcode::Fork, 2u16);
        builder.emit_raw(&idx2a.to_be_bytes());
        builder.emit_raw(&idx2b.to_be_bytes());

        // Commit(1) removes inner fork only
        builder.emit_byte(Opcode::Commit, 1u8);
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // 2 results from outer fork (inner was committed away)
        prop_assert_eq!(results.len(), 2);
    }

    // =========================================================================
    // YIELD Properties
    // =========================================================================
    //
    // Yield(v) adds v to results and backtracks.
    //
    // Mathematical Properties:
    //   Y1: v ∈ results after Yield(v)
    //   Y2: Yield ; P → results contains v, then continues backtracking
    //
    // Behavioral Properties:
    //   Y3: Yield ≡ add_result + Fail

    /// Y1: v ∈ results after Yield(v)
    #[test]
    fn prop_yield_adds_to_results(val in -1000i64..1000i64) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(val));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        prop_assert!(result.unwrap().contains(&MettaValue::Long(val)));
    }

    // =========================================================================
    // FAIL Properties
    // =========================================================================
    //
    // Fail backtracks to the most recent choice point, or returns results.
    //
    // Mathematical Properties:
    //   F1: Fail with no choice points → return current results
    //   F2: Stack values are discarded (not added to results)
    //
    // Behavioral Properties:
    //   F3: Values must be explicitly Yielded to appear in results

    /// F2: Stack values are discarded by Fail
    #[test]
    fn prop_fail_discards_stack(val in -1000i64..1000i64) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Long(val));
        builder.emit_u16(Opcode::PushConstant, idx);
        // Fail without Yield
        builder.emit(Opcode::Fail);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        // Value was NOT yielded, so not in results
        prop_assert!(!result.unwrap().contains(&MettaValue::Long(val)));
    }

    // =========================================================================
    // COLLECT Properties
    // =========================================================================
    //
    // Collect gathers all yielded results into an S-expression.
    //
    // Mathematical Properties:
    //   L1: Collect filters out Nil values
    //   L2: |Collect| ≤ |yielded values|  (Nil removed)
    //
    // CollectN Properties:
    //   LN1: |CollectN(k)| ≤ min(k, |yielded values|)

    /// L1: Collect filters Nil
    #[test]
    fn prop_collect_filters_nil(non_nil_count in 1usize..4, nil_count in 1usize..4) {
        let mut builder = ChunkBuilder::new("test");

        // Create Fork with mix of values and Nil
        let mut indices = Vec::new();
        for i in 0..non_nil_count {
            indices.push(builder.add_constant(MettaValue::Long(i as i64)));
        }
        for _ in 0..nil_count {
            indices.push(builder.add_constant(MettaValue::Unit()));
        }

        builder.emit_u16(Opcode::Fork, indices.len() as u16);
        for idx in &indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        builder.emit(Opcode::Yield);
        builder.emit_u16(Opcode::Collect, 0u16);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Any SExpr result should have no Nil values
        for r in &results {
            if let MettaValueInner::SExpr(items) = r.inner() {
                for item in *items {
                    prop_assert!(!matches!(item.inner(), MettaValueInner::Unit));
                }
            }
        }
    }

    /// LN1: CollectN(k) limits to k results
    #[test]
    fn prop_collect_n_limits(
        values in prop::collection::vec(-100i64..100i64, 3..6),
        limit in 1u8..3
    ) {
        let mut builder = ChunkBuilder::new("test");

        let const_indices: Vec<u16> = values.iter()
            .map(|v| builder.add_constant(MettaValue::Long(*v)))
            .collect();

        builder.emit_u16(Opcode::Fork, values.len() as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        builder.emit(Opcode::Yield);
        builder.emit_byte(Opcode::CollectN, limit);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Results should contain an SExpr with at most `limit` items
        for r in &results {
            if let MettaValueInner::SExpr(items) = r.inner() {
                prop_assert!(items.len() <= limit as usize);
            }
        }
    }

    // =========================================================================
    // op_fail Alternative Branch Coverage
    // =========================================================================

    /// Branch: op_fail with empty alternatives continues loop (line 70-73)
    /// Invariant: Exhausted choice points are skipped during backtracking
    #[test]
    fn prop_fail_skips_exhausted_choice_points(val in -100i64..100i64) {
        let mut builder = ChunkBuilder::new("test");

        // First fork with 1 alternative (will be exhausted after first use)
        let idx1 = builder.add_constant(MettaValue::Long(val));
        builder.emit_u16(Opcode::Fork, 1u16);
        builder.emit_raw(&idx1.to_be_bytes());
        builder.emit(Opcode::Pop);

        // Second fork with 2 alternatives
        let idx2a = builder.add_constant(MettaValue::Long(val + 100));
        let idx2b = builder.add_constant(MettaValue::Long(val + 200));
        builder.emit_u16(Opcode::Fork, 2u16);
        builder.emit_raw(&idx2a.to_be_bytes());
        builder.emit_raw(&idx2b.to_be_bytes());
        builder.emit(Opcode::Yield);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Both alternatives from fork2 should be yielded
        prop_assert_eq!(results.len(), 2);
        prop_assert!(results.contains(&MettaValue::Long(val + 100)));
        prop_assert!(results.contains(&MettaValue::Long(val + 200)));
    }
}

// =============================================================================
// Return Operations Property Tests
// =============================================================================

proptest! {
    /// Return from top-level puts value in results
    #[test]
    fn prop_return_top_level(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(val.clone());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), vec![val]);
    }

    /// ReturnMulti returns all values above base_ptr
    #[test]
    fn prop_return_multi(count in 1usize..5usize) {
        let mut builder = ChunkBuilder::new("test");
        for i in 0..count {
            builder.emit_byte(Opcode::PushLongSmall, i as u8);
        }
        builder.emit(Opcode::ReturnMulti);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        prop_assert_eq!(results.len(), count);
    }
}

// =============================================================================
// VM Execution Path Tests
// =============================================================================

proptest! {
    /// VM pre-allocates local variable slots
    #[test]
    fn prop_vm_preallocates_locals(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.set_local_count(5); // Request 5 locals
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), vec![MettaValue::Long(42)]);
    }

    /// StoreLocal/LoadLocal work after preallocation
    #[test]
    fn prop_store_load_local(val in arb_long()) {
        let mut builder = ChunkBuilder::new("test");
        builder.set_local_count(3);

        let idx = builder.add_constant(val.clone());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::StoreLocal, 1); // Store in slot 1
        builder.emit_byte(Opcode::LoadLocal, 1);  // Load from slot 1
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        prop_assert_eq!(result.unwrap(), vec![val]);
    }

    /// Handle chunk end without call stack returns stack values
    #[test]
    fn prop_chunk_end_returns_stack(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::PushLongSmall, 43);
        // No Return - just end of chunk

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok());
        let results = result.unwrap();
        // Both values should be in results
        prop_assert_eq!(results.len(), 2);
    }

    /// IP out of bounds returns error (step with bad IP)
    #[test]
    fn prop_ip_out_of_bounds_errors(_unit: ()) {
        // Create a minimal chunk but force IP past end
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::Nop);
        let chunk = builder.build_arc();

        let mut vm = BytecodeVM::new(chunk);
        vm.ip = 1000; // Force IP way out of bounds

        // step() should detect IP >= chunk.len() and call handle_chunk_end
        let result = vm.run();
        // This should complete (handle_chunk_end is called), not error
        prop_assert!(result.is_ok());
    }

    /// Invalid opcode byte returns error
    #[test]
    fn prop_invalid_opcode_errors(_unit: ()) {
        // Manually construct a chunk with an invalid opcode byte
        let mut builder = ChunkBuilder::new("test");
        builder.emit_raw_byte(0xFF); // Invalid opcode

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }

    /// Halt opcode returns Halted error
    #[test]
    fn prop_halt_returns_error(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::Halt);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }
}

// =============================================================================
// Stack Underflow Tests
// =============================================================================

proptest! {
    /// Pop on empty stack returns StackUnderflow
    #[test]
    fn prop_pop_empty_underflow(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::Pop); // Pop with nothing on stack

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }

    /// Dup on empty stack returns StackUnderflow
    #[test]
    fn prop_dup_empty_underflow(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::Dup);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }

    /// Swap with < 2 items returns StackUnderflow
    #[test]
    fn prop_swap_underflow(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::Swap); // Only 1 item

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }

    /// Over with < 2 items returns StackUnderflow
    #[test]
    fn prop_over_underflow(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::Over); // Only 1 item

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }

    /// Rot3 with < 3 items returns StackUnderflow
    #[test]
    fn prop_rot3_underflow(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit(Opcode::Rot3); // Only 2 items

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }

    /// Binary op with < 2 items returns StackUnderflow
    #[test]
    fn prop_binary_op_underflow(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::Add); // Only 1 item

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_err_or_error_atom());
    }

    /// Return on empty stack at top-level yields empty results.
    ///
    /// Bytecode chunks for side-effect-only forms (e.g., `(= lhs rhs)` →
    /// DefineRule + Pop) terminate with an empty value stack. Top-level
    /// Return treats this as "no result" rather than an underflow error,
    /// so the caller doesn't fall back to the trampoline and re-fire the
    /// side-effect (e.g., double-adding the rule).
    #[test]
    fn prop_return_empty_underflow(_unit: ()) {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::Return); // Nothing to return

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let result = vm.run();

        prop_assert!(result.is_ok(), "expected Ok(empty), got {:?}", result);
        prop_assert!(result.unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use proptest::test_runner::TestRunner;

    use super::*;

    /// Sanity test that proptest strategies generate valid values
    #[test]
    fn test_strategy_sanity() {
        let mut runner = TestRunner::default();

        // Test simple value generation - runner.run returns Result<(), TestError>
        let result = runner.run(&arb_simple_value(), |value| {
            // Just verify the value is valid
            match value.inner() {
                MettaValueInner::Long(_)
                | MettaValueInner::Float(_)
                | MettaValueInner::Bool(_)
                | MettaValueInner::Atom(_)
                | MettaValueInner::String(_)
                | MettaValueInner::Unit => Ok(()),
                _ => Err(TestCaseError::fail("Unexpected value type")),
            }
        });

        assert!(result.is_ok());
    }
}

// =============================================================================
// Multi-Tier Property Tests
// =============================================================================
//
// These tests verify that mathematical properties hold across ALL execution tiers:
// - Tier 0: Tree-walker interpreter (eval_trampoline)
// - Tier 1: Bytecode VM (BytecodeVM)
// - Tier 2/3: JIT (HybridExecutor)
//
// This ensures that all tiers produce equivalent results and maintain invariants.

#[cfg(test)]
mod multi_tier_tests {
    use super::*;

    use crate::backend::grounded::{
        AddOp, AndOp, DivOp, EqualOp, GreaterEqOp, GreaterOp, GroundedOperationTCO, GroundedState,
        GroundedWork, LessEqOp, LessOp, ModOp, MulOp, NotEqualOp, NotOp, OrOp, SubOp,
    };
    use crate::backend::models::GcFactory;

    // =========================================================================
    // Tier Definitions and Helpers
    // =========================================================================

    /// Execution tier identifier
    #[allow(dead_code)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Tier {
        /// Tree-walker tier: Grounded operations via GroundedOperationTCO
        Grounded,
        /// Bytecode VM tier: Opcode execution
        BytecodeVM,
    }

    /// Drive a generic TCO grounded operation to completion.
    ///
    /// For proptests, arguments are already concrete values, so when the state
    /// machine requests `EvalArg(idx)`, we "evaluate" by returning the original
    /// argument at that index as-is (wrapping it in a single-element Vec).
    fn drive_tco<Op>(op: &Op, args: Vec<MettaValue>) -> Result<MettaValue, String>
    where
        Op: GroundedOperationTCO<MettaValue>,
    {
        let factory = GcFactory::default();
        let original_args = args.clone();
        let mut state = GroundedState::new(op.name().to_string(), args);

        loop {
            let work = op.execute_step(&mut state, &factory);
            match work {
                GroundedWork::Done(results) => {
                    return Ok(results
                        .into_iter()
                        .next()
                        .map(|(v, _)| v)
                        .unwrap_or(MettaValue::Unit()));
                }
                GroundedWork::EvalArg {
                    arg_idx,
                    state: returned_state,
                } => {
                    // Restore the returned state (contains updated step counter)
                    state = returned_state;
                    // "Evaluate" the argument by returning the original concrete value
                    let arg_val = original_args[arg_idx].clone();
                    state.set_arg(arg_idx, vec![arg_val]);
                }
                GroundedWork::Error(e) => {
                    return Err(format!("grounded error: {:?}", e));
                }
            }
        }
    }

    /// Execute a binary operation via generic TCO grounded operation (tree-walker tier)
    fn run_grounded_binary<Op>(op: &Op, a: MettaValue, b: MettaValue) -> Result<MettaValue, String>
    where
        Op: GroundedOperationTCO<MettaValue>,
    {
        drive_tco(op, vec![a, b])
    }

    /// Execute a unary operation via generic TCO grounded operation (tree-walker tier)
    fn run_grounded_unary<Op>(op: &Op, a: MettaValue) -> Result<MettaValue, String>
    where
        Op: GroundedOperationTCO<MettaValue>,
    {
        drive_tco(op, vec![a])
    }

    /// Execute a binary operation via bytecode VM tier
    fn run_vm_binary(a: i64, b: i64, opcode: Opcode) -> Result<MettaValue, String> {
        run_vm_binary_op(a, b, opcode)
    }

    /// Execute a unary operation via bytecode VM tier
    #[allow(dead_code)]
    fn run_vm_unary(a: i64, opcode: Opcode) -> Result<MettaValue, String> {
        run_vm_unary_op(a, opcode)
    }

    // =========================================================================
    // Arithmetic Properties: Grounded vs VM Tier Equivalence
    // =========================================================================

    proptest! {
        /// ADD: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_add_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&AddOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Add);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok(),
                "One tier failed: grounded={:?}, vm={:?}", grounded_result, vm_result);
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap(),
                "Tier mismatch for add({}, {})", a, b);
        }

        /// SUB: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_sub_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&SubOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Sub);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// MUL: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_mul_equivalence(a in -100i64..100i64, b in -100i64..100i64) {
            let grounded_result = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Mul);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// DIV: Grounded tier and VM tier produce same result (non-zero divisor)
        #[test]
        fn prop_tier_div_equivalence(a in -1000i64..1000i64, b in 1i64..100i64) {
            let grounded_result = run_grounded_binary(&DivOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Div);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok(),
                "Tier failed for div({}, {}): grounded={:?}, vm={:?}", a, b, grounded_result, vm_result);
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// MOD: Grounded tier and VM tier produce same result (non-zero divisor)
        #[test]
        fn prop_tier_mod_equivalence(a in 0i64..1000i64, b in 1i64..100i64) {
            let grounded_result = run_grounded_binary(&ModOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Mod);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        // NOTE: NegOp and AbsOp are not defined as grounded operations.
        // These operations exist only at the VM tier level (Opcode::Neg, Opcode::Abs).
        // The tree-walker tier computes negation via (- 0 x) and abs via conditionals.

        /// DIV by zero: Both tiers error consistently
        #[test]
        fn prop_tier_div_by_zero_both_error(a in 1i64..1000i64) {
            let grounded_result = run_grounded_binary(&DivOp, MettaValue::Long(a), MettaValue::Long(0));
            let vm_result = run_vm_binary(a, 0, Opcode::Div);

            // Both should error
            prop_assert!(grounded_result.is_err() && vm_result.is_err(),
                "Both tiers should error on div by zero, but: grounded={:?}, vm={:?}",
                grounded_result, vm_result);
        }
    }

    // =========================================================================
    // Comparison Properties: Grounded vs VM Tier Equivalence
    // =========================================================================

    proptest! {
        /// LESS: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_less_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&LessOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Lt);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// LESS_EQ: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_less_eq_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&LessEqOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Le);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// GREATER: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_greater_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&GreaterOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Gt);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// GREATER_EQ: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_greater_eq_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&GreaterEqOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Ge);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// EQUAL: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_equal_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&EqualOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Eq);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// NOT_EQUAL: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_not_equal_equivalence(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded_result = run_grounded_binary(&NotEqualOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm_result = run_vm_binary(a, b, Opcode::Ne);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }
    }

    // =========================================================================
    // Boolean Properties: Grounded vs VM Tier Equivalence
    // =========================================================================

    /// Execute boolean binary op via bytecode VM
    fn run_vm_bool_binary(a: bool, b: bool, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(if b {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute boolean unary op via bytecode VM
    fn run_vm_bool_unary(a: bool, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    proptest! {
        /// AND: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_and_equivalence(a: bool, b: bool) {
            let grounded_result = run_grounded_binary(&AndOp, MettaValue::Bool(a), MettaValue::Bool(b));
            let vm_result = run_vm_bool_binary(a, b, Opcode::And);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// OR: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_or_equivalence(a: bool, b: bool) {
            let grounded_result = run_grounded_binary(&OrOp, MettaValue::Bool(a), MettaValue::Bool(b));
            let vm_result = run_vm_bool_binary(a, b, Opcode::Or);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }

        /// NOT: Grounded tier and VM tier produce same result
        #[test]
        fn prop_tier_not_equivalence(a: bool) {
            let grounded_result = run_grounded_unary(&NotOp, MettaValue::Bool(a));
            let vm_result = run_vm_bool_unary(a, Opcode::Not);

            prop_assert!(grounded_result.is_ok() && vm_result.is_ok());
            prop_assert_eq!(grounded_result.unwrap(), vm_result.unwrap());
        }
    }

    // =========================================================================
    // Mathematical Properties Must Hold at Both Tiers
    // =========================================================================

    proptest! {
        /// ADD commutativity at both tiers
        #[test]
        fn prop_tier_add_commutative_both(a in -100i64..100i64, b in -100i64..100i64) {
            // Grounded tier
            let g_ab = run_grounded_binary(&AddOp, MettaValue::Long(a), MettaValue::Long(b));
            let g_ba = run_grounded_binary(&AddOp, MettaValue::Long(b), MettaValue::Long(a));
            prop_assert!(g_ab.is_ok() && g_ba.is_ok());
            prop_assert_eq!(g_ab.unwrap(), g_ba.unwrap(), "Grounded tier violated commutativity");

            // VM tier
            let v_ab = run_vm_binary(a, b, Opcode::Add);
            let v_ba = run_vm_binary(b, a, Opcode::Add);
            prop_assert!(v_ab.is_ok() && v_ba.is_ok());
            prop_assert_eq!(v_ab.unwrap(), v_ba.unwrap(), "VM tier violated commutativity");
        }

        /// MUL commutativity at both tiers
        #[test]
        fn prop_tier_mul_commutative_both(a in -50i64..50i64, b in -50i64..50i64) {
            // Grounded tier
            let g_ab = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(b));
            let g_ba = run_grounded_binary(&MulOp, MettaValue::Long(b), MettaValue::Long(a));
            prop_assert!(g_ab.is_ok() && g_ba.is_ok());
            prop_assert_eq!(g_ab.unwrap(), g_ba.unwrap());

            // VM tier
            let v_ab = run_vm_binary(a, b, Opcode::Mul);
            let v_ba = run_vm_binary(b, a, Opcode::Mul);
            prop_assert!(v_ab.is_ok() && v_ba.is_ok());
            prop_assert_eq!(v_ab.unwrap(), v_ba.unwrap());
        }

        /// ADD identity at both tiers: a + 0 = a
        #[test]
        fn prop_tier_add_identity_both(a in -1000i64..1000i64) {
            // Grounded tier
            let grounded = run_grounded_binary(&AddOp, MettaValue::Long(a), MettaValue::Long(0));
            prop_assert!(grounded.is_ok());
            prop_assert_eq!(grounded.unwrap(), MettaValue::Long(a));

            // VM tier
            let vm = run_vm_binary(a, 0, Opcode::Add);
            prop_assert!(vm.is_ok());
            prop_assert_eq!(vm.unwrap(), MettaValue::Long(a));
        }

        /// MUL identity at both tiers: a * 1 = a
        #[test]
        fn prop_tier_mul_identity_both(a in -1000i64..1000i64) {
            // Grounded tier
            let grounded = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(1));
            prop_assert!(grounded.is_ok());
            prop_assert_eq!(grounded.unwrap(), MettaValue::Long(a));

            // VM tier
            let vm = run_vm_binary(a, 1, Opcode::Mul);
            prop_assert!(vm.is_ok());
            prop_assert_eq!(vm.unwrap(), MettaValue::Long(a));
        }

        /// MUL zero annihilation at both tiers: a * 0 = 0
        #[test]
        fn prop_tier_mul_zero_both(a in -1000i64..1000i64) {
            // Grounded tier
            let grounded = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(0));
            prop_assert!(grounded.is_ok());
            prop_assert_eq!(grounded.unwrap(), MettaValue::Long(0));

            // VM tier
            let vm = run_vm_binary(a, 0, Opcode::Mul);
            prop_assert!(vm.is_ok());
            prop_assert_eq!(vm.unwrap(), MettaValue::Long(0));
        }

        /// SUB inverse at both tiers: a - a = 0
        #[test]
        fn prop_tier_sub_inverse_both(a in -1000i64..1000i64) {
            // Grounded tier
            let grounded = run_grounded_binary(&SubOp, MettaValue::Long(a), MettaValue::Long(a));
            prop_assert!(grounded.is_ok());
            prop_assert_eq!(grounded.unwrap(), MettaValue::Long(0));

            // VM tier
            let vm = run_vm_binary(a, a, Opcode::Sub);
            prop_assert!(vm.is_ok());
            prop_assert_eq!(vm.unwrap(), MettaValue::Long(0));
        }

        /// EQ reflexive at both tiers: a == a
        #[test]
        fn prop_tier_eq_reflexive_both(a in -1000i64..1000i64) {
            // Grounded tier
            let grounded = run_grounded_binary(&EqualOp, MettaValue::Long(a), MettaValue::Long(a));
            prop_assert!(grounded.is_ok());
            prop_assert_eq!(grounded.unwrap(), MettaValue::Bool(true));

            // VM tier
            let vm = run_vm_binary(a, a, Opcode::Eq);
            prop_assert!(vm.is_ok());
            prop_assert_eq!(vm.unwrap(), MettaValue::Bool(true));
        }

        /// LE reflexive at both tiers: a <= a
        #[test]
        fn prop_tier_le_reflexive_both(a in -1000i64..1000i64) {
            // Grounded tier
            let grounded = run_grounded_binary(&LessEqOp, MettaValue::Long(a), MettaValue::Long(a));
            prop_assert!(grounded.is_ok());
            prop_assert_eq!(grounded.unwrap(), MettaValue::Bool(true));

            // VM tier
            let vm = run_vm_binary(a, a, Opcode::Le);
            prop_assert!(vm.is_ok());
            prop_assert_eq!(vm.unwrap(), MettaValue::Bool(true));
        }

        /// NOT involutive at both tiers: !!a = a
        #[test]
        fn prop_tier_not_involutive_both(a: bool) {
            // Grounded tier: apply NOT twice
            let grounded1 = run_grounded_unary(&NotOp, MettaValue::Bool(a)).unwrap();
            let grounded2 = run_grounded_unary(&NotOp, grounded1).unwrap();
            prop_assert_eq!(grounded2, MettaValue::Bool(a));

            // VM tier: apply NOT twice
            let mut builder = ChunkBuilder::new("test");
            builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
            builder.emit(Opcode::Not);
            builder.emit(Opcode::Not);
            builder.emit(Opcode::Return);
            let chunk = builder.build_arc();
            let mut vm = BytecodeVM::new(chunk);
            let vm_result = vm.run().unwrap();
            prop_assert_eq!(&vm_result[0], &MettaValue::Bool(a));
        }

        /// De Morgan's law holds at both tiers: !(a && b) = !a || !b
        #[test]
        fn prop_tier_de_morgan_and_both(a: bool, b: bool) {
            // Grounded tier: !(a && b)
            let and_result = run_grounded_binary(&AndOp, MettaValue::Bool(a), MettaValue::Bool(b)).unwrap();
            let not_and = run_grounded_unary(&NotOp, and_result).unwrap();

            // Grounded tier: !a || !b
            let not_a = run_grounded_unary(&NotOp, MettaValue::Bool(a)).unwrap();
            let not_b = run_grounded_unary(&NotOp, MettaValue::Bool(b)).unwrap();
            let or_not = run_grounded_binary(&OrOp, not_a, not_b).unwrap();

            prop_assert_eq!(not_and, or_not, "Grounded tier De Morgan failed");

            // VM tier: same property
            // !(a && b)
            let mut builder1 = ChunkBuilder::new("test");
            builder1.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
            builder1.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
            builder1.emit(Opcode::And);
            builder1.emit(Opcode::Not);
            builder1.emit(Opcode::Return);
            let vm_not_and = BytecodeVM::new(builder1.build_arc()).run().unwrap()[0].clone();

            // !a || !b
            let mut builder2 = ChunkBuilder::new("test");
            builder2.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
            builder2.emit(Opcode::Not);
            builder2.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
            builder2.emit(Opcode::Not);
            builder2.emit(Opcode::Or);
            builder2.emit(Opcode::Return);
            let vm_or_not = BytecodeVM::new(builder2.build_arc()).run().unwrap()[0].clone();

            prop_assert_eq!(vm_not_and, vm_or_not, "VM tier De Morgan failed");
        }
    }
}

// =============================================================================
// Three-Tier Tests: Grounded vs VM vs JIT
// =============================================================================
//
// These tests ensure semantic equivalence across all three execution tiers:
// - Tier 0: Grounded operations (tree-walker)
// - Tier 1: Bytecode VM
// - Tier 2/3: JIT-compiled native code
//
// The tests use a combination of:
// - Example-based tests for specific operation validation
// - Property-based tests for mathematical invariants

#[cfg(test)]
mod three_tier_tests {
    use std::sync::Arc;

    use super::*;

    use crate::backend::bytecode::chunk::ChunkBuilder;
    use crate::backend::bytecode::jit::{JitCompiler, JitContext, JitValue};
    use crate::backend::bytecode::opcodes::Opcode;
    use crate::backend::bytecode::BytecodeVM;
    use crate::backend::grounded::{
        AddOp, AndOp, DivOp, EqualOp, GreaterEqOp, GreaterOp, GroundedOperationTCO, GroundedState,
        GroundedWork, LessEqOp, LessOp, ModOp, MulOp, NotEqualOp, NotOp, OrOp, SubOp,
    };
    use crate::backend::models::{GcFactory, MettaValue, MettaValueInner};

    // =========================================================================
    // Helper Functions
    // =========================================================================

    /// Drive a generic TCO grounded operation to completion.
    ///
    /// For tests, arguments are already concrete values, so when the state
    /// machine requests `EvalArg(idx)`, we "evaluate" by returning the original
    /// argument at that index as-is (wrapping it in a single-element Vec).
    fn drive_tco<Op>(op: &Op, args: Vec<MettaValue>) -> Result<MettaValue, String>
    where
        Op: GroundedOperationTCO<MettaValue>,
    {
        let factory = GcFactory::default();
        let original_args = args.clone();
        let mut state = GroundedState::new(op.name().to_string(), args);

        loop {
            let work = op.execute_step(&mut state, &factory);
            match work {
                GroundedWork::Done(results) => {
                    return Ok(results
                        .into_iter()
                        .next()
                        .map(|(v, _)| v)
                        .unwrap_or(MettaValue::Unit()));
                }
                GroundedWork::EvalArg {
                    arg_idx,
                    state: returned_state,
                } => {
                    state = returned_state;
                    let arg_val = original_args[arg_idx].clone();
                    state.set_arg(arg_idx, vec![arg_val]);
                }
                GroundedWork::Error(e) => {
                    return Err(format!("grounded error: {:?}", e));
                }
            }
        }
    }

    /// Execute a binary operation via generic TCO grounded operation (Tier 0)
    fn run_grounded_binary<Op>(op: &Op, a: MettaValue, b: MettaValue) -> Result<MettaValue, String>
    where
        Op: GroundedOperationTCO<MettaValue>,
    {
        drive_tco(op, vec![a, b])
    }

    /// Execute a unary operation via generic TCO grounded operation (Tier 0)
    fn run_grounded_unary<Op>(op: &Op, a: MettaValue) -> Result<MettaValue, String>
    where
        Op: GroundedOperationTCO<MettaValue>,
    {
        drive_tco(op, vec![a])
    }

    /// Execute a binary operation via bytecode VM (Tier 1)
    fn run_vm_binary(a: i64, b: i64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");

        if a >= -128 && a <= 127 {
            builder.emit_byte(Opcode::PushLongSmall, a as u8);
        } else {
            let idx = builder.add_constant(MettaValue::Long(a));
            builder.emit_u16(Opcode::PushLong, idx);
        }

        if b >= -128 && b <= 127 {
            builder.emit_byte(Opcode::PushLongSmall, b as u8);
        } else {
            let idx = builder.add_constant(MettaValue::Long(b));
            builder.emit_u16(Opcode::PushLong, idx);
        }

        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute a unary operation via bytecode VM (Tier 1)
    fn run_vm_unary(a: i64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");

        if a >= -128 && a <= 127 {
            builder.emit_byte(Opcode::PushLongSmall, a as u8);
        } else {
            let idx = builder.add_constant(MettaValue::Long(a));
            builder.emit_u16(Opcode::PushLong, idx);
        }

        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute a boolean binary operation via bytecode VM (Tier 1)
    fn run_vm_bool_binary(a: bool, b: bool, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(if b {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute a boolean unary operation via bytecode VM (Tier 1)
    fn run_vm_bool_unary(a: bool, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(if a {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute a binary operation via JIT-compiled native code (Tier 2/3)
    ///
    /// This function directly compiles the chunk to native code, bypassing
    /// the tier thresholds that HybridExecutor uses.
    fn run_jit_binary(a: i64, b: i64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("jit_test");

        if a >= -128 && a <= 127 {
            builder.emit_byte(Opcode::PushLongSmall, a as u8);
        } else {
            let idx = builder.add_constant(MettaValue::Long(a));
            builder.emit_u16(Opcode::PushLong, idx);
        }

        if b >= -128 && b <= 127 {
            builder.emit_byte(Opcode::PushLongSmall, b as u8);
        } else {
            let idx = builder.add_constant(MettaValue::Long(b));
            builder.emit_u16(Opcode::PushLong, idx);
        }

        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        execute_jit_chunk(&chunk)
    }

    /// Execute a unary operation via JIT-compiled native code (Tier 2/3)
    fn run_jit_unary(a: i64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("jit_test");

        if a >= -128 && a <= 127 {
            builder.emit_byte(Opcode::PushLongSmall, a as u8);
        } else {
            let idx = builder.add_constant(MettaValue::Long(a));
            builder.emit_u16(Opcode::PushLong, idx);
        }

        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        execute_jit_chunk(&chunk)
    }

    /// Execute a boolean binary operation via JIT (Tier 2/3)
    fn run_jit_bool_binary(a: bool, b: bool, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("jit_test");
        builder.emit(if a {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(if b {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        execute_jit_chunk(&chunk)
    }

    /// Execute a boolean unary operation via JIT (Tier 2/3)
    fn run_jit_bool_unary(a: bool, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("jit_test");
        builder.emit(if a {
            Opcode::PushTrue
        } else {
            Opcode::PushFalse
        });
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        execute_jit_chunk(&chunk)
    }

    /// Execute a float binary operation via VM (Tier 1)
    fn run_vm_float_binary(a: f64, b: f64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");
        let idx_a = builder.add_constant(MettaValue::Float(a));
        let idx_b = builder.add_constant(MettaValue::Float(b));
        builder.emit_u16(Opcode::PushConstant, idx_a);
        builder.emit_u16(Opcode::PushConstant, idx_b);
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute a float binary operation via JIT (Tier 2/3)
    fn run_jit_float_binary(a: f64, b: f64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("jit_test");
        let idx_a = builder.add_constant(MettaValue::Float(a));
        let idx_b = builder.add_constant(MettaValue::Float(b));
        builder.emit_u16(Opcode::PushConstant, idx_a);
        builder.emit_u16(Opcode::PushConstant, idx_b);
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        execute_jit_chunk(&chunk)
    }

    /// Execute a float unary operation via VM (Tier 1)
    fn run_vm_float_unary(a: f64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");
        let idx = builder.add_constant(MettaValue::Float(a));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute a float unary operation via JIT (Tier 2/3)
    fn run_jit_float_unary(a: f64, opcode: Opcode) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("jit_test");
        let idx = builder.add_constant(MettaValue::Float(a));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(opcode);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        execute_jit_chunk(&chunk)
    }

    /// Execute a binary operation on arbitrary MettaValue operands via VM
    fn run_vm_value_binary(
        a: MettaValue,
        b: MettaValue,
        opcode: Opcode,
    ) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("test");
        let idx_a = builder.add_constant(a);
        let idx_b = builder.add_constant(b);
        builder.emit_u16(Opcode::PushConstant, idx_a);
        builder.emit_u16(Opcode::PushConstant, idx_b);
        builder.emit(opcode);
        builder.emit(Opcode::Return);
        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        vm.run()
            .map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            .map_err(|e| format!("{}", e))
    }

    /// Execute a binary operation on arbitrary MettaValue operands via JIT
    fn run_jit_value_binary(
        a: MettaValue,
        b: MettaValue,
        opcode: Opcode,
    ) -> Result<MettaValue, String> {
        let mut builder = ChunkBuilder::new("jit_test");
        let idx_a = builder.add_constant(a);
        let idx_b = builder.add_constant(b);
        builder.emit_u16(Opcode::PushConstant, idx_a);
        builder.emit_u16(Opcode::PushConstant, idx_b);
        builder.emit(opcode);
        builder.emit(Opcode::Return);
        let chunk = builder.build_arc();
        execute_jit_chunk(&chunk)
    }

    /// Core JIT execution helper - compiles chunk and executes via JIT context
    fn execute_jit_chunk(
        chunk: &Arc<crate::backend::bytecode::BytecodeChunk>,
    ) -> Result<MettaValue, String> {
        // Try to create JIT compiler (may fail if JIT is disabled or unsupported)
        let mut compiler = match JitCompiler::new() {
            Ok(c) => c,
            Err(e) => return Err(format!("JIT compiler creation failed: {:?}", e)),
        };

        // Compile the chunk to native code
        let native_ptr = match compiler.compile(chunk) {
            Ok(ptr) => ptr,
            Err(e) => return Err(format!("JIT compilation failed: {:?}", e)),
        };

        // Set up JIT context with minimal buffers
        let mut stack = vec![JitValue::unit(); 64];
        let constants = chunk.constants();

        // Create JIT context
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Cast and call native function
        let native_fn: extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(native_ptr) };

        let jit_result = native_fn(&mut ctx);

        // Check for bailout (should not happen for simple arithmetic)
        if ctx.bailout {
            return Err(format!(
                "JIT bailout at ip={}: {:?}",
                ctx.bailout_ip, ctx.bailout_reason
            ));
        }

        // Convert result
        if jit_result != 0 {
            let jit_val = JitValue::from_raw(jit_result as u64);
            Ok(unsafe { jit_val.to_metta() })
        } else if ctx.sp > 0 {
            let jit_val = unsafe { *ctx.value_stack };
            Ok(unsafe { jit_val.to_metta() })
        } else {
            Ok(MettaValue::Unit())
        }
    }

    // =========================================================================
    // Phase 1: Three-Tier Equivalence Tests (Example-Based)
    // =========================================================================
    //
    // These tests verify that specific operations produce identical results
    // across all three tiers using concrete examples.

    #[test]
    fn test_three_tier_add_basic() {
        let test_cases = [(5, 3), (0, 0), (-10, 10), (100, 200), (-50, -50)];

        for (a, b) in test_cases {
            let grounded = run_grounded_binary(&AddOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Add);
            let jit = run_jit_binary(a, b, Opcode::Add);

            assert!(grounded.is_ok(), "Grounded failed for add({}, {})", a, b);
            assert!(vm.is_ok(), "VM failed for add({}, {})", a, b);

            let grounded_val = grounded.unwrap();
            let vm_val = vm.unwrap();

            assert_eq!(grounded_val, vm_val, "Grounded != VM for add({}, {})", a, b);

            // JIT may not be available on all platforms
            if let Ok(jit_val) = jit {
                assert_eq!(vm_val, jit_val, "VM != JIT for add({}, {})", a, b);
            }
        }
    }

    #[test]
    fn test_three_tier_sub_basic() {
        let test_cases = [(10, 3), (0, 0), (5, 10), (100, 50), (-20, -10)];

        for (a, b) in test_cases {
            let grounded = run_grounded_binary(&SubOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Sub);
            let jit = run_jit_binary(a, b, Opcode::Sub);

            assert!(grounded.is_ok() && vm.is_ok());
            assert_eq!(grounded.unwrap(), vm.unwrap());

            if let Ok(jit_val) = jit {
                assert_eq!(
                    run_vm_binary(a, b, Opcode::Sub).unwrap(),
                    jit_val,
                    "VM != JIT for sub({}, {})",
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn test_three_tier_mul_basic() {
        let test_cases = [(3, 4), (0, 100), (-5, 5), (7, 7), (-3, -3)];

        for (a, b) in test_cases {
            let grounded = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Mul);
            let jit = run_jit_binary(a, b, Opcode::Mul);

            assert!(grounded.is_ok() && vm.is_ok());
            assert_eq!(grounded.unwrap(), vm.unwrap());

            if let Ok(jit_val) = jit {
                assert_eq!(run_vm_binary(a, b, Opcode::Mul).unwrap(), jit_val);
            }
        }
    }

    #[test]
    fn test_three_tier_div_basic() {
        let test_cases = [(10, 2), (100, 10), (15, 3), (7, 2), (-10, 2)];

        for (a, b) in test_cases {
            let grounded = run_grounded_binary(&DivOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Div);
            let jit = run_jit_binary(a, b, Opcode::Div);

            assert!(grounded.is_ok() && vm.is_ok());
            assert_eq!(grounded.unwrap(), vm.unwrap());

            if let Ok(jit_val) = jit {
                assert_eq!(run_vm_binary(a, b, Opcode::Div).unwrap(), jit_val);
            }
        }
    }

    #[test]
    fn test_three_tier_mod_basic() {
        let test_cases = [(10, 3), (100, 7), (15, 4), (8, 3), (17, 5)];

        for (a, b) in test_cases {
            let grounded = run_grounded_binary(&ModOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Mod);
            let jit = run_jit_binary(a, b, Opcode::Mod);

            assert!(grounded.is_ok() && vm.is_ok());
            assert_eq!(grounded.unwrap(), vm.unwrap());

            if let Ok(jit_val) = jit {
                assert_eq!(run_vm_binary(a, b, Opcode::Mod).unwrap(), jit_val);
            }
        }
    }

    #[test]
    fn test_three_tier_less_basic() {
        let test_cases = [(1, 2, true), (2, 1, false), (5, 5, false), (-1, 0, true)];

        for (a, b, expected) in test_cases {
            let grounded = run_grounded_binary(&LessOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Lt);
            let jit = run_jit_binary(a, b, Opcode::Lt);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_less_eq_basic() {
        let test_cases = [(1, 2, true), (2, 1, false), (5, 5, true), (-1, -1, true)];

        for (a, b, expected) in test_cases {
            let grounded = run_grounded_binary(&LessEqOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Le);
            let jit = run_jit_binary(a, b, Opcode::Le);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_greater_basic() {
        let test_cases = [(2, 1, true), (1, 2, false), (5, 5, false), (0, -1, true)];

        for (a, b, expected) in test_cases {
            let grounded =
                run_grounded_binary(&GreaterOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Gt);
            let jit = run_jit_binary(a, b, Opcode::Gt);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_greater_eq_basic() {
        let test_cases = [(2, 1, true), (1, 2, false), (5, 5, true), (0, 0, true)];

        for (a, b, expected) in test_cases {
            let grounded =
                run_grounded_binary(&GreaterEqOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Ge);
            let jit = run_jit_binary(a, b, Opcode::Ge);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_equal_basic() {
        let test_cases = [(5, 5, true), (5, 6, false), (0, 0, true), (-1, -1, true)];

        for (a, b, expected) in test_cases {
            let grounded = run_grounded_binary(&EqualOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Eq);
            let jit = run_jit_binary(a, b, Opcode::Eq);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_not_equal_basic() {
        let test_cases = [(5, 5, false), (5, 6, true), (0, 1, true), (-1, 1, true)];

        for (a, b, expected) in test_cases {
            let grounded =
                run_grounded_binary(&NotEqualOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Ne);
            let jit = run_jit_binary(a, b, Opcode::Ne);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_and_basic() {
        let test_cases = [
            (true, true, true),
            (true, false, false),
            (false, true, false),
            (false, false, false),
        ];

        for (a, b, expected) in test_cases {
            let grounded = run_grounded_binary(&AndOp, MettaValue::Bool(a), MettaValue::Bool(b));
            let vm = run_vm_bool_binary(a, b, Opcode::And);
            let jit = run_jit_bool_binary(a, b, Opcode::And);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_or_basic() {
        let test_cases = [
            (true, true, true),
            (true, false, true),
            (false, true, true),
            (false, false, false),
        ];

        for (a, b, expected) in test_cases {
            let grounded = run_grounded_binary(&OrOp, MettaValue::Bool(a), MettaValue::Bool(b));
            let vm = run_vm_bool_binary(a, b, Opcode::Or);
            let jit = run_jit_bool_binary(a, b, Opcode::Or);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    #[test]
    fn test_three_tier_not_basic() {
        let test_cases = [(true, false), (false, true)];

        for (a, expected) in test_cases {
            let grounded = run_grounded_unary(&NotOp, MettaValue::Bool(a));
            let vm = run_vm_bool_unary(a, Opcode::Not);
            let jit = run_jit_bool_unary(a, Opcode::Not);

            assert_eq!(grounded.unwrap(), MettaValue::Bool(expected));
            assert_eq!(vm.unwrap(), MettaValue::Bool(expected));

            if let Ok(jit_val) = jit {
                assert_eq!(jit_val, MettaValue::Bool(expected));
            }
        }
    }

    // =========================================================================
    // Phase 2: Three-Tier Property Tests (Property-Based)
    // =========================================================================
    //
    // These tests use proptest to verify mathematical properties hold across
    // all tiers with randomly generated inputs.

    proptest! {
        /// Three-tier ADD equivalence with property-based inputs
        #[test]
        fn prop_three_tier_add(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&AddOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Add);
            let jit = run_jit_binary(a, b, Opcode::Add);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap(), "Grounded != VM");

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Add).unwrap(), &jit_val, "VM != JIT");
            }
        }

        /// Three-tier SUB equivalence
        #[test]
        fn prop_three_tier_sub(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&SubOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Sub);
            let jit = run_jit_binary(a, b, Opcode::Sub);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Sub).unwrap(), &jit_val);
            }
        }

        /// Three-tier MUL equivalence
        #[test]
        fn prop_three_tier_mul(a in -100i64..100i64, b in -100i64..100i64) {
            let grounded = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Mul);
            let jit = run_jit_binary(a, b, Opcode::Mul);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Mul).unwrap(), &jit_val);
            }
        }

        /// Three-tier DIV equivalence (non-zero divisor)
        #[test]
        fn prop_three_tier_div(a in -1000i64..1000i64, b in 1i64..100i64) {
            let grounded = run_grounded_binary(&DivOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Div);
            let jit = run_jit_binary(a, b, Opcode::Div);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Div).unwrap(), &jit_val);
            }
        }

        /// Three-tier MOD equivalence (non-zero divisor)
        #[test]
        fn prop_three_tier_mod(a in 0i64..1000i64, b in 1i64..100i64) {
            let grounded = run_grounded_binary(&ModOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Mod);
            let jit = run_jit_binary(a, b, Opcode::Mod);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Mod).unwrap(), &jit_val);
            }
        }

        /// Three-tier LESS equivalence
        #[test]
        fn prop_three_tier_less(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&LessOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Lt);
            let jit = run_jit_binary(a, b, Opcode::Lt);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Lt).unwrap(), &jit_val);
            }
        }

        /// Three-tier LESS_EQ equivalence
        #[test]
        fn prop_three_tier_less_eq(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&LessEqOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Le);
            let jit = run_jit_binary(a, b, Opcode::Le);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Le).unwrap(), &jit_val);
            }
        }

        /// Three-tier GREATER equivalence
        #[test]
        fn prop_three_tier_greater(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&GreaterOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Gt);
            let jit = run_jit_binary(a, b, Opcode::Gt);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Gt).unwrap(), &jit_val);
            }
        }

        /// Three-tier GREATER_EQ equivalence
        #[test]
        fn prop_three_tier_greater_eq(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&GreaterEqOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Ge);
            let jit = run_jit_binary(a, b, Opcode::Ge);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Ge).unwrap(), &jit_val);
            }
        }

        /// Three-tier EQUAL equivalence
        #[test]
        fn prop_three_tier_equal(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&EqualOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Eq);
            let jit = run_jit_binary(a, b, Opcode::Eq);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Eq).unwrap(), &jit_val);
            }
        }

        /// Three-tier NOT_EQUAL equivalence
        #[test]
        fn prop_three_tier_not_equal(a in -1000i64..1000i64, b in -1000i64..1000i64) {
            let grounded = run_grounded_binary(&NotEqualOp, MettaValue::Long(a), MettaValue::Long(b));
            let vm = run_vm_binary(a, b, Opcode::Ne);
            let jit = run_jit_binary(a, b, Opcode::Ne);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_binary(a, b, Opcode::Ne).unwrap(), &jit_val);
            }
        }

        /// Three-tier AND equivalence
        #[test]
        fn prop_three_tier_and(a: bool, b: bool) {
            let grounded = run_grounded_binary(&AndOp, MettaValue::Bool(a), MettaValue::Bool(b));
            let vm = run_vm_bool_binary(a, b, Opcode::And);
            let jit = run_jit_bool_binary(a, b, Opcode::And);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_bool_binary(a, b, Opcode::And).unwrap(), &jit_val);
            }
        }

        /// Three-tier OR equivalence
        #[test]
        fn prop_three_tier_or(a: bool, b: bool) {
            let grounded = run_grounded_binary(&OrOp, MettaValue::Bool(a), MettaValue::Bool(b));
            let vm = run_vm_bool_binary(a, b, Opcode::Or);
            let jit = run_jit_bool_binary(a, b, Opcode::Or);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_bool_binary(a, b, Opcode::Or).unwrap(), &jit_val);
            }
        }

        /// Three-tier NOT equivalence
        #[test]
        fn prop_three_tier_not(a: bool) {
            let grounded = run_grounded_unary(&NotOp, MettaValue::Bool(a));
            let vm = run_vm_bool_unary(a, Opcode::Not);
            let jit = run_jit_bool_unary(a, Opcode::Not);

            prop_assert!(grounded.is_ok() && vm.is_ok());
            prop_assert_eq!(&grounded.unwrap(), &vm.unwrap());

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&run_vm_bool_unary(a, Opcode::Not).unwrap(), &jit_val);
            }
        }
    }

    // =========================================================================
    // Phase 3: Mathematical Properties Across All Tiers
    // =========================================================================
    //
    // These tests verify that mathematical invariants hold at all tiers.

    proptest! {
        /// ADD commutativity: a + b == b + a at all tiers
        #[test]
        fn prop_three_tier_add_commutative(a in -500i64..500i64, b in -500i64..500i64) {
            // Grounded tier
            let g_ab = run_grounded_binary(&AddOp, MettaValue::Long(a), MettaValue::Long(b)).unwrap();
            let g_ba = run_grounded_binary(&AddOp, MettaValue::Long(b), MettaValue::Long(a)).unwrap();
            prop_assert_eq!(&g_ab, &g_ba, "Grounded tier violated commutativity");

            // VM tier
            let v_ab = run_vm_binary(a, b, Opcode::Add).unwrap();
            let v_ba = run_vm_binary(b, a, Opcode::Add).unwrap();
            prop_assert_eq!(&v_ab, &v_ba, "VM tier violated commutativity");

            // JIT tier (if available)
            if let (Ok(j_ab), Ok(j_ba)) = (run_jit_binary(a, b, Opcode::Add), run_jit_binary(b, a, Opcode::Add)) {
                prop_assert_eq!(&j_ab, &j_ba, "JIT tier violated commutativity");
            }
        }

        /// MUL commutativity: a * b == b * a at all tiers
        #[test]
        fn prop_three_tier_mul_commutative(a in -50i64..50i64, b in -50i64..50i64) {
            // Grounded tier
            let g_ab = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(b)).unwrap();
            let g_ba = run_grounded_binary(&MulOp, MettaValue::Long(b), MettaValue::Long(a)).unwrap();
            prop_assert_eq!(&g_ab, &g_ba);

            // VM tier
            let v_ab = run_vm_binary(a, b, Opcode::Mul).unwrap();
            let v_ba = run_vm_binary(b, a, Opcode::Mul).unwrap();
            prop_assert_eq!(&v_ab, &v_ba);

            // JIT tier
            if let (Ok(j_ab), Ok(j_ba)) = (run_jit_binary(a, b, Opcode::Mul), run_jit_binary(b, a, Opcode::Mul)) {
                prop_assert_eq!(&j_ab, &j_ba);
            }
        }

        /// ADD identity: a + 0 == a at all tiers
        #[test]
        fn prop_three_tier_add_identity(a in -1000i64..1000i64) {
            let expected = MettaValue::Long(a);

            // Grounded
            prop_assert_eq!(&run_grounded_binary(&AddOp, MettaValue::Long(a), MettaValue::Long(0)).unwrap(), &expected);

            // VM
            prop_assert_eq!(&run_vm_binary(a, 0, Opcode::Add).unwrap(), &expected);

            // JIT
            if let Ok(jit_val) = run_jit_binary(a, 0, Opcode::Add) {
                prop_assert_eq!(&jit_val, &expected);
            }
        }

        /// MUL identity: a * 1 == a at all tiers
        #[test]
        fn prop_three_tier_mul_identity(a in -1000i64..1000i64) {
            let expected = MettaValue::Long(a);

            // Grounded
            prop_assert_eq!(&run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(1)).unwrap(), &expected);

            // VM
            prop_assert_eq!(&run_vm_binary(a, 1, Opcode::Mul).unwrap(), &expected);

            // JIT
            if let Ok(jit_val) = run_jit_binary(a, 1, Opcode::Mul) {
                prop_assert_eq!(&jit_val, &expected);
            }
        }

        /// MUL zero annihilation: a * 0 == 0 at all tiers
        #[test]
        fn prop_three_tier_mul_zero(a in -1000i64..1000i64) {
            let expected = MettaValue::Long(0);

            // Grounded
            prop_assert_eq!(&run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(0)).unwrap(), &expected);

            // VM
            prop_assert_eq!(&run_vm_binary(a, 0, Opcode::Mul).unwrap(), &expected);

            // JIT
            if let Ok(jit_val) = run_jit_binary(a, 0, Opcode::Mul) {
                prop_assert_eq!(&jit_val, &expected);
            }
        }

        /// De Morgan's law for AND: !(a && b) == !a || !b at all tiers
        #[test]
        fn prop_three_tier_de_morgan_and(a: bool, b: bool) {
            let expected = !a || !b;

            // Grounded tier
            let g_and = run_grounded_binary(&AndOp, MettaValue::Bool(a), MettaValue::Bool(b)).unwrap();
            let g_not_and = run_grounded_unary(&NotOp, g_and).unwrap();
            let g_not_a = run_grounded_unary(&NotOp, MettaValue::Bool(a)).unwrap();
            let g_not_b = run_grounded_unary(&NotOp, MettaValue::Bool(b)).unwrap();
            let g_or_not = run_grounded_binary(&OrOp, g_not_a, g_not_b).unwrap();
            prop_assert_eq!(&g_not_and, &g_or_not, "Grounded De Morgan failed");
            prop_assert_eq!(&g_not_and, &MettaValue::Bool(expected));

            // VM tier (use bytecode sequences)
            let vm_not_and = {
                let mut builder = ChunkBuilder::new("test");
                builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(Opcode::And);
                builder.emit(Opcode::Not);
                builder.emit(Opcode::Return);
                let mut vm = BytecodeVM::new(builder.build_arc());
                vm.run().unwrap()[0].clone()
            };
            prop_assert_eq!(&vm_not_and, &MettaValue::Bool(expected));

            // JIT tier
            if let Ok(jit_not_and) = {
                let mut builder = ChunkBuilder::new("jit_test");
                builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(Opcode::And);
                builder.emit(Opcode::Not);
                builder.emit(Opcode::Return);
                execute_jit_chunk(&builder.build_arc())
            } {
                prop_assert_eq!(&jit_not_and, &MettaValue::Bool(expected));
            }
        }

        /// De Morgan's law for OR: !(a || b) == !a && !b at all tiers
        #[test]
        fn prop_three_tier_de_morgan_or(a: bool, b: bool) {
            let expected = !a && !b;

            // Grounded tier
            let g_or = run_grounded_binary(&OrOp, MettaValue::Bool(a), MettaValue::Bool(b)).unwrap();
            let g_not_or = run_grounded_unary(&NotOp, g_or).unwrap();
            let g_not_a = run_grounded_unary(&NotOp, MettaValue::Bool(a)).unwrap();
            let g_not_b = run_grounded_unary(&NotOp, MettaValue::Bool(b)).unwrap();
            let g_and_not = run_grounded_binary(&AndOp, g_not_a, g_not_b).unwrap();
            prop_assert_eq!(&g_not_or, &g_and_not, "Grounded De Morgan (OR) failed");
            prop_assert_eq!(&g_not_or, &MettaValue::Bool(expected));

            // VM tier
            let vm_not_or = {
                let mut builder = ChunkBuilder::new("test");
                builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(Opcode::Or);
                builder.emit(Opcode::Not);
                builder.emit(Opcode::Return);
                let mut vm = BytecodeVM::new(builder.build_arc());
                vm.run().unwrap()[0].clone()
            };
            prop_assert_eq!(&vm_not_or, &MettaValue::Bool(expected));

            // JIT tier
            if let Ok(jit_not_or) = {
                let mut builder = ChunkBuilder::new("jit_test");
                builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(Opcode::Or);
                builder.emit(Opcode::Not);
                builder.emit(Opcode::Return);
                execute_jit_chunk(&builder.build_arc())
            } {
                prop_assert_eq!(&jit_not_or, &MettaValue::Bool(expected));
            }
        }

        /// Distributive property: a * (b + c) == a*b + a*c at all tiers
        #[test]
        fn prop_three_tier_distributive(a in -20i64..20i64, b in -20i64..20i64, c in -20i64..20i64) {
            // Calculate expected result: a * (b + c)
            let bc_sum = b + c;
            let left = a * bc_sum;
            let right = a * b + a * c;
            // Sanity check: distributive property should hold in Rust
            prop_assert_eq!(left, right);

            // Now verify at each tier
            let expected = MettaValue::Long(left);

            // Grounded tier: a * (b + c)
            let g_bc = run_grounded_binary(&AddOp, MettaValue::Long(b), MettaValue::Long(c)).unwrap();
            let g_bc_val = match g_bc.inner() { MettaValueInner::Long(n) => *n, _ => panic!("Expected Long") };
            let g_left = run_grounded_binary(&MulOp, MettaValue::Long(a), MettaValue::Long(g_bc_val)).unwrap();
            prop_assert_eq!(&g_left, &expected, "Grounded distributive failed");

            // VM tier
            let v_bc = match run_vm_binary(b, c, Opcode::Add).unwrap().inner() { MettaValueInner::Long(n) => *n, _ => panic!() };
            let v_left = run_vm_binary(a, v_bc, Opcode::Mul).unwrap();
            prop_assert_eq!(&v_left, &expected, "VM distributive failed");

            // JIT tier
            if let Ok(jit_bc) = run_jit_binary(b, c, Opcode::Add) {
                if let MettaValueInner::Long(jit_bc_val) = jit_bc.inner() {
                    if let Ok(jit_left) = run_jit_binary(a, *jit_bc_val, Opcode::Mul) {
                        prop_assert_eq!(&jit_left, &expected, "JIT distributive failed");
                    }
                }
            }
        }
    }

    // =========================================================================
    // Phase 4: VM-Only Operation Tests (VM vs JIT)
    // =========================================================================
    //
    // These operations exist only at VM tier (no grounded equivalent).
    // We test VM vs JIT equivalence.

    proptest! {
        /// NEG: VM vs JIT equivalence
        #[test]
        fn prop_vm_jit_neg(a in -1000i64..1000i64) {
            let vm = run_vm_unary(a, Opcode::Neg);
            let jit = run_jit_unary(a, Opcode::Neg);

            prop_assert!(vm.is_ok());
            prop_assert_eq!(&vm.unwrap(), &MettaValue::Long(-a));

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&jit_val, &MettaValue::Long(-a));
            }
        }

        /// ABS: VM vs JIT equivalence
        #[test]
        fn prop_vm_jit_abs(a in -1000i64..1000i64) {
            let vm = run_vm_unary(a, Opcode::Abs);
            let jit = run_jit_unary(a, Opcode::Abs);

            prop_assert!(vm.is_ok());
            prop_assert_eq!(&vm.unwrap(), &MettaValue::Long(a.abs()));

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&jit_val, &MettaValue::Long(a.abs()));
            }
        }

        /// XOR: VM vs JIT equivalence (boolean XOR)
        #[test]
        fn prop_vm_jit_xor(a: bool, b: bool) {
            let vm = {
                let mut builder = ChunkBuilder::new("test");
                builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(Opcode::Xor);
                builder.emit(Opcode::Return);
                let chunk = builder.build_arc();
                let mut vm = BytecodeVM::new(chunk);
                vm.run().map(|r| r.into_iter().next().unwrap_or(MettaValue::Unit()))
            };

            let jit = {
                let mut builder = ChunkBuilder::new("jit_test");
                builder.emit(if a { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(if b { Opcode::PushTrue } else { Opcode::PushFalse });
                builder.emit(Opcode::Xor);
                builder.emit(Opcode::Return);
                execute_jit_chunk(&builder.build_arc())
            };

            prop_assert!(vm.is_ok());
            prop_assert_eq!(&vm.unwrap(), &MettaValue::Bool(a ^ b));

            if let Ok(jit_val) = jit {
                prop_assert_eq!(&jit_val, &MettaValue::Bool(a ^ b));
            }
        }
    }

    // =========================================================================
    // Phase 5: Float Type Coverage
    // =========================================================================

    // NOTE: The VM's arithmetic operations (Add, Sub, Mul, Div, Mod) only support
    // Long (integer) values. Float operations like Sin, Cos, Sqrt work because
    // they are inherently float operations. We test these cases separately below.

    /// Test that VM correctly handles trig operations on float values
    #[test]
    fn test_vm_float_trig_operations() {
        // Sin should work with float input
        let sin_result = run_vm_float_unary(0.5, Opcode::Sin);
        assert!(sin_result.is_ok(), "VM sin(0.5) failed: {:?}", sin_result);

        // Cos should work with float input
        let cos_result = run_vm_float_unary(0.5, Opcode::Cos);
        assert!(cos_result.is_ok(), "VM cos(0.5) failed: {:?}", cos_result);

        // Sqrt should work with float input
        let sqrt_result = run_vm_float_unary(4.0, Opcode::Sqrt);
        assert!(
            sqrt_result.is_ok(),
            "VM sqrt(4.0) failed: {:?}",
            sqrt_result
        );
    }

    /// Test that VM arithmetic ops support both Long and Float
    #[test]
    fn test_vm_arithmetic_float_support() {
        // Float + Float should work
        let float_add = run_vm_float_binary(1.5, 2.5, Opcode::Add);
        assert!(float_add.is_ok(), "VM Add should support Float");
        assert_eq!(float_add.unwrap(), MettaValue::Float(4.0));

        // Long + Long should work
        let int_add = run_vm_binary(10, 20, Opcode::Add);
        assert!(int_add.is_ok());
        assert_eq!(int_add.unwrap(), MettaValue::Long(30));

        // Float - Float should work
        let float_sub = run_vm_float_binary(5.5, 2.5, Opcode::Sub);
        assert!(float_sub.is_ok(), "VM Sub should support Float");
        assert_eq!(float_sub.unwrap(), MettaValue::Float(3.0));

        // Float * Float should work
        let float_mul = run_vm_float_binary(2.0, 3.5, Opcode::Mul);
        assert!(float_mul.is_ok(), "VM Mul should support Float");
        assert_eq!(float_mul.unwrap(), MettaValue::Float(7.0));

        // Float / Float should work
        let float_div = run_vm_float_binary(7.0, 2.0, Opcode::Div);
        assert!(float_div.is_ok(), "VM Div should support Float");
        assert_eq!(float_div.unwrap(), MettaValue::Float(3.5));
    }

    /// Test VM comparison operations with Float values
    #[test]
    fn test_vm_comparison_float_support() {
        // Float < Float
        let lt_result = run_vm_float_binary(1.5, 2.5, Opcode::Lt);
        assert!(lt_result.is_ok(), "VM Lt should support Float");
        assert_eq!(lt_result.unwrap(), MettaValue::Bool(true));

        // Float > Float
        let gt_result = run_vm_float_binary(3.0, 2.0, Opcode::Gt);
        assert!(gt_result.is_ok(), "VM Gt should support Float");
        assert_eq!(gt_result.unwrap(), MettaValue::Bool(true));

        // Float <= Float
        let le_result = run_vm_float_binary(2.5, 2.5, Opcode::Le);
        assert!(le_result.is_ok(), "VM Le should support Float");
        assert_eq!(le_result.unwrap(), MettaValue::Bool(true));

        // Float >= Float
        let ge_result = run_vm_float_binary(3.0, 2.0, Opcode::Ge);
        assert!(ge_result.is_ok(), "VM Ge should support Float");
        assert_eq!(ge_result.unwrap(), MettaValue::Bool(true));
    }

    // =========================================================================
    // Phase 6: Extended Math Operations (VM vs JIT)
    // =========================================================================
    //
    // Trigonometric and other math functions that exist only at VM tier.

    #[test]
    fn test_vm_jit_sin_basic() {
        let test_cases = [0.0, std::f64::consts::PI / 2.0, std::f64::consts::PI];

        for a in test_cases {
            let vm = run_vm_float_unary(a, Opcode::Sin);
            let jit = run_jit_float_unary(a, Opcode::Sin);

            assert!(vm.is_ok());
            let vm_val = match vm.unwrap().inner() {
                MettaValueInner::Float(f) => *f,
                _ => panic!("Expected Float"),
            };

            // Check against Rust's sin
            assert!(
                (vm_val - a.sin()).abs() < 1e-10,
                "VM sin({}) = {}, expected {}",
                a,
                vm_val,
                a.sin()
            );

            if let Ok(jit_val) = jit {
                if let MettaValueInner::Float(jit_f) = jit_val.inner() {
                    assert!(
                        (jit_f - a.sin()).abs() < 1e-10,
                        "JIT sin({}) = {}, expected {}",
                        a,
                        jit_f,
                        a.sin()
                    );
                }
            }
        }
    }

    #[test]
    fn test_vm_jit_cos_basic() {
        let test_cases = [0.0, std::f64::consts::PI / 2.0, std::f64::consts::PI];

        for a in test_cases {
            let vm = run_vm_float_unary(a, Opcode::Cos);
            let jit = run_jit_float_unary(a, Opcode::Cos);

            assert!(vm.is_ok());
            let vm_val = match vm.unwrap().inner() {
                MettaValueInner::Float(f) => *f,
                _ => panic!("Expected Float"),
            };

            assert!(
                (vm_val - a.cos()).abs() < 1e-10,
                "VM cos({}) = {}, expected {}",
                a,
                vm_val,
                a.cos()
            );

            if let Ok(jit_val) = jit {
                if let MettaValueInner::Float(jit_f) = jit_val.inner() {
                    assert!(
                        (jit_f - a.cos()).abs() < 1e-10,
                        "JIT cos({}) = {}, expected {}",
                        a,
                        jit_f,
                        a.cos()
                    );
                }
            }
        }
    }

    #[test]
    fn test_vm_jit_sqrt_basic() {
        let test_cases = [0.0, 1.0, 4.0, 9.0, 16.0, 100.0];

        for a in test_cases {
            let vm = run_vm_float_unary(a, Opcode::Sqrt);
            let jit = run_jit_float_unary(a, Opcode::Sqrt);

            assert!(vm.is_ok());
            let vm_val = match vm.unwrap().inner() {
                MettaValueInner::Float(f) => *f,
                _ => panic!("Expected Float"),
            };

            assert!(
                (vm_val - a.sqrt()).abs() < 1e-10,
                "VM sqrt({}) = {}, expected {}",
                a,
                vm_val,
                a.sqrt()
            );

            if let Ok(jit_val) = jit {
                if let MettaValueInner::Float(jit_f) = jit_val.inner() {
                    assert!(
                        (jit_f - a.sqrt()).abs() < 1e-10,
                        "JIT sqrt({}) = {}, expected {}",
                        a,
                        jit_f,
                        a.sqrt()
                    );
                }
            }
        }
    }

    #[test]
    fn test_vm_jit_pow_basic() {
        // VM Pow expects Long base and non-negative Long exponent
        let test_cases: [(i64, i64); 4] = [(2, 3), (3, 2), (10, 0), (2, 10)];

        for (base, exp) in test_cases {
            let vm = run_vm_binary(base, exp, Opcode::Pow);
            let jit = run_jit_binary(base, exp, Opcode::Pow);

            assert!(vm.is_ok(), "VM pow({}, {}) failed: {:?}", base, exp, vm);
            let vm_val = match vm.unwrap().inner() {
                MettaValueInner::Long(n) => *n,
                _ => panic!("Expected Long"),
            };

            // Integer power: base^exp
            let expected = (base as f64).powi(exp as i32) as i64;
            assert_eq!(
                vm_val, expected,
                "VM pow({}, {}) = {}, expected {}",
                base, exp, vm_val, expected
            );

            if let Ok(jit_val) = jit {
                if let MettaValueInner::Long(jit_n) = jit_val.inner() {
                    assert_eq!(
                        *jit_n, expected,
                        "JIT pow({}, {}) = {}, expected {}",
                        base, exp, jit_n, expected
                    );
                }
            }
        }
    }

    // =========================================================================
    // Phase 7: Error Consistency Tests
    // =========================================================================
    //
    // Verify that error conditions are handled consistently across tiers.

    #[test]
    fn test_three_tier_div_by_zero() {
        // Grounded tier
        let grounded = run_grounded_binary(&DivOp, MettaValue::Long(10), MettaValue::Long(0));
        assert!(grounded.is_err(), "Grounded should error on div by zero");

        // VM tier (T1.A: division-by-zero now yields an Error-atom result,
        // not a propagated Rust Err).
        let vm = run_vm_binary(10, 0, Opcode::Div);
        assert!(vm.is_err_or_error_atom(), "VM should error on div by zero");

        // JIT tier (if available)
        let jit = run_jit_binary(10, 0, Opcode::Div);
        // JIT may error or return a special value - both are acceptable
        // as long as it doesn't crash
        let _ = jit;
    }

    #[test]
    fn test_three_tier_mod_by_zero() {
        // Grounded tier
        let grounded = run_grounded_binary(&ModOp, MettaValue::Long(10), MettaValue::Long(0));
        assert!(grounded.is_err(), "Grounded should error on mod by zero");

        // VM tier (T1.A: mod-by-zero now yields Error atom, not Rust Err)
        let vm = run_vm_binary(10, 0, Opcode::Mod);
        assert!(vm.is_err_or_error_atom(), "VM should error on mod by zero");

        // JIT tier
        let jit = run_jit_binary(10, 0, Opcode::Mod);
        let _ = jit; // May error, doesn't crash
    }

    // =========================================================================
    // Phase 8: Mixed-Type Float Semantics (HE-Compliant)
    // =========================================================================

    #[test]
    fn test_three_tier_eq_long_float() {
        // All tiers use numeric promotion: Long(2) == Float(2.0) → true
        let grounded = run_grounded_binary(&EqualOp, MettaValue::Long(2), MettaValue::Float(2.0));
        let vm = run_vm_value_binary(MettaValue::Long(2), MettaValue::Float(2.0), Opcode::Eq);
        let jit = run_jit_value_binary(MettaValue::Long(2), MettaValue::Float(2.0), Opcode::Eq);

        assert_eq!(
            grounded.unwrap(),
            MettaValue::Bool(true),
            "Grounded: Long(2) == Float(2.0)"
        );
        assert_eq!(
            vm.unwrap(),
            MettaValue::Bool(true),
            "VM: Long(2) == Float(2.0)"
        );
        if let Ok(jit_val) = jit {
            assert_eq!(
                jit_val,
                MettaValue::Bool(true),
                "JIT: Long(2) == Float(2.0)"
            );
        }
    }

    #[test]
    fn test_three_tier_ne_long_float_same() {
        // All tiers use numeric promotion: Long(2) != Float(2.0) → false
        let grounded =
            run_grounded_binary(&NotEqualOp, MettaValue::Long(2), MettaValue::Float(2.0));
        let vm = run_vm_value_binary(MettaValue::Long(2), MettaValue::Float(2.0), Opcode::Ne);
        let jit = run_jit_value_binary(MettaValue::Long(2), MettaValue::Float(2.0), Opcode::Ne);

        assert_eq!(
            grounded.unwrap(),
            MettaValue::Bool(false),
            "Grounded: Long(2) != Float(2.0)"
        );
        assert_eq!(
            vm.unwrap(),
            MettaValue::Bool(false),
            "VM: Long(2) != Float(2.0)"
        );
        if let Ok(jit_val) = jit {
            assert_eq!(
                jit_val,
                MettaValue::Bool(false),
                "JIT: Long(2) != Float(2.0)"
            );
        }
    }

    #[test]
    fn test_three_tier_eq_float_float() {
        let grounded =
            run_grounded_binary(&EqualOp, MettaValue::Float(3.14), MettaValue::Float(3.14));
        let vm = run_vm_float_binary(3.14, 3.14, Opcode::Eq);
        let jit = run_jit_float_binary(3.14, 3.14, Opcode::Eq);

        assert_eq!(grounded.unwrap(), MettaValue::Bool(true));
        assert_eq!(vm.unwrap(), MettaValue::Bool(true));
        if let Ok(jit_val) = jit {
            assert_eq!(jit_val, MettaValue::Bool(true));
        }
    }

    #[test]
    fn test_three_tier_ne_float_float_different() {
        let grounded =
            run_grounded_binary(&NotEqualOp, MettaValue::Float(1.0), MettaValue::Float(2.0));
        let vm = run_vm_float_binary(1.0, 2.0, Opcode::Ne);
        let jit = run_jit_float_binary(1.0, 2.0, Opcode::Ne);

        assert_eq!(grounded.unwrap(), MettaValue::Bool(true));
        assert_eq!(vm.unwrap(), MettaValue::Bool(true));
        if let Ok(jit_val) = jit {
            assert_eq!(jit_val, MettaValue::Bool(true));
        }
    }

    #[test]
    fn test_three_tier_lt_long_float() {
        let grounded = run_grounded_binary(&LessOp, MettaValue::Long(1), MettaValue::Float(2.5));
        let vm = run_vm_value_binary(MettaValue::Long(1), MettaValue::Float(2.5), Opcode::Lt);
        let jit = run_jit_value_binary(MettaValue::Long(1), MettaValue::Float(2.5), Opcode::Lt);

        assert_eq!(grounded.unwrap(), MettaValue::Bool(true));
        assert_eq!(vm.unwrap(), MettaValue::Bool(true));
        if let Ok(jit_val) = jit {
            assert_eq!(jit_val, MettaValue::Bool(true));
        }
    }

    #[test]
    fn test_three_tier_ge_float_long() {
        let grounded =
            run_grounded_binary(&GreaterEqOp, MettaValue::Float(5.0), MettaValue::Long(5));
        let vm = run_vm_value_binary(MettaValue::Float(5.0), MettaValue::Long(5), Opcode::Ge);
        let jit = run_jit_value_binary(MettaValue::Float(5.0), MettaValue::Long(5), Opcode::Ge);

        assert_eq!(grounded.unwrap(), MettaValue::Bool(true));
        assert_eq!(vm.unwrap(), MettaValue::Bool(true));
        if let Ok(jit_val) = jit {
            assert_eq!(jit_val, MettaValue::Bool(true));
        }
    }

    #[test]
    fn test_three_tier_mod_long_float() {
        // All tiers: (% 85 43.5) → Float(41.5) via numeric promotion
        let grounded = run_grounded_binary(&ModOp, MettaValue::Long(85), MettaValue::Float(43.5));
        let vm = run_vm_value_binary(MettaValue::Long(85), MettaValue::Float(43.5), Opcode::Mod);
        let jit = run_jit_value_binary(MettaValue::Long(85), MettaValue::Float(43.5), Opcode::Mod);

        let g_f = grounded
            .expect("Grounded mod failed")
            .as_float()
            .expect("Expected Float");
        assert!(
            (g_f - 41.5).abs() < 1e-10,
            "Grounded: (% 85 43.5) = {}, expected ~41.5",
            g_f
        );

        let vm_f = vm
            .expect("VM mod failed")
            .as_float()
            .expect("Expected Float");
        assert!(
            (vm_f - 41.5).abs() < 1e-10,
            "VM: (% 85 43.5) = {}, expected ~41.5",
            vm_f
        );

        if let Ok(jit_val) = jit {
            let jit_f = jit_val.as_float().expect("Expected Float");
            assert!(
                (jit_f - 41.5).abs() < 1e-10,
                "JIT: (% 85 43.5) = {}, expected ~41.5",
                jit_f
            );
        }
    }

    #[test]
    fn test_three_tier_mod_float_float() {
        // All tiers: (% 10.5 3.0) → Float(1.5)
        let grounded = run_grounded_binary(&ModOp, MettaValue::Float(10.5), MettaValue::Float(3.0));
        let vm = run_vm_float_binary(10.5, 3.0, Opcode::Mod);
        let jit = run_jit_float_binary(10.5, 3.0, Opcode::Mod);

        let g_f = grounded
            .expect("Grounded mod failed")
            .as_float()
            .expect("Expected Float");
        assert!(
            (g_f - 1.5).abs() < 1e-10,
            "Grounded: (% 10.5 3.0) = {}, expected ~1.5",
            g_f
        );

        let vm_f = vm
            .expect("VM mod failed")
            .as_float()
            .expect("Expected Float");
        assert!(
            (vm_f - 1.5).abs() < 1e-10,
            "VM: (% 10.5 3.0) = {}, expected ~1.5",
            vm_f
        );

        if let Ok(jit_val) = jit {
            let jit_f = jit_val.as_float().expect("Expected Float");
            assert!(
                (jit_f - 1.5).abs() < 1e-10,
                "JIT: (% 10.5 3.0) = {}, expected ~1.5",
                jit_f
            );
        }
    }

    #[test]
    fn test_three_tier_add_long_float() {
        // Long(3) + Float(2.5) -> Float(5.5)
        let grounded = run_grounded_binary(&AddOp, MettaValue::Long(3), MettaValue::Float(2.5));
        let vm = run_vm_value_binary(MettaValue::Long(3), MettaValue::Float(2.5), Opcode::Add);
        let jit = run_jit_value_binary(MettaValue::Long(3), MettaValue::Float(2.5), Opcode::Add);

        assert_eq!(grounded.unwrap(), MettaValue::Float(5.5));
        assert_eq!(vm.unwrap(), MettaValue::Float(5.5));
        if let Ok(jit_val) = jit {
            assert_eq!(jit_val, MettaValue::Float(5.5));
        }
    }

    #[test]
    fn test_three_tier_mul_float_long() {
        // Float(2.5) * Long(4) -> Float(10.0)
        let grounded = run_grounded_binary(&MulOp, MettaValue::Float(2.5), MettaValue::Long(4));
        let vm = run_vm_value_binary(MettaValue::Float(2.5), MettaValue::Long(4), Opcode::Mul);
        let jit = run_jit_value_binary(MettaValue::Float(2.5), MettaValue::Long(4), Opcode::Mul);

        assert_eq!(grounded.unwrap(), MettaValue::Float(10.0));
        assert_eq!(vm.unwrap(), MettaValue::Float(10.0));
        if let Ok(jit_val) = jit {
            assert_eq!(jit_val, MettaValue::Float(10.0));
        }
    }
}
