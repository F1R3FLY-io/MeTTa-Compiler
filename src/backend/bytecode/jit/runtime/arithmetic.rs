//! Arithmetic and math runtime functions for JIT compilation
//!
//! This module provides FFI-callable arithmetic operations including:
//! - Integer operations: pow, abs, signum
//! - Extended math: sqrt, log, trunc, ceil, floor, round
//! - Trigonometric: sin, cos, tan, asin, acos, atan
//! - Predicates: isnan, isinf

use std::cell::Cell;

use super::helpers::{box_long, extract_long_signed, metta_to_jit};
use crate::backend::bytecode::jit::types::JitValue;
use crate::backend::models::{numeric_equal, MettaValue, ValueView};

thread_local! {
    static JIT_TYPE_ERROR_FLAG: Cell<bool> = const { Cell::new(false) };
}

/// Signal a type error from within a JIT runtime function.
/// The HybridExecutor checks this flag after JIT execution completes.
pub fn signal_jit_type_error() {
    JIT_TYPE_ERROR_FLAG.with(|f| f.set(true));
}

/// Check and clear the type error flag. Returns true if a type error occurred.
pub fn check_and_clear_jit_type_error() -> bool {
    JIT_TYPE_ERROR_FLAG.with(|f| {
        let had_error = f.get();
        f.set(false);
        had_error
    })
}

// =============================================================================
// Integer Arithmetic Operations
// =============================================================================

/// Compute integer power: base^exp
///
/// Handles negative exponents by returning 0 (integer division truncation).
///
/// # Safety
/// The inputs must be valid NaN-boxed Long values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pow(base: u64, exp: u64) -> u64 {
    // Extract the actual values from NaN-boxed representation
    let base_val = extract_long_signed(base);
    let exp_val = extract_long_signed(exp);

    let result = if exp_val < 0 {
        // Integer power with negative exponent is 0 (except for base=1 or base=-1)
        match base_val {
            1 => 1,
            -1 => {
                if exp_val % 2 == 0 {
                    1
                } else {
                    -1
                }
            }
            _ => 0,
        }
    } else if exp_val == 0 {
        1
    } else {
        // Use binary exponentiation for efficiency
        let mut result: i64 = 1;
        let mut base = base_val;
        let mut exp = exp_val as u64;

        while exp > 0 {
            if exp & 1 == 1 {
                result = result.wrapping_mul(base);
            }
            base = base.wrapping_mul(base);
            exp >>= 1;
        }
        result
    };

    // Box result as Long
    box_long(result)
}

/// Integer absolute value
///
/// # Safety
/// The input must be a valid NaN-boxed Long value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_abs(val: u64) -> u64 {
    let n = extract_long_signed(val);
    box_long(n.abs())
}

/// Integer sign function: returns -1, 0, or 1
///
/// # Safety
/// The input must be a valid NaN-boxed Long value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_signum(val: u64) -> u64 {
    let n = extract_long_signed(val);
    let result = if n < 0 {
        -1
    } else if n > 0 {
        1
    } else {
        0
    };
    box_long(result)
}

// =============================================================================
// Extended Math Operations (PR #62)
// =============================================================================

