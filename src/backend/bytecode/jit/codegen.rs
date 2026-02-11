//! Cranelift IR Generation Helpers
//!
//! This module provides helper functions for generating Cranelift IR,
//! abstracting common patterns like NaN-boxing, type guards, and stack operations.

use cranelift::codegen::ir::FuncRef;
use cranelift::prelude::*;

use super::types::{
    JitError, JitResult, PAYLOAD_MASK, TAG_BOOL, TAG_LONG, TAG_MASK, TAG_UNIT,
};
#[cfg(test)]
use super::types::TAG_PTR;

/// Pre-declared function references for error handlers.
///
/// These are created once at the start of function compilation and used
/// by bailout code to call runtime error handlers instead of using trap().
/// This prevents SIGILL crashes from ud2 instructions.
#[derive(Clone, Copy)]
pub struct ErrorFuncRefs {
    /// Type error handler: fn(ctx, ip, expected) -> ()
    pub type_error: FuncRef,
    /// Division by zero handler: fn(ctx, ip) -> ()
    pub div_by_zero: FuncRef,
    /// Arithmetic overflow handler: fn(ctx, ip) -> ()
    pub overflow: FuncRef,
}

/// Code generation context wrapping a Cranelift FunctionBuilder
///
/// Provides high-level operations for:
/// - Stack manipulation (push/pop/peek)
/// - NaN-boxing (box/unbox values)
/// - Type guards (emit bailout on type mismatch)
/// - Runtime function calls

pub struct CodegenContext<'a, 'b> {
    pub builder: &'a mut FunctionBuilder<'b>,

    /// Pointer to JitContext
    ctx_ptr: Value,

    /// Simulated stack for values (we track SSA values, not memory)
    /// This allows us to keep values in registers when possible
    value_stack: Vec<Value>,

    /// Flag indicating if current block is terminated
    terminated: bool,

    /// Local variables (for Stage 4 local variable support)
    locals: Vec<Option<Value>>,

    /// Pre-declared error handler function references.
    /// When set, bailout code calls these instead of using trap().
    error_func_refs: Option<ErrorFuncRefs>,
}

impl<'a, 'b> CodegenContext<'a, 'b> {
    /// Create a new codegen context
    pub fn new(builder: &'a mut FunctionBuilder<'b>, ctx_ptr: Value) -> Self {
        CodegenContext {
            builder,
            ctx_ptr,
            value_stack: Vec::with_capacity(32),
            terminated: false,
            locals: Vec::new(),
            error_func_refs: None,
        }
    }

    /// Create a new codegen context with error handler support
    ///
    /// When error FuncRefs are provided, bailout code will call the runtime
    /// error handlers and return instead of using trap() instructions.
    /// This prevents SIGILL crashes from ud2 instructions.
    pub fn with_error_handlers(
        builder: &'a mut FunctionBuilder<'b>,
        ctx_ptr: Value,
        error_func_refs: ErrorFuncRefs,
    ) -> Self {
        CodegenContext {
            builder,
            ctx_ptr,
            value_stack: Vec::with_capacity(32),
            terminated: false,
            locals: Vec::new(),
            error_func_refs: Some(error_func_refs),
        }
    }

    // =========================================================================
    // Stack Operations
    // =========================================================================

    /// Push a value onto the simulated stack
    pub fn push(&mut self, val: Value) -> JitResult<()> {
        self.value_stack.push(val);
        Ok(())
    }

    /// Pop a value from the simulated stack
    pub fn pop(&mut self) -> JitResult<Value> {
        self.value_stack.pop().ok_or(JitError::StackUnderflow)
    }

    /// Peek at the top of the stack without removing
    pub fn peek(&self) -> JitResult<Value> {
        self.value_stack
            .last()
            .copied()
            .ok_or(JitError::StackUnderflow)
    }

    /// Get current stack depth
    pub fn stack_depth(&self) -> usize {
        self.value_stack.len()
    }

    /// Check if current block is terminated
    pub fn is_terminated(&self) -> bool {
        self.terminated
    }

    /// Mark current block as terminated
    pub fn mark_terminated(&mut self) {
        self.terminated = true;
    }

    /// Clear the terminated flag (when switching to a new block)
    pub fn clear_terminated(&mut self) {
        self.terminated = false;
    }

