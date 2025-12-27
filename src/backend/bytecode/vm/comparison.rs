//! Comparison and boolean operations for the bytecode VM.
//!
//! This module contains methods for comparison operations (lt, le, gt, ge, eq, ne)
//! and boolean operations (and, or, not, xor).

use super::types::{VmError, VmResult};
use super::BytecodeVM;
use crate::backend::models::MettaValue;

impl BytecodeVM {
    // === Comparison Operations ===

    pub(super) fn op_lt(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x < y),
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

    pub(super) fn op_le(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x <= y),
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

    pub(super) fn op_gt(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x > y),
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

    pub(super) fn op_ge(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x >= y),
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

    pub(super) fn op_eq(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push(MettaValue::Bool(a == b));
        Ok(())
    }

    pub(super) fn op_ne(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push(MettaValue::Bool(a != b));
        Ok(())
    }

    pub(super) fn op_struct_eq(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        // Structural equality (same as == for now)
        self.push(MettaValue::Bool(a == b));
        Ok(())
    }

    // === Boolean Operations ===

    pub(super) fn op_and(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Bool(x), MettaValue::Bool(y)) => MettaValue::Bool(*x && *y),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Bool",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_or(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Bool(x), MettaValue::Bool(y)) => MettaValue::Bool(*x || *y),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Bool",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_not(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Bool(x) => MettaValue::Bool(!x),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Bool",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_xor(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Bool(x), MettaValue::Bool(y)) => MettaValue::Bool(*x ^ *y),
            _ => {
                return Err(VmError::TypeError {
                    expected: "Bool",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    // === Type Operations ===

    pub(super) fn op_get_type(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let type_name = value.type_name();
        self.push(MettaValue::sym(type_name));
        Ok(())
    }

    pub(super) fn op_check_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.pop()?;
        let expected = match &type_val {
            MettaValue::Atom(s) => s.as_str(),
            _ => {
                return Err(VmError::TypeError {
                    expected: "type symbol",
                    got: "other",
                })
            }
        };
        // Type variables match anything (consistent with tree-visitor)
        let matches = if expected.starts_with('$') {
            true
        } else {
            value.type_name() == expected
        };
        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    pub(super) fn op_is_type(&mut self) -> VmResult<()> {
        // Same as check_type for now
        self.op_check_type()
    }

    pub(super) fn op_assert_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.peek()?;
        let expected = match &type_val {
            MettaValue::Atom(s) => s.as_str(),
            _ => {
                return Err(VmError::TypeError {
                    expected: "type symbol",
                    got: "other",
                })
            }
        };
        if value.type_name() != expected {
            return Err(VmError::TypeError {
                expected: "matching type",
                got: value.type_name(),
            });
        }
        Ok(())
    }
}
