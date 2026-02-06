//! Arithmetic operations for the bytecode VM.
//!
//! This module contains methods for arithmetic operations
//! like add, sub, mul, div, mod, neg, abs, pow, and extended math operations.

use super::types::{VmError, VmResult};
use super::BytecodeVM;
use crate::backend::models::{MettaValue, MettaValueInner};

impl BytecodeVM {
    // === Basic Arithmetic Operations ===

    pub(super) fn op_add(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (a.inner(), b.inner()) {
            (MettaValueInner::Long(x), MettaValueInner::Long(y)) => {
                MettaValue::Long(x.wrapping_add(*y))
            }
            (MettaValueInner::Float(x), MettaValueInner::Float(y)) => MettaValue::Float(x + y),
            (MettaValueInner::Long(x), MettaValueInner::Float(y)) => {
                MettaValue::Float(*x as f64 + y)
            }
            (MettaValueInner::Float(x), MettaValueInner::Long(y)) => {
                MettaValue::Float(x + *y as f64)
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_sub(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (a.inner(), b.inner()) {
            (MettaValueInner::Long(x), MettaValueInner::Long(y)) => {
                MettaValue::Long(x.wrapping_sub(*y))
            }
            (MettaValueInner::Float(x), MettaValueInner::Float(y)) => MettaValue::Float(x - y),
            (MettaValueInner::Long(x), MettaValueInner::Float(y)) => {
                MettaValue::Float(*x as f64 - y)
            }
            (MettaValueInner::Float(x), MettaValueInner::Long(y)) => {
                MettaValue::Float(x - *y as f64)
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_mul(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (a.inner(), b.inner()) {
            (MettaValueInner::Long(x), MettaValueInner::Long(y)) => {
                MettaValue::Long(x.wrapping_mul(*y))
            }
            (MettaValueInner::Float(x), MettaValueInner::Float(y)) => MettaValue::Float(x * y),
            (MettaValueInner::Long(x), MettaValueInner::Float(y)) => {
                MettaValue::Float(*x as f64 * y)
            }
            (MettaValueInner::Float(x), MettaValueInner::Long(y)) => {
                MettaValue::Float(x * *y as f64)
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_div(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (a.inner(), b.inner()) {
            (MettaValueInner::Long(_), MettaValueInner::Long(0)) => {
                return Err(VmError::DivisionByZero)
            }
            (MettaValueInner::Long(x), MettaValueInner::Long(y)) => match x.checked_div(*y) {
                Some(r) => MettaValue::Long(r),
                None => return Err(VmError::ArithmeticOverflow),
            },
            (MettaValueInner::Float(x), MettaValueInner::Float(y)) => {
                if *y == 0.0 {
                    return Err(VmError::DivisionByZero);
                }
                MettaValue::Float(x / y)
            }
            (MettaValueInner::Long(x), MettaValueInner::Float(y)) => {
                if *y == 0.0 {
                    return Err(VmError::DivisionByZero);
                }
                MettaValue::Float(*x as f64 / y)
            }
            (MettaValueInner::Float(x), MettaValueInner::Long(y)) => {
                if *y == 0 {
                    return Err(VmError::DivisionByZero);
                }
                MettaValue::Float(x / *y as f64)
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_mod(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (a.inner(), b.inner()) {
            (MettaValueInner::Long(_), MettaValueInner::Long(0)) => {
                return Err(VmError::DivisionByZero)
            }
            (MettaValueInner::Long(x), MettaValueInner::Long(y)) => match x.checked_rem(*y) {
                Some(r) => MettaValue::Long(r),
                None => return Err(VmError::ArithmeticOverflow),
            },
            _ => {
                return Err(VmError::TypeError {
                    expected: "Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_neg(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Long(x) => MettaValue::Long(-x),
            MettaValueInner::Float(x) => MettaValue::Float(-x),
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_abs(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Long(x) => {
                // i64::MIN.abs() overflows because |i64::MIN| > i64::MAX
                if *x == i64::MIN {
                    return Err(VmError::ArithmeticOverflow);
                }
                MettaValue::Long(x.abs())
            }
            MettaValueInner::Float(x) => MettaValue::Float(x.abs()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_floor_div(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (a.inner(), b.inner()) {
            (MettaValueInner::Long(_), MettaValueInner::Long(0)) => {
                return Err(VmError::DivisionByZero)
            }
            (MettaValueInner::Long(x), MettaValueInner::Long(y)) => {
                MettaValue::Long(x.div_euclid(*y))
            }
            (MettaValueInner::Float(x), MettaValueInner::Float(y)) => {
                if *y == 0.0 {
                    return Err(VmError::DivisionByZero);
                }
                MettaValue::Long((x / y).floor() as i64)
            }
            (MettaValueInner::Long(x), MettaValueInner::Float(y)) => {
                if *y == 0.0 {
                    return Err(VmError::DivisionByZero);
                }
                MettaValue::Long((*x as f64 / y).floor() as i64)
            }
            (MettaValueInner::Float(x), MettaValueInner::Long(y)) => {
                if *y == 0 {
                    return Err(VmError::DivisionByZero);
                }
                MettaValue::Long((x / *y as f64).floor() as i64)
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_pow(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (a.inner(), b.inner()) {
            (MettaValueInner::Long(x), MettaValueInner::Long(y)) if *y >= 0 => {
                MettaValue::Long(x.pow(*y as u32))
            }
            (MettaValueInner::Float(x), MettaValueInner::Float(y)) => MettaValue::Float(x.powf(*y)),
            (MettaValueInner::Long(x), MettaValueInner::Float(y)) => {
                MettaValue::Float((*x as f64).powf(*y))
            }
            (MettaValueInner::Float(x), MettaValueInner::Long(y)) => {
                MettaValue::Float(x.powi(*y as i32))
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "number (Long or Float)",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    // === Extended Math Operations (PR #62) ===

    pub(super) fn op_sqrt(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Float(x.sqrt()),
            MettaValueInner::Long(x) => MettaValue::Float((*x as f64).sqrt()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_log(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let base = self.pop()?;
        let result = match (base.inner(), value.inner()) {
            (MettaValueInner::Float(b), MettaValueInner::Float(v)) => MettaValue::Float(v.log(*b)),
            (MettaValueInner::Long(b), MettaValueInner::Float(v)) => {
                MettaValue::Float(v.log(*b as f64))
            }
            (MettaValueInner::Float(b), MettaValueInner::Long(v)) => {
                MettaValue::Float((*v as f64).log(*b))
            }
            (MettaValueInner::Long(b), MettaValueInner::Long(v)) => {
                MettaValue::Float((*v as f64).log(*b as f64))
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_trunc(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Long(x.trunc() as i64),
            MettaValueInner::Long(x) => MettaValue::Long(*x),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_ceil(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Long(x.ceil() as i64),
            MettaValueInner::Long(x) => MettaValue::Long(*x),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_floor_math(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Long(x.floor() as i64),
            MettaValueInner::Long(x) => MettaValue::Long(*x),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_round(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Long(x.round() as i64),
            MettaValueInner::Long(x) => MettaValue::Long(*x),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    // === Trigonometric Operations ===

    pub(super) fn op_sin(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Float(x.sin()),
            MettaValueInner::Long(x) => MettaValue::Float((*x as f64).sin()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_cos(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Float(x.cos()),
            MettaValueInner::Long(x) => MettaValue::Float((*x as f64).cos()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_tan(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Float(x.tan()),
            MettaValueInner::Long(x) => MettaValue::Float((*x as f64).tan()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_asin(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Float(x.asin()),
            MettaValueInner::Long(x) => MettaValue::Float((*x as f64).asin()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_acos(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Float(x.acos()),
            MettaValueInner::Long(x) => MettaValue::Float((*x as f64).acos()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_atan(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Float(x.atan()),
            MettaValueInner::Long(x) => MettaValue::Float((*x as f64).atan()),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_isnan(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Bool(x.is_nan()),
            MettaValueInner::Long(_) => MettaValue::Bool(false), // integers are never NaN
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_isinf(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a.inner() {
            MettaValueInner::Float(x) => MettaValue::Bool(x.is_infinite()),
            MettaValueInner::Long(_) => MettaValue::Bool(false), // integers are never infinite
            _ => {
                return Err(VmError::TypeError {
                    expected: "Float or Long",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }
}