    /// Clear the simulated stack (for merge blocks)
    pub fn clear_stack(&mut self) {
        self.value_stack.clear();
    }

    /// Get the context pointer for runtime calls
    pub fn ctx_ptr(&self) -> Value {
        self.ctx_ptr
    }

    // =========================================================================
    // Local Variable Operations
    // =========================================================================

    /// Initialize local variable storage for n locals
    pub fn init_locals(&mut self, count: usize) {
        self.locals = vec![None; count];
    }

    /// Load a local variable onto the stack
    pub fn load_local(&mut self, index: usize) -> JitResult<()> {
        if index >= self.locals.len() {
            return Err(JitError::InvalidLocalIndex(index));
        }
        match self.locals[index] {
            Some(val) => {
                self.value_stack.push(val);
                Ok(())
            }
            None => {
                // Uninitialized local - push unit
                let unit = self.const_unit();
                self.value_stack.push(unit);
                Ok(())
            }
        }
    }

    /// Store the top of stack into a local variable
    ///
    /// Note: This uses peek() instead of pop() to match VM behavior.
    /// The VM's StoreLocal keeps the value on the stack (at stack[base+index]),
    /// so the JIT must keep the value on its simulated stack too. This ensures
    /// scope cleanup patterns (Swap; Pop after let bodies) work correctly.
    pub fn store_local(&mut self, index: usize) -> JitResult<()> {
        if index >= self.locals.len() {
            return Err(JitError::InvalidLocalIndex(index));
        }
        let val = self.peek()?; // Keep on stack to match VM behavior
        self.locals[index] = Some(val);
        Ok(())
    }

    // =========================================================================
    // Constant Creation
    // =========================================================================

    /// Create a NaN-boxed unit constant
    pub fn const_unit(&mut self) -> Value {
        self.builder.ins().iconst(types::I64, TAG_UNIT as i64)
    }

    /// Create a NaN-boxed boolean constant
    pub fn const_bool(&mut self, b: bool) -> Value {
        let bits = TAG_BOOL | (b as u64);
        self.builder.ins().iconst(types::I64, bits as i64)
    }

    /// Create a NaN-boxed long constant
    pub fn const_long(&mut self, n: i64) -> Value {
        // Truncate to 48 bits and add tag
        let payload = (n as u64) & PAYLOAD_MASK;
        let bits = TAG_LONG | payload;
        self.builder.ins().iconst(types::I64, bits as i64)
    }

    // =========================================================================
    // NaN-Boxing: Extraction (Unboxing)
    // =========================================================================

    /// Extract the raw integer value from a Long (lower 48 bits, sign-extended)
    pub fn extract_long(&mut self, val: Value) -> Value {
        // Mask to get lower 48 bits
        let mask = self.builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
        let payload = self.builder.ins().band(val, mask);

        // Sign extend from 48 bits
        // Shift left 16, then arithmetic shift right 16
        let shifted = self.builder.ins().ishl_imm(payload, 16);
        self.builder.ins().sshr_imm(shifted, 16)
    }

    /// Extract the boolean value (just bit 0)
    pub fn extract_bool(&mut self, val: Value) -> Value {
        let one = self.builder.ins().iconst(types::I64, 1);
        self.builder.ins().band(val, one)
    }

    /// Get the tag from a NaN-boxed value
    pub fn extract_tag(&mut self, val: Value) -> Value {
        let mask = self.builder.ins().iconst(types::I64, TAG_MASK as i64);
        self.builder.ins().band(val, mask)
    }

    // =========================================================================
    // NaN-Boxing: Boxing
    // =========================================================================

    /// Box an integer value as Long (assumes value fits in 48 bits)
    pub fn box_long(&mut self, val: Value) -> Value {
        let mask = self.builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
        let payload = self.builder.ins().band(val, mask);
        let tag = self.builder.ins().iconst(types::I64, TAG_LONG as i64);
        self.builder.ins().bor(payload, tag)
    }

    /// Box a boolean value (0 or 1)
    pub fn box_bool(&mut self, val: Value) -> Value {
        let tag = self.builder.ins().iconst(types::I64, TAG_BOOL as i64);
        self.builder.ins().bor(val, tag)
    }

