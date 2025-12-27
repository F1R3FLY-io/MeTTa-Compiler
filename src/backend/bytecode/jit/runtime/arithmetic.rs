//! Arithmetic and math runtime functions for JIT compilation
//!
//! This module provides FFI-callable arithmetic operations including:
//! - Integer operations: pow, abs, signum
//! - Extended math: sqrt, log, trunc, ceil, floor, round
//! - Trigonometric: sin, cos, tan, asin, acos, atan
//! - Predicates: isnan, isinf

use super::helpers::{box_long, extract_long_signed, metta_to_jit};
use crate::backend::bytecode::jit::types::JitValue;
use crate::backend::models::MettaValue;

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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Float(x.sqrt()),
        MettaValue::Long(x) => MettaValue::Float((x as f64).sqrt()),
        _ => MettaValue::Float(f64::NAN), // Type error - return NaN
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

    let result = match (&base_mv, &val_mv) {
        (MettaValue::Float(b), MettaValue::Float(v)) => MettaValue::Float(v.log(*b)),
        (MettaValue::Long(b), MettaValue::Float(v)) => MettaValue::Float(v.log(*b as f64)),
        (MettaValue::Float(b), MettaValue::Long(v)) => MettaValue::Float((*v as f64).log(*b)),
        (MettaValue::Long(b), MettaValue::Long(v)) => MettaValue::Float((*v as f64).log(*b as f64)),
        _ => MettaValue::Float(f64::NAN), // Type error - return NaN
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Long(x.trunc() as i64),
        MettaValue::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                   // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Long(x.ceil() as i64),
        MettaValue::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                   // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Long(x.floor() as i64),
        MettaValue::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                   // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Long(x.round() as i64),
        MettaValue::Long(x) => MettaValue::Long(x), // Already an integer
        _ => MettaValue::Long(0),                   // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Float(x.sin()),
        MettaValue::Long(x) => MettaValue::Float((x as f64).sin()),
        _ => MettaValue::Float(f64::NAN), // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Float(x.cos()),
        MettaValue::Long(x) => MettaValue::Float((x as f64).cos()),
        _ => MettaValue::Float(f64::NAN), // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Float(x.tan()),
        MettaValue::Long(x) => MettaValue::Float((x as f64).tan()),
        _ => MettaValue::Float(f64::NAN), // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Float(x.asin()),
        MettaValue::Long(x) => MettaValue::Float((x as f64).asin()),
        _ => MettaValue::Float(f64::NAN), // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Float(x.acos()),
        MettaValue::Long(x) => MettaValue::Float((x as f64).acos()),
        _ => MettaValue::Float(f64::NAN), // Type error
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

    let result = match mv {
        MettaValue::Float(x) => MettaValue::Float(x.atan()),
        MettaValue::Long(x) => MettaValue::Float((x as f64).atan()),
        _ => MettaValue::Float(f64::NAN), // Type error
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

    let is_nan = match mv {
        MettaValue::Float(x) => x.is_nan(),
        MettaValue::Long(_) => false, // Integers are never NaN
        _ => false,                   // Non-numeric types are not NaN
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

    let is_inf = match mv {
        MettaValue::Float(x) => x.is_infinite(),
        MettaValue::Long(_) => false, // Integers are never infinite
        _ => false,                   // Non-numeric types are not infinite
    };

    JitValue::from_bool(is_inf).to_bits()
}