/// Square root: sqrt(value) -> Float
///
/// # Safety
/// The input must be a valid NaN-boxed Long or heap pointer to Float.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_sqrt(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Float(x.sqrt()),
        ValueView::Long(x) => MettaValue::Float((x as f64).sqrt()),
        _ => {
            // BUG-T0-T1-012 (spec K.3.x): signal type error to hybrid executor
            // and return Error atom rather than a silent NaN sentinel (which
            // is indistinguishable from genuine IEEE-754 NaN).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Logarithm: log_base(value) -> Float
///
/// # Safety
/// The inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_log(base: u64, val: u64) -> u64 {
    let base_jv = JitValue::from_raw(base);
    let val_jv = JitValue::from_raw(val);
    let base_mv = base_jv.to_metta();
    let val_mv = val_jv.to_metta();

    let result = match (base_mv.view(), val_mv.view()) {
        (ValueView::Float(b), ValueView::Float(v)) => MettaValue::Float(v.log(b)),
        (ValueView::Long(b), ValueView::Float(v)) => MettaValue::Float(v.log(b as f64)),
        (ValueView::Float(b), ValueView::Long(v)) => MettaValue::Float((v as f64).log(b)),
        (ValueView::Long(b), ValueView::Long(v)) => MettaValue::Float((v as f64).log(b as f64)),
        _ => {
            // BUG-T0-T1-012 (spec K.3.x): signal type error to hybrid executor
            // and return Error atom rather than a silent NaN sentinel (which
            // is indistinguishable from genuine IEEE-754 NaN).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Truncate to integer: trunc(value) -> Long
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_trunc(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Long(x.trunc() as i64),
        ValueView::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                  // Type error
    };

    metta_to_jit(&result).to_bits()
}

/// Ceiling: ceil(value) -> Long
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_ceil(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Long(x.ceil() as i64),
        ValueView::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                  // Type error
    };

    metta_to_jit(&result).to_bits()
}

/// Floor: floor(value) -> Long
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_floor_math(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Long(x.floor() as i64),
        ValueView::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                  // Type error
    };

    metta_to_jit(&result).to_bits()
}

/// Round: round(value) -> Long
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_round(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Long(x.round() as i64),
        ValueView::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                  // Type error
    };

    metta_to_jit(&result).to_bits()
}

// =============================================================================
// Trigonometric Operations
// =============================================================================