    // =========================================================================
    // Type Guards
    // =========================================================================

    /// Emit a guard that checks if value is a Long, bailout if not
    pub fn guard_long(&mut self, val: Value, ip: usize) -> JitResult<()> {
        let tag = self.extract_tag(val);
        let expected = self.builder.ins().iconst(types::I64, TAG_LONG as i64);
        let is_long = self.builder.ins().icmp(IntCC::Equal, tag, expected);

        // Create bailout block
        let continue_block = self.builder.create_block();
        let bailout_block = self.builder.create_block();

        self.builder
            .ins()
            .brif(is_long, continue_block, &[], bailout_block, &[]);

        // Bailout block: call runtime error handler
        self.builder.switch_to_block(bailout_block);
        self.builder.seal_block(bailout_block);
        self.emit_type_error_bailout(ip, "Long");
        // Note: bailout block terminates with trap/return

        // Continue block
        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);
        self.clear_terminated(); // Reset flag for new unterminated block

        Ok(())
    }

    /// Emit a guard that checks if value is a Bool, bailout if not
    pub fn guard_bool(&mut self, val: Value, ip: usize) -> JitResult<()> {
        let tag = self.extract_tag(val);
        let expected = self.builder.ins().iconst(types::I64, TAG_BOOL as i64);
        let is_bool = self.builder.ins().icmp(IntCC::Equal, tag, expected);

        let continue_block = self.builder.create_block();
        let bailout_block = self.builder.create_block();

        self.builder
            .ins()
            .brif(is_bool, continue_block, &[], bailout_block, &[]);

        self.builder.switch_to_block(bailout_block);
        self.builder.seal_block(bailout_block);
        self.emit_type_error_bailout(ip, "Bool");

        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);
        self.clear_terminated(); // Reset flag for new unterminated block

        Ok(())
    }

    /// Emit a guard that checks if value is non-zero (for division)
    pub fn guard_nonzero(&mut self, val: Value, ip: usize) -> JitResult<()> {
        let zero = self.builder.ins().iconst(types::I64, 0);
        let is_nonzero = self.builder.ins().icmp(IntCC::NotEqual, val, zero);

        let continue_block = self.builder.create_block();
        let bailout_block = self.builder.create_block();

        self.builder
            .ins()
            .brif(is_nonzero, continue_block, &[], bailout_block, &[]);

        self.builder.switch_to_block(bailout_block);
        self.builder.seal_block(bailout_block);
        self.emit_div_zero_bailout(ip);

        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);
        self.clear_terminated(); // Reset flag for new unterminated block

        Ok(())
    }

    /// Emit a guard that checks if value is not i64::MIN (for abs overflow)
    pub fn guard_not_i64_min(&mut self, val: Value, ip: usize) -> JitResult<()> {
        let i64_min = self.builder.ins().iconst(types::I64, i64::MIN);
        let is_not_min = self.builder.ins().icmp(IntCC::NotEqual, val, i64_min);

        let continue_block = self.builder.create_block();
        let bailout_block = self.builder.create_block();

        self.builder
            .ins()
            .brif(is_not_min, continue_block, &[], bailout_block, &[]);

        self.builder.switch_to_block(bailout_block);
        self.builder.seal_block(bailout_block);
        self.emit_overflow_bailout(ip);

        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);
        self.clear_terminated(); // Reset flag for new unterminated block

        Ok(())
    }

    // =========================================================================
    // Bailout Emission
    // =========================================================================

    /// Emit code for type error bailout
    ///
    /// If error FuncRefs are available, calls the runtime error handler and returns.
    /// Otherwise falls back to trap() which generates ud2 (may cause SIGILL).
    fn emit_type_error_bailout(&mut self, ip: usize, _expected: &'static str) {
        if let Some(error_refs) = self.error_func_refs {
            // Call jit_runtime_type_error(ctx, ip, expected)
            let ctx = self.ctx_ptr;
            let ip_val = self.builder.ins().iconst(types::I64, ip as i64);
            let expected_val = self.builder.ins().iconst(types::I64, 0); // placeholder for expected type

            self.builder
                .ins()
                .call(error_refs.type_error, &[ctx, ip_val, expected_val]);

            // Return from function - VM will check bailout flag
            let zero = self.builder.ins().iconst(types::I64, 0);
            self.builder.ins().return_(&[zero]);
        } else {
            // Fallback: use trap (may cause SIGILL but backwards compatible)
            self.builder.ins().trap(TrapCode::unwrap_user(1));
        }
        self.terminated = true;
    }

    /// Emit code for division by zero bailout
    ///
    /// If error FuncRefs are available, calls the runtime error handler and returns.
    /// Otherwise falls back to trap() which generates ud2 (may cause SIGILL).
    fn emit_div_zero_bailout(&mut self, ip: usize) {
        if let Some(error_refs) = self.error_func_refs {
            // Call jit_runtime_div_by_zero(ctx, ip)
            let ctx = self.ctx_ptr;
            let ip_val = self.builder.ins().iconst(types::I64, ip as i64);

            self.builder
                .ins()
                .call(error_refs.div_by_zero, &[ctx, ip_val]);

            // Return from function - VM will check bailout flag
            let zero = self.builder.ins().iconst(types::I64, 0);
            self.builder.ins().return_(&[zero]);
        } else {
            // Fallback: use trap (may cause SIGILL but backwards compatible)
            self.builder.ins().trap(TrapCode::unwrap_user(2));
        }
        self.terminated = true;
    }

    /// Emit code for arithmetic overflow bailout
    ///
    /// If error FuncRefs are available, calls the runtime error handler and returns.
    /// Otherwise falls back to trap() which generates ud2 (may cause SIGILL).
    fn emit_overflow_bailout(&mut self, ip: usize) {
        if let Some(error_refs) = self.error_func_refs {
            // Call jit_runtime_stack_overflow(ctx, ip) - reused for arithmetic overflow
            let ctx = self.ctx_ptr;
            let ip_val = self.builder.ins().iconst(types::I64, ip as i64);

            self.builder.ins().call(error_refs.overflow, &[ctx, ip_val]);

            // Return from function - VM will check bailout flag
            let zero = self.builder.ins().iconst(types::I64, 0);
            self.builder.ins().return_(&[zero]);
        } else {
            // Fallback: use trap (may cause SIGILL but backwards compatible)
            self.builder.ins().trap(TrapCode::unwrap_user(3));
        }
        self.terminated = true;
    }

    // =========================================================================
    // Runtime Calls
    // =========================================================================

    /// Placeholder for runtime power function - DEFERRED TO STAGE 2
    ///
    /// Pow operations require runtime calls which need:
    /// 1. External function declaration in JITModule
    /// 2. Function import into the current function context
    /// 3. Proper argument passing and return value handling
    ///
    /// For Stage 1, chunks containing Pow are not JIT-compilable and will
    /// execute on the bytecode VM instead.
    ///
    /// Stage 2 implementation should:
    /// 1. Register jit_runtime_pow in compiler::register_runtime_symbols()
    /// 2. Pass function references through CodegenContext
    /// 3. Use builder.ins().call(func_ref, &[base, exp]) to call the runtime
    #[allow(dead_code)]
    pub fn call_runtime_pow(&mut self, _base: Value, _exp: Value) -> Value {
        // Stage 2: This will be replaced with a proper runtime call
        // For now, return 1 as a placeholder (unreachable in Stage 1)
        self.const_long(1)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nan_boxing_constants() {
        // Verify constant bit patterns
        assert_eq!(TAG_LONG, 0x7FF8_0000_0000_0000);
        assert_eq!(TAG_BOOL, 0x7FF9_0000_0000_0000);
        assert_eq!(TAG_UNIT, 0x7FFB_0000_0000_0000);
        assert_eq!(TAG_PTR, 0x7FFC_0000_0000_0000);
    }

    #[test]
    fn test_payload_mask() {
        assert_eq!(PAYLOAD_MASK, 0x0000_FFFF_FFFF_FFFF);

        // Check that 48 bits is enough for common values
        let max_48: i64 = (1 << 47) - 1;
        let min_48: i64 = -(1 << 47);
        assert!(max_48 > 0);
        assert!(min_48 < 0);

        // Check masking works
        let masked = (42i64 as u64) & PAYLOAD_MASK;
        assert_eq!(masked, 42);
    }
}
