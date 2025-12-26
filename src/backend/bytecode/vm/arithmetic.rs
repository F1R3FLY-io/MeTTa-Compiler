//! Arithmetic operations for the bytecode VM.
//!
//! This module contains methods for arithmetic operations
//! like add, sub, mul, div, mod, neg, abs, pow, and extended math operations.

use crate::backend::models::MettaValue;
use super::types::{VmError, VmResult};
use super::BytecodeVM;

impl BytecodeVM {
    // === Basic Arithmetic Operations ===

    pub(super) fn op_add(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x + y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_sub(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x - y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_mul(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x * y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_div(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(_), MettaValue::Long(0)) => return Err(VmError::DivisionByZero),
            (MettaValue::Long(x), MettaValue::Long(y)) => {
                match x.checked_div(*y) {
                    Some(r) => MettaValue::Long(r),
                    None => return Err(VmError::ArithmeticOverflow),
                }
            }
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_mod(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(_), MettaValue::Long(0)) => return Err(VmError::DivisionByZero),
            (MettaValue::Long(x), MettaValue::Long(y)) => {
                match x.checked_rem(*y) {
                    Some(r) => MettaValue::Long(r),
                    None => return Err(VmError::ArithmeticOverflow),
                }
            }
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_neg(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Long(x) => MettaValue::Long(-x),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_abs(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Long(x) => {
                // i64::MIN.abs() overflows because |i64::MIN| > i64::MAX
                if x == i64::MIN {
                    return Err(VmError::ArithmeticOverflow);
                }
                MettaValue::Long(x.abs())
            }
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_floor_div(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(_), MettaValue::Long(0)) => return Err(VmError::DivisionByZero),
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x.div_euclid(*y)),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_pow(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) if *y >= 0 => {
                MettaValue::Long(x.pow(*y as u32))
            }
            _ => return Err(VmError::TypeError { expected: "Long with non-negative exponent", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    // === Extended Math Operations (PR #62) ===

    pub(super) fn op_sqrt(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Float(x.sqrt()),
            MettaValue::Long(x) => MettaValue::Float((x as f64).sqrt()),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_log(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let base = self.pop()?;
        let result = match (&base, &value) {
            (MettaValue::Float(b), MettaValue::Float(v)) => MettaValue::Float(v.log(*b)),
            (MettaValue::Long(b), MettaValue::Float(v)) => MettaValue::Float(v.log(*b as f64)),
            (MettaValue::Float(b), MettaValue::Long(v)) => MettaValue::Float((*v as f64).log(*b)),
            (MettaValue::Long(b), MettaValue::Long(v)) => MettaValue::Float((*v as f64).log(*b as f64)),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_trunc(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Long(x.trunc() as i64),
            MettaValue::Long(x) => MettaValue::Long(x),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_ceil(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Long(x.ceil() as i64),
            MettaValue::Long(x) => MettaValue::Long(x),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_floor_math(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Long(x.floor() as i64),
            MettaValue::Long(x) => MettaValue::Long(x),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_round(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Long(x.round() as i64),
            MettaValue::Long(x) => MettaValue::Long(x),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    // === Trigonometric Operations ===

    pub(super) fn op_sin(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Float(x.sin()),
            MettaValue::Long(x) => MettaValue::Float((x as f64).sin()),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_cos(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Float(x.cos()),
            MettaValue::Long(x) => MettaValue::Float((x as f64).cos()),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_tan(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Float(x.tan()),
            MettaValue::Long(x) => MettaValue::Float((x as f64).tan()),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_asin(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Float(x.asin()),
            MettaValue::Long(x) => MettaValue::Float((x as f64).asin()),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_acos(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Float(x.acos()),
            MettaValue::Long(x) => MettaValue::Float((x as f64).acos()),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_atan(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Float(x.atan()),
            MettaValue::Long(x) => MettaValue::Float((x as f64).atan()),
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_isnan(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Bool(x.is_nan()),
            MettaValue::Long(_) => MettaValue::Bool(false), // integers are never NaN
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_isinf(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Float(x) => MettaValue::Bool(x.is_infinite()),
            MettaValue::Long(_) => MettaValue::Bool(false), // integers are never infinite
            _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }
}