/// Sine: sin(value) -> Float
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_sin(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Float(x.sin()),
        ValueView::Long(x) => MettaValue::Float((x as f64).sin()),
        _ => {
            // BUG-T0-T1-012: in-tier Error atom (not silent NaN sentinel).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Cosine: cos(value) -> Float
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cos(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Float(x.cos()),
        ValueView::Long(x) => MettaValue::Float((x as f64).cos()),
        _ => {
            // BUG-T0-T1-012: in-tier Error atom (not silent NaN sentinel).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Tangent: tan(value) -> Float
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_tan(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Float(x.tan()),
        ValueView::Long(x) => MettaValue::Float((x as f64).tan()),
        _ => {
            // BUG-T0-T1-012: in-tier Error atom (not silent NaN sentinel).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Arc sine: asin(value) -> Float
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_asin(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Float(x.asin()),
        ValueView::Long(x) => MettaValue::Float((x as f64).asin()),
        _ => {
            // BUG-T0-T1-012: in-tier Error atom (not silent NaN sentinel).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Arc cosine: acos(value) -> Float
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_acos(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Float(x.acos()),
        ValueView::Long(x) => MettaValue::Float((x as f64).acos()),
        _ => {
            // BUG-T0-T1-012: in-tier Error atom (not silent NaN sentinel).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Arc tangent: atan(value) -> Float
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_atan(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let result = match mv.view() {
        ValueView::Float(x) => MettaValue::Float(x.atan()),
        ValueView::Long(x) => MettaValue::Float((x as f64).atan()),
        _ => {
            // BUG-T0-T1-012: in-tier Error atom (not silent NaN sentinel).
            signal_jit_type_error();
            let factory = crate::backend::models::global_factory();
            use crate::backend::models::MettaValueFactory;
            factory.error("JIT type error", factory.atom("BadType"))
        }
    };

    metta_to_jit(&result).to_bits()
}

// =============================================================================
// Floating-Point Predicates
// =============================================================================

/// Check if value is NaN: isnan(value) -> Bool
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_isnan(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let is_nan = match mv.view() {
        ValueView::Float(x) => x.is_nan(),
        ValueView::Long(_) => false, // Integers are never NaN
        _ => false,                  // Non-numeric types are not NaN
    };

    JitValue::from_bool(is_nan).to_bits()
}

/// Check if value is infinite: isinf(value) -> Bool
///
/// # Safety
/// The input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_isinf(val: u64) -> u64 {
    let jv = JitValue::from_raw(val);
    let mv = jv.to_metta();

    let is_inf = match mv.view() {
        ValueView::Float(x) => x.is_infinite(),
        ValueView::Long(_) => false, // Integers are never infinite
        _ => false,                  // Non-numeric types are not infinite
    };

    JitValue::from_bool(is_inf).to_bits()
}

// =============================================================================
// Numeric Arithmetic with Type Promotion (Float Support)
// =============================================================================

/// Numeric addition with type promotion: Long+Long->Long, mixed/Float->Float
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_add(a: u64, b: u64) -> u64 {
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    match (a_mv.view(), b_mv.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => box_long(x.wrapping_add(y)),
        (ValueView::Float(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x + y)).to_bits()
        }
        (ValueView::Long(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x as f64 + y)).to_bits()
        }
        (ValueView::Float(x), ValueView::Long(y)) => {
            metta_to_jit(&MettaValue::Float(x + y as f64)).to_bits()
        }
        _ => {
            signal_jit_type_error();
            box_long(0) // Dummy value; result will be discarded after flag check
        }
    }
}

/// Numeric subtraction with type promotion
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_sub(a: u64, b: u64) -> u64 {
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    match (a_mv.view(), b_mv.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => box_long(x.wrapping_sub(y)),
        (ValueView::Float(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x - y)).to_bits()
        }
        (ValueView::Long(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x as f64 - y)).to_bits()
        }
        (ValueView::Float(x), ValueView::Long(y)) => {
            metta_to_jit(&MettaValue::Float(x - y as f64)).to_bits()
        }
        _ => {
            signal_jit_type_error();
            box_long(0)
        }
    }
}

/// Numeric multiplication with type promotion
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_mul(a: u64, b: u64) -> u64 {
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    match (a_mv.view(), b_mv.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => box_long(x.wrapping_mul(y)),
        (ValueView::Float(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x * y)).to_bits()
        }
        (ValueView::Long(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x as f64 * y)).to_bits()
        }
        (ValueView::Float(x), ValueView::Long(y)) => {
            metta_to_jit(&MettaValue::Float(x * y as f64)).to_bits()
        }
        _ => {
            signal_jit_type_error();
            box_long(0)
        }
    }
}

/// Numeric division with type promotion and zero-check
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_div(a: u64, b: u64) -> u64 {
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    // Spec §13.2: integer / 0 → DivisionByZero error; otherwise wrapping_div
    // (so i64::MIN / -1 wraps to i64::MIN). Float / 0.0 → IEEE 754 (±Inf or NaN),
    // no error.
    match (a_mv.view(), b_mv.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => {
            if y == 0 {
                return super::helpers::make_jit_error("Division by zero");
            }
            box_long(x.wrapping_div(y))
        }
        (ValueView::Float(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x / y)).to_bits()
        }
        (ValueView::Long(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x as f64 / y)).to_bits()
        }
        (ValueView::Float(x), ValueView::Long(y)) => {
            metta_to_jit(&MettaValue::Float(x / y as f64)).to_bits()
        }
        _ => {
            signal_jit_type_error();
            box_long(0)
        }
    }
}

/// Numeric modulo with type promotion and zero-check
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_mod(a: u64, b: u64) -> u64 {
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    // Spec §13.2: integer % 0 → DivisionByZero error; otherwise wrapping_rem
    // (so i64::MIN % -1 wraps to 0). Float % 0.0 → NaN per IEEE/HE, no error.
    match (a_mv.view(), b_mv.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => {
            if y == 0 {
                return super::helpers::make_jit_error("Modulo by zero");
            }
            box_long(x.wrapping_rem(y))
        }
        (ValueView::Float(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x % y)).to_bits()
        }
        (ValueView::Long(x), ValueView::Float(y)) => {
            metta_to_jit(&MettaValue::Float(x as f64 % y)).to_bits()
        }
        (ValueView::Float(x), ValueView::Long(y)) => {
            metta_to_jit(&MettaValue::Float(x % y as f64)).to_bits()
        }
        _ => {
            signal_jit_type_error();
            box_long(0)
        }
    }
}

