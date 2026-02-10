//! Control flow operations for the bytecode VM.
//!
//! This module contains methods for control flow operations
//! like jumps, calls, returns, and rule dispatch.

use std::ops::ControlFlow;
use std::sync::Arc;
use tracing::trace;

use super::types::{Alternative, BindingFrame, CallFrame, ChoicePoint, VmError, VmResult};
use super::BytecodeVM;
use crate::backend::bytecode::chunk::BytecodeChunk;
use crate::backend::models::{Bindings, MettaValue, MettaValueInner};

impl BytecodeVM {
    // === Jump Operations ===

    pub(super) fn op_jump(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        self.ip = (self.ip as isize + offset as isize) as usize;
        Ok(())
    }

    pub(super) fn op_jump_if_false(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.pop()?;
        if matches!(cond.inner(), MettaValueInner::Bool(false)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    pub(super) fn op_jump_if_true(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.pop()?;
        if matches!(cond.inner(), MettaValueInner::Bool(true)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    pub(super) fn op_jump_if_unit(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.pop()?;
        if matches!(cond.inner(), MettaValueInner::Unit) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    pub(super) fn op_jump_if_error(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.peek()?;
        if matches!(cond.inner(), MettaValueInner::Error(..)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    pub(super) fn op_jump_short(&mut self) -> VmResult<()> {
        let offset = self.read_i8()?;
        self.ip = (self.ip as isize + offset as isize) as usize;
        Ok(())
    }

    pub(super) fn op_jump_if_false_short(&mut self) -> VmResult<()> {
        let offset = self.read_i8()?;
        let cond = self.pop()?;
        if matches!(cond.inner(), MettaValueInner::Bool(false)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    pub(super) fn op_jump_if_true_short(&mut self) -> VmResult<()> {
        let offset = self.read_i8()?;
        let cond = self.pop()?;
        if matches!(cond.inner(), MettaValueInner::Bool(true)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    /// Multi-way branch via jump table.
    ///
    /// Reads a table index from bytecode, pops a selector value from stack,
    /// looks up the corresponding offset in the jump table, and jumps to it.
    /// If the selector doesn't match any entry, jumps to the default offset.
    ///
    /// Stack: [selector] -> []
    /// Bytecode: JumpTable table_index:u16
    pub(super) fn op_jump_table(&mut self) -> VmResult<()> {
        let table_index = self.read_u16()? as usize;
        let selector = self.pop()?;

        // Get jump table from chunk
        let jump_table = self
            .chunk
            .get_jump_table(table_index)
            .ok_or_else(|| VmError::Runtime(format!("Invalid jump table index: {}", table_index)))?;

        // Compute hash of selector value for table lookup
        use xxhash_rust::xxh3::xxh3_64;
        // Hash the selector value - we use the string representation for consistent hashing
        let selector_hash = xxh3_64(format!("{:?}", selector).as_bytes());

        // Look up in jump table entries
        let target_offset = jump_table
            .entries
            .iter()
            .find(|(hash, _)| *hash == selector_hash)
            .map(|(_, offset)| *offset)
            .unwrap_or(jump_table.default_offset);

        // Jump to target
        self.ip = target_offset;
        Ok(())
    }

    // === Call Operations ===

    /// Execute a function call via MORK rule dispatch
    ///
    /// Opcode format: Call head_index:u16 arity:u8
    /// - Pops `arity` arguments from stack
    /// - Builds expression (head arg0 arg1 ...)
    /// - Dispatches to MORK for rule matching
    /// - Executes first matching rule body, or pushes expr if irreducible
    pub(super) fn op_call(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call");
        let head_index = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Get head symbol from constant pool
        let head_symbol = match self.chunk.get_constant(head_index) {
            Some(val) => match val.inner() {
                MettaValueInner::Atom(s) => s.clone(),
                other => {
                    return Err(VmError::Runtime(format!(
                        "Call head must be atom, got {:?}",
                        other
                    )));
                }
            },
            None => return Err(VmError::InvalidConstant(head_index)),
        };

        // Pop arguments from stack (they were pushed left-to-right)
        if self.value_stack.len() < arity {
            return Err(VmError::StackUnderflow);
        }
        let args: Vec<MettaValue> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(MettaValue::Atom(head_symbol));
        items.extend(args);
        let expr = MettaValue::SExpr(items);

        // Dispatch via MORK bridge if available
        if let Some(ref bridge) = self.bridge {
            let matches = bridge.dispatch_rules(&expr);

            if matches.is_empty() {
                // No rules match - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }

            if matches.len() == 1 {
                // Single match - execute directly (no Fork needed)
                let rule = &matches[0];
                return self.execute_rule_body(&rule.body, &rule.bindings);
            }

            // Multiple matches - create choice point for backtracking
            // First rule executes now, others become alternatives
            let alternatives: Vec<Alternative> = matches[1..]
                .iter()
                .map(|rule| Alternative::RuleMatch {
                    chunk: Arc::clone(&rule.body),
                    bindings: rule.bindings.clone(),
                })
                .collect();

            // Create choice point for backtracking to other matches
            let choice_point = ChoicePoint {
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                alternatives,
            };
            self.choice_points.push(choice_point);

            // Execute first matching rule
            let rule = &matches[0];
            return self.execute_rule_body(&rule.body, &rule.bindings);
        }

        // No bridge - return expression as data (irreducible)
        self.push(expr);
        Ok(())
    }

    /// Execute a tail call - same as call but reuses current call frame
    ///
    /// Opcode format: TailCall head_index:u16 arity:u8
    pub(super) fn op_tail_call(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "tail_call");
        let head_index = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Get head symbol from constant pool
        let head_symbol = match self.chunk.get_constant(head_index) {
            Some(val) => match val.inner() {
                MettaValueInner::Atom(s) => s.clone(),
                other => {
                    return Err(VmError::Runtime(format!(
                        "TailCall head must be atom, got {:?}",
                        other
                    )));
                }
            },
            None => return Err(VmError::InvalidConstant(head_index)),
        };

        // Pop arguments from stack
        if self.value_stack.len() < arity {
            return Err(VmError::StackUnderflow);
        }
        let args: Vec<MettaValue> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(MettaValue::Atom(head_symbol));
        items.extend(args);
        let expr = MettaValue::SExpr(items);

        // Dispatch via MORK bridge if available
        if let Some(ref bridge) = self.bridge {
            let matches = bridge.dispatch_rules(&expr);

            if matches.is_empty() {
                // No rules match - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }

            // TODO: Handle multiple matches via choice points (Fork)
            // For now, execute first matching rule with TCO
            let rule = &matches[0];
            return self.execute_rule_body_tail(&rule.body, &rule.bindings);
        }

        // No bridge - return expression as data (irreducible)
        self.push(expr);
        Ok(())
    }

    /// Execute a rule body by pushing a call frame and switching to the rule chunk
    pub(super) fn execute_rule_body(
        &mut self,
        body: &Arc<BytecodeChunk>,
        bindings: &Bindings,
    ) -> VmResult<()> {
        // Check call stack limit
        if self.call_stack.len() >= self.config.max_call_stack {
            return Err(VmError::CallStackOverflow);
        }

        // Push a new binding frame with pattern variables
        let scope_depth = self.bindings_stack.len() as u32;
        let mut frame = BindingFrame::new(scope_depth);
        for (name, value) in bindings.iter() {
            frame.set(name.clone(), value.clone());
        }
        self.bindings_stack.push(frame);

        // Push call frame to return to after rule execution
        let call_frame = CallFrame {
            return_ip: self.ip,
            return_chunk: Arc::clone(&self.chunk),
            base_ptr: self.value_stack.len(),
            bindings_base: self.bindings_stack.len() - 1,
        };
        self.call_stack.push(call_frame);

        // Switch to rule body
        self.chunk = Arc::clone(body);
        self.ip = 0;

        Ok(())
    }

    /// Execute a rule body with tail call optimization (reuse current frame)
    pub(super) fn execute_rule_body_tail(
        &mut self,
        body: &Arc<BytecodeChunk>,
        bindings: &Bindings,
    ) -> VmResult<()> {
        // For TCO: don't push a new call frame, just replace the current chunk
        // and reset bindings to the current frame level

        // Clear current binding frame and repopulate with new bindings
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.clear();
            for (name, value) in bindings.iter() {
                frame.set(name.clone(), value.clone());
            }
        }

        // Switch to rule body (no call frame push = TCO)
        self.chunk = Arc::clone(body);
        self.ip = 0;

        Ok(())
    }

    /// Call with N arguments where the head is on the stack (not constant pool).
    ///
    /// Pops the head and N arguments from the stack, builds a call expression,
    /// and dispatches to MORK for rule matching.
    ///
    /// Stack: [head, arg1, arg2, ..., argN] -> [result]
    /// Bytecode: CallN arity:u8
    pub(super) fn op_call_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_n");
        let arity = self.read_u8()? as usize;

        // Pop arguments and head from stack
        // Stack order: head is pushed first, then args left-to-right
        // So we need to pop args first, then head
        if self.value_stack.len() < arity + 1 {
            return Err(VmError::StackUnderflow);
        }

        // Pop arguments (in reverse order, then reverse to get correct order)
        let mut args: Vec<MettaValue> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Pop head
        let head = self.pop()?;

        // Extract head symbol if it's an atom
        let head_symbol = match head.inner() {
            MettaValueInner::Atom(s) => s.clone(),
            _ => {
                // Head is not an atom - return expression as data
                let mut items = Vec::with_capacity(arity + 1);
                items.push(head);
                items.extend(args);
                self.push(MettaValue::SExpr(items));
                return Ok(());
            }
        };

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(MettaValue::Atom(head_symbol.clone()));
        items.extend(args);
        let expr = MettaValue::SExpr(items);

        // Dispatch via MORK bridge if available
        if let Some(ref bridge) = self.bridge {
            let matches = bridge.dispatch_rules(&expr);

            if matches.is_empty() {
                // No rules match - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }

            if matches.len() == 1 {
                // Single match - execute directly
                let rule = &matches[0];
                return self.execute_rule_body(&rule.body, &rule.bindings);
            }

            // Multiple matches - create choice point for backtracking
            let alternatives: Vec<Alternative> = matches[1..]
                .iter()
                .map(|rule| Alternative::RuleMatch {
                    chunk: Arc::clone(&rule.body),
                    bindings: rule.bindings.clone(),
                })
                .collect();

            let choice_point = ChoicePoint {
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                alternatives,
            };
            self.choice_points.push(choice_point);

            // Execute first matching rule
            let rule = &matches[0];
            return self.execute_rule_body(&rule.body, &rule.bindings);
        }

        // No bridge - return expression as data (irreducible)
        self.push(expr);
        Ok(())
    }

    /// Tail call with N arguments where the head is on the stack.
    ///
    /// Same as CallN but uses tail call optimization (reuses current call frame).
    ///
    /// Stack: [head, arg1, arg2, ..., argN] -> [result]
    /// Bytecode: TailCallN arity:u8
    pub(super) fn op_tail_call_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "tail_call_n");
        let arity = self.read_u8()? as usize;

        // Pop arguments and head from stack
        if self.value_stack.len() < arity + 1 {
            return Err(VmError::StackUnderflow);
        }

        // Pop arguments
        let mut args: Vec<MettaValue> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Pop head
        let head = self.pop()?;

        // Extract head symbol if it's an atom
        let head_symbol = match head.inner() {
            MettaValueInner::Atom(s) => s.clone(),
            _ => {
                // Head is not an atom - return expression as data
                let mut items = Vec::with_capacity(arity + 1);
                items.push(head);
                items.extend(args);
                self.push(MettaValue::SExpr(items));
                return Ok(());
            }
        };

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(MettaValue::Atom(head_symbol.clone()));
        items.extend(args);
        let expr = MettaValue::SExpr(items);

        // Dispatch via MORK bridge if available
        if let Some(ref bridge) = self.bridge {
            let matches = bridge.dispatch_rules(&expr);

            if matches.is_empty() {
                // No rules match - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }

            // For tail call, execute first match with TCO (no choice points for now)
            // Full nondeterminism support can be added later
            let rule = &matches[0];
            return self.execute_rule_body_tail(&rule.body, &rule.bindings);
        }

        // No bridge - return expression as data (irreducible)
        self.push(expr);
        Ok(())
    }

    // === Return Operations ===

    pub(super) fn op_return(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "return");
        let value = self.pop()?;
        if let Some(frame) = self.call_stack.pop() {
            // Return to caller - restore chunk/ip
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);

            // Pop binding frames down to caller's level
            while self.bindings_stack.len() > frame.bindings_base + 1 {
                self.bindings_stack.pop();
            }

            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // Return from top-level
            self.results.push(value);
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    pub(super) fn op_return_multi(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        // Return all values on stack above base_ptr
        let base = self.call_stack.last().map(|f| f.base_ptr).unwrap_or(0);
        let values: Vec<MettaValue> = self.value_stack.drain(base..).collect();

        if let Some(frame) = self.call_stack.pop() {
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);

            // Pop binding frames down to caller's level
            while self.bindings_stack.len() > frame.bindings_base + 1 {
                self.bindings_stack.pop();
            }

            for v in values {
                self.push(v);
            }
            Ok(ControlFlow::Continue(()))
        } else {
            self.results.extend(values);
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }
}
