//! Value creation and manipulation operations for the bytecode VM.
//!
//! This module contains methods for creating and manipulating values
//! such as pushing constants, making S-expressions, and variable operations.

use crate::backend::models::MettaValue;
use super::types::{VmError, VmResult, BindingFrame};
use super::BytecodeVM;

impl BytecodeVM {
    // === Value Creation ===

    pub(super) fn op_push_long_small(&mut self) -> VmResult<()> {
        let value = self.read_i8()? as i64;
        self.push(MettaValue::Long(value));
        Ok(())
    }

    pub(super) fn op_push_constant(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let value = self.chunk.get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?
            .clone();
        self.push(value);
        Ok(())
    }

    /// Push a variable, resolving from bindings if available
    ///
    /// For pattern variables ($x), this checks the binding stack first.
    /// If found in bindings, pushes the bound value. Otherwise pushes
    /// the variable symbol as-is (for irreducible expressions).
    pub(super) fn op_push_variable(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let value = self.chunk.get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?;

        // Check if it's a pattern variable that should be resolved from bindings
        if let MettaValue::Atom(name) = value {
            if name.starts_with('$') {
                // Search bindings from innermost to outermost
                for frame in self.bindings_stack.iter().rev() {
                    if let Some(bound_value) = frame.get(name) {
                        self.push(bound_value.clone());
                        return Ok(());
                    }
                }
            }
        }

        // Not found in bindings or not a pattern variable - push as-is
        self.push(value.clone());
        Ok(())
    }

    pub(super) fn op_make_sexpr(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        self.make_sexpr_impl(arity)
    }

    pub(super) fn op_make_sexpr_large(&mut self) -> VmResult<()> {
        let arity = self.read_u16()? as usize;
        self.make_sexpr_impl(arity)
    }

    pub(super) fn make_sexpr_impl(&mut self, arity: usize) -> VmResult<()> {
        let len = self.value_stack.len();
        if arity > len {
            return Err(VmError::StackUnderflow);
        }
        let elements: Vec<MettaValue> = self.value_stack.drain((len - arity)..).collect();
        self.push(MettaValue::sexpr(elements));
        Ok(())
    }

    pub(super) fn op_make_list(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if arity > len {
            return Err(VmError::StackUnderflow);
        }
        let elements: Vec<MettaValue> = self.value_stack.drain((len - arity)..).collect();
        // Build proper list
        let mut list = MettaValue::Nil;
        for elem in elements.into_iter().rev() {
            list = MettaValue::sexpr(vec![
                MettaValue::sym("Cons"),
                elem,
                list,
            ]);
        }
        self.push(list);
        Ok(())
    }

    pub(super) fn op_make_quote(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        self.push(MettaValue::sexpr(vec![
            MettaValue::sym("quote"),
            value,
        ]));
        Ok(())
    }

    // === Variable Operations ===

    pub(super) fn op_load_local(&mut self) -> VmResult<()> {
        let index = self.read_u8()? as usize;
        self.load_local_impl(index)
    }

    pub(super) fn op_load_local_wide(&mut self) -> VmResult<()> {
        let index = self.read_u16()? as usize;
        self.load_local_impl(index)
    }

    pub(super) fn load_local_impl(&mut self, index: usize) -> VmResult<()> {
        let base = self.call_stack.last()
            .map(|f| f.base_ptr)
            .unwrap_or(0);
        let abs_index = base + index;
        if abs_index >= self.value_stack.len() {
            return Err(VmError::InvalidLocal(index as u16));
        }
        let value = self.value_stack[abs_index].clone();
        self.push(value);
        Ok(())
    }

    pub(super) fn op_store_local(&mut self) -> VmResult<()> {
        let index = self.read_u8()? as usize;
        self.store_local_impl(index)
    }

    pub(super) fn op_store_local_wide(&mut self) -> VmResult<()> {
        let index = self.read_u16()? as usize;
        self.store_local_impl(index)
    }

    pub(super) fn store_local_impl(&mut self, index: usize) -> VmResult<()> {
        let value = self.pop()?;
        let base = self.call_stack.last()
            .map(|f| f.base_ptr)
            .unwrap_or(0);
        let abs_index = base + index;
        if abs_index >= self.value_stack.len() {
            return Err(VmError::InvalidLocal(index as u16));
        }
        self.value_stack[abs_index] = value;
        Ok(())
    }

    pub(super) fn op_load_binding(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = match self.chunk.get_constant(index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            _ => return Err(VmError::InvalidConstant(index)),
        };
        // Search bindings from innermost to outermost
        for frame in self.bindings_stack.iter().rev() {
            if let Some(value) = frame.get(&name) {
                self.push(value.clone());
                return Ok(());
            }
        }
        Err(VmError::InvalidBinding(name.clone()))
    }

    pub(super) fn op_store_binding(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let value = self.pop()?;
        let name = match self.chunk.get_constant(index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            _ => return Err(VmError::InvalidConstant(index)),
        };
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.set(name.clone(), value);
        }
        Ok(())
    }

    pub(super) fn op_has_binding(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = match self.chunk.get_constant(index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            _ => return Err(VmError::InvalidConstant(index)),
        };
        let exists = self.bindings_stack.iter()
            .rev()
            .any(|frame| frame.has(&name));
        self.push(MettaValue::Bool(exists));
        Ok(())
    }

    pub(super) fn op_clear_bindings(&mut self) {
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.clear();
        }
    }

    pub(super) fn op_push_binding_frame(&mut self) {
        let depth = self.bindings_stack.len() as u32;
        self.bindings_stack.push(BindingFrame::new(depth));
    }

    pub(super) fn op_pop_binding_frame(&mut self) -> VmResult<()> {
        if self.bindings_stack.len() <= 1 {
            return Err(VmError::Runtime("Cannot pop root binding frame".into()));
        }
        self.bindings_stack.pop();
        Ok(())
    }

    pub(super) fn op_load_upvalue(&mut self) -> VmResult<()> {
        let _operand = self.read_u16()?;
        // TODO: Implement upvalue loading
        Err(VmError::Runtime("Upvalues not yet implemented".into()))
    }
}