/// Numeric negation with type promotion: Long->Long, Float->Float.
///
/// Spec §13.2: silent two's-complement wrap. -(i64::MIN) → i64::MIN.
///
/// # Safety
/// Input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_neg(a: u64) -> u64 {
    let jv = JitValue::from_raw(a);
    let mv = jv.to_metta();

    match mv.view() {
        ValueView::Long(x) => box_long(x.wrapping_neg()),
        ValueView::Float(x) => metta_to_jit(&MettaValue::Float(-x)).to_bits(),
        _ => {
            signal_jit_type_error();
            box_long(0)
        }
    }
}

/// Numeric absolute value with type promotion and wrap on overflow.
///
/// Spec is silent on abs(i64::MIN); wrapping_abs returns i64::MIN for
/// tier-consistency with bytecode VM and trampoline (no error).
///
/// # Safety
/// Input must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_abs(a: u64) -> u64 {
    let jv = JitValue::from_raw(a);
    let mv = jv.to_metta();

    match mv.view() {
        ValueView::Long(x) => box_long(x.wrapping_abs()),
        ValueView::Float(x) => metta_to_jit(&MettaValue::Float(x.abs())).to_bits(),
        _ => {
            signal_jit_type_error();
            box_long(0)
        }
    }
}

/// Helper for comparison operations with type promotion and string lex ordering.
///
/// BUG T0-T2-005 (plan T2/T3.B): String comparison via `str::cmp`-based ordering,
/// matching `grounded/comparison.rs:200-208` (T0) and `op_comparison` (T1).
#[inline]
unsafe fn numeric_cmp(
    a: u64,
    b: u64,
    int_cmp: fn(i64, i64) -> bool,
    float_cmp: fn(f64, f64) -> bool,
) -> u64 {
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    let result = match (a_mv.view(), b_mv.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => int_cmp(x, y),
        (ValueView::Float(x), ValueView::Float(y)) => float_cmp(x, y),
        (ValueView::Long(x), ValueView::Float(y)) => float_cmp(x as f64, y),
        (ValueView::Float(x), ValueView::Long(y)) => float_cmp(x, y as f64),
        // BUG T0-T2-005: String lex order via float_cmp-equivalent applied to
        // ord values. Map String::cmp to a `f64` proxy so the existing float
        // combinator semantics hold (Less = -1.0, Equal = 0.0, Greater = 1.0
        // vs 0.0). Cross-tier matches T1's `String::cmp` arm.
        (ValueView::String(x), ValueView::String(y)) => {
            let proxy = match x.cmp(y) {
                std::cmp::Ordering::Less => -1.0f64,
                std::cmp::Ordering::Equal => 0.0,
                std::cmp::Ordering::Greater => 1.0,
            };
            float_cmp(proxy, 0.0)
        }
        _ => {
            signal_jit_type_error();
            false
        }
    };

    JitValue::from_bool(result).to_bits()
}

/// Numeric less-than with type promotion
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_lt(a: u64, b: u64) -> u64 {
    numeric_cmp(a, b, |x, y| x < y, |x, y| x < y)
}

/// Numeric less-than-or-equal with type promotion
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_le(a: u64, b: u64) -> u64 {
    numeric_cmp(a, b, |x, y| x <= y, |x, y| x <= y)
}

/// Numeric greater-than with type promotion
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_gt(a: u64, b: u64) -> u64 {
    numeric_cmp(a, b, |x, y| x > y, |x, y| x > y)
}

/// Numeric greater-than-or-equal with type promotion
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_ge(a: u64, b: u64) -> u64 {
    numeric_cmp(a, b, |x, y| x >= y, |x, y| x >= y)
}

/// Numeric equality with type promotion and epsilon tolerance
///
/// Uses `numeric_equal()` for MeTTa HE-compatible semantics:
/// Long(2) == Float(2.0) -> true.
///
/// # Safety
/// Inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_numeric_eq(a: u64, b: u64) -> u64 {
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    JitValue::from_bool(numeric_equal(&a_mv, &b_mv)).to_bits()
}
