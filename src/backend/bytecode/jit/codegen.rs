//! Cranelift IR Generation Helpers
//!
//! This module provides helper functions for generating Cranelift IR,
//! abstracting common patterns like NaN-boxing, type guards, and stack operations.

#[cfg(feature = "jit")]
use cranelift::prelude::*;

use super::types::{
    JitError, JitResult, TAG_BOOL, TAG_HEAP, TAG_LONG, TAG_MASK, TAG_NIL, TAG_UNIT, PAYLOAD_MASK,
};

/// Code generation context wrapping a Cranelift FunctionBuilder
///
/// Provides high-level operations for:
/// - Stack manipulation (push/pop/peek)
/// - NaN-boxing (box/unbox values)
/// - Type guards (emit bailout on type mismatch)
/// - Runtime function calls
#[cfg(feature = "jit")]
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
}

#[cfg(feature = "jit")]
impl<'a, 'b> CodegenContext<'a, 'b> {
    /// Create a new codegen context
    pub fn new(builder: &'a mut FunctionBuilder<'b>, ctx_ptr: Value) -> Self {
        CodegenContext {
            builder,
            ctx_ptr,
            value_stack: Vec::with_capacity(32),
            terminated: false,
            locals: Vec::new(),
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
        self.value_stack
            .pop()
            .ok_or(JitError::StackUnderflow)
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
                // Uninitialized local - push nil
                let nil = self.const_nil();
                self.value_stack.push(nil);
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
        let val = self.peek()?;  // Keep on stack to match VM behavior
        self.locals[index] = Some(val);
        Ok(())
    }

    // =========================================================================
    // Constant Creation
    // =========================================================================

    /// Create a NaN-boxed nil constant
    pub fn const_nil(&mut self) -> Value {
        self.builder.ins().iconst(types::I64, TAG_NIL as i64)
    }

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

        self.builder.ins().brif(is_long, continue_block, &[], bailout_block, &[]);

        // Bailout block: call runtime error handler
        self.builder.switch_to_block(bailout_block);
        self.builder.seal_block(bailout_block);
        self.emit_type_error_bailout(ip, "Long");
        // Note: bailout block terminates with trap/return

        // Continue block
        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);

        Ok(())
    }

    /// Emit a guard that checks if value is a Bool, bailout if not
    pub fn guard_bool(&mut self, val: Value, ip: usize) -> JitResult<()> {
        let tag = self.extract_tag(val);
        let expected = self.builder.ins().iconst(types::I64, TAG_BOOL as i64);
        let is_bool = self.builder.ins().icmp(IntCC::Equal, tag, expected);

        let continue_block = self.builder.create_block();
        let bailout_block = self.builder.create_block();

        self.builder.ins().brif(is_bool, continue_block, &[], bailout_block, &[]);

        self.builder.switch_to_block(bailout_block);
        self.builder.seal_block(bailout_block);
        self.emit_type_error_bailout(ip, "Bool");

        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);

        Ok(())
    }

    /// Emit a guard that checks if value is non-zero (for division)
    pub fn guard_nonzero(&mut self, val: Value, ip: usize) -> JitResult<()> {
        let zero = self.builder.ins().iconst(types::I64, 0);
        let is_nonzero = self.builder.ins().icmp(IntCC::NotEqual, val, zero);

        let continue_block = self.builder.create_block();
        let bailout_block = self.builder.create_block();

        self.builder.ins().brif(is_nonzero, continue_block, &[], bailout_block, &[]);

        self.builder.switch_to_block(bailout_block);
        self.builder.seal_block(bailout_block);
        self.emit_div_zero_bailout(ip);

        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);

        Ok(())
    }

    // =========================================================================
    // Bailout Emission
    // =========================================================================

    /// Emit code for type error bailout
    fn emit_type_error_bailout(&mut self, _ip: usize, _expected: &'static str) {
        // For now, just trap - in full implementation would call runtime
        // Use user trap code 1 for type errors (0 is reserved/invalid)
        self.builder.ins().trap(TrapCode::unwrap_user(1));
        self.terminated = true;
    }

    /// Emit code for division by zero bailout
    fn emit_div_zero_bailout(&mut self, _ip: usize) {
        // Use user trap code 2 for division by zero
        self.builder.ins().trap(TrapCode::unwrap_user(2));
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

/// Stub implementation when JIT is not enabled
#[cfg(not(feature = "jit"))]
pub struct CodegenContext<'a> {
    _phantom: std::marker::PhantomData<&'a ()>,
}

#[cfg(not(feature = "jit"))]
impl<'a> CodegenContext<'a> {
    pub fn new(_builder: &'a mut (), _ctx_ptr: ()) -> Self {
        CodegenContext {
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn push(&mut self, _val: ()) -> JitResult<()> {
        Err(JitError::NotCompilable("JIT not enabled".to_string()))
    }

    pub fn pop(&mut self) -> JitResult<()> {
        Err(JitError::NotCompilable("JIT not enabled".to_string()))
    }

    pub fn peek(&self) -> JitResult<()> {
        Err(JitError::NotCompilable("JIT not enabled".to_string()))
    }

    pub fn stack_depth(&self) -> usize {
        0
    }

    pub fn is_terminated(&self) -> bool {
        false
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
        assert_eq!(TAG_NIL, 0x7FFA_0000_0000_0000);
        assert_eq!(TAG_UNIT, 0x7FFB_0000_0000_0000);
        assert_eq!(TAG_HEAP, 0x7FFC_0000_0000_0000);
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
