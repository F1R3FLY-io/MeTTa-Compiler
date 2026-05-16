//! Cranelift IR Generation Helpers
//!
//! This module provides helper functions for generating Cranelift IR,
//! abstracting common patterns like NaN-boxing, type guards, and stack operations.

use cranelift::codegen::ir::FuncRef;
use cranelift::prelude::*;

#[cfg(test)]
use super::types::TAG_PTR;
use super::types::{JitError, JitResult, PAYLOAD_MASK, TAG_BOOL, TAG_LONG, TAG_MASK, TAG_UNIT};

/// Pre-declared function references for error handlers.
///
/// These are created once at the start of function compilation and used
/// by bailout code to call runtime error handlers. They are required for
/// every `CodegenContext`: there is no legacy `trap()` fallback path.
#[derive(Clone, Copy)]
pub struct ErrorFuncRefs {
    /// Type error handler: fn(ctx, ip, expected) -> ()
    pub type_error: FuncRef,
    /// Division by zero handler: fn(ctx, ip) -> ()
    pub div_by_zero: FuncRef,
    // Note: arithmetic overflow handler removed. Per MeTTa spec §13.2 + §C.7g,
    // integer arithmetic must wrap silently rather than emit Error atoms.
    // The dead `guard_not_i64_min` / `emit_overflow_bailout` machinery has been
    // removed alongside this field's former `overflow` member.
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

    /// Pre-declared error handler function references used by bailout
    /// code instead of `trap()`. Required (no `Option`) so type/div-zero
    /// guards always have a graceful handler — the legacy unguarded
    /// `trap()` path that previously produced SIGILL via `ud2` has been
    /// removed entirely.
    error_func_refs: ErrorFuncRefs,
}

impl<'a, 'b> CodegenContext<'a, 'b> {
    /// Create a new codegen context.
    ///
    /// `error_func_refs` is required — bailout code calls those runtime
    /// handlers instead of emitting raw `trap()` (which compiles to
    /// `ud2` on x86_64 and would raise SIGILL at runtime). The previous
    /// `new(builder, ctx_ptr)` overload that defaulted to `None` and
    /// fell back to `trap()` has been deleted; making the handlers
    /// mandatory at the type level prevents that footgun from being
    /// reintroduced.
    pub fn new(
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
            error_func_refs,
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

    /// Debug-only invariant check: panic if the current block is already
    /// terminated. Emitting IR (return_, brif, call, etc.) on a terminated
    /// block produces invalid Cranelift IR which the backend may compile
    /// to `ud2` (SIGILL at runtime). Past regressions in this class are
    /// documented in commits `c339e5d`, `28053e4`, `e782fd9`. Cheap in
    /// release (`debug_assert!` compiles to nothing).
    #[inline]
    fn debug_assert_not_terminated(&self, op: &'static str) {
        debug_assert!(
            !self.terminated,
            "CodegenContext::{op}: emitting IR on a terminated block — this would produce invalid IR (likely compiled to `ud2` → SIGILL). \
             A previous instruction terminated this block; call `clear_terminated()` after `switch_to_block` to a fresh block before emitting more code."
        );
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
        self.debug_assert_not_terminated("guard_long");
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
        self.debug_assert_not_terminated("guard_bool");
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
        self.debug_assert_not_terminated("guard_nonzero");
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

    // =========================================================================
    // Bailout Emission
    // =========================================================================

    /// Emit code for type error bailout.
    ///
    /// Calls the runtime type-error handler (which sets `ctx.bailout = true`
    /// + `ctx.bailout_reason`) and returns from the JIT function. The VM
    /// then resumes from the bytecode interpreter at `ctx.bailout_ip`.
    /// There is no `trap()` fallback — the previous `Option<ErrorFuncRefs>`
    /// machinery that emitted `ud2` when the handlers were absent has
    /// been removed (see `CodegenContext::new` doc).
    fn emit_type_error_bailout(&mut self, ip: usize, _expected: &'static str) {
        self.debug_assert_not_terminated("emit_type_error_bailout");
        // Call jit_runtime_type_error(ctx, ip, expected)
        let ctx = self.ctx_ptr;
        let ip_val = self.builder.ins().iconst(types::I64, ip as i64);
        let expected_val = self.builder.ins().iconst(types::I64, 0); // placeholder for expected type

        self.builder
            .ins()
            .call(self.error_func_refs.type_error, &[ctx, ip_val, expected_val]);

        // Return from function - VM will check bailout flag
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.ins().return_(&[zero]);
        self.terminated = true;
    }

    /// Emit code for division by zero bailout.
    ///
    /// Calls the runtime div-by-zero handler (which sets `ctx.bailout = true`
    /// + `ctx.bailout_reason = DivisionByZero`) and returns. See
    /// `emit_type_error_bailout` for the protocol; the same removal of
    /// the legacy `trap()` fallback applies here.
    fn emit_div_zero_bailout(&mut self, ip: usize) {
        self.debug_assert_not_terminated("emit_div_zero_bailout");
        // Call jit_runtime_div_by_zero(ctx, ip)
        let ctx = self.ctx_ptr;
        let ip_val = self.builder.ins().iconst(types::I64, ip as i64);

        self.builder
            .ins()
            .call(self.error_func_refs.div_by_zero, &[ctx, ip_val]);

        // Return from function - VM will check bailout flag
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.ins().return_(&[zero]);
        self.terminated = true;
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
