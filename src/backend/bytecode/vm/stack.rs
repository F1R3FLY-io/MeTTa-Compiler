//! Stack operations for the bytecode VM.
//!
//! This module contains methods for stack manipulation operations
//! like push, pop, peek, dup, swap, rot3, and over.

use tracing::trace;

use crate::backend::models::MettaValue;
use super::types::{VmError, VmResult};
use super::BytecodeVM;

impl BytecodeVM {
    // === Stack Operations ===

    #[inline]
    pub(super) fn push(&mut self, value: MettaValue) {
        self.value_stack.push(value);
    }

    #[inline]
    pub(super) fn pop(&mut self) -> VmResult<MettaValue> {
        self.value_stack.pop().ok_or(VmError::StackUnderflow)
    }

    #[inline]
    pub(super) fn peek(&self) -> VmResult<&MettaValue> {
        self.value_stack.last().ok_or(VmError::StackUnderflow)
    }

    #[inline]
    pub(super) fn peek_n(&self, n: usize) -> VmResult<&MettaValue> {
        let len = self.value_stack.len();
        if n >= len {
            return Err(VmError::StackUnderflow);
        }
        Ok(&self.value_stack[len - 1 - n])
    }

    pub(super) fn op_dup(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::stack", ip = self.ip, "dup");
        let value = self.peek()?.clone();
        self.push(value);
        Ok(())
    }

    pub(super) fn op_swap(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::stack", ip = self.ip, "swap");
        let len = self.value_stack.len();
        if len < 2 {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.swap(len - 1, len - 2);
        Ok(())
    }

    pub(super) fn op_rot3(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::stack", ip = self.ip, "rot3");
        let len = self.value_stack.len();
        if len < 3 {
            return Err(VmError::StackUnderflow);
        }
        // [a, b, c] -> [c, a, b]
        let c = self.value_stack.pop().expect("length checked");
        let b = self.value_stack.pop().expect("length checked");
        let a = self.value_stack.pop().expect("length checked");
        self.value_stack.push(c);
        self.value_stack.push(a);
        self.value_stack.push(b);
        Ok(())
    }

    pub(super) fn op_over(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::stack", ip = self.ip, "over");
        let value = self.peek_n(1)?.clone();
        self.push(value);
        Ok(())
    }

    pub(super) fn op_dup_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::stack", ip = self.ip, "dup_n");
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        for i in (len - n)..len {
            let value = self.value_stack[i].clone();
            self.push(value);
        }
        Ok(())
    }

    pub(super) fn op_pop_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::stack", ip = self.ip, "pop_n");
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.truncate(len - n);
        Ok(())
    }
}
