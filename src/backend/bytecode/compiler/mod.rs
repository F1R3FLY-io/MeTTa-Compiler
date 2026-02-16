//! Bytecode compiler for MeTTa expressions
//!
//! This module compiles MettaValue expressions to bytecode chunks.
//! The compiler handles:
//! - Literals (numbers, booleans, strings, etc.)
//! - Symbols and variables
//! - S-expressions (recursive compilation)
//! - Grounded operations (+, -, *, /, comparisons, etc.)
//! - Special forms (if, let, quote, etc.)

mod context;
mod control_flow;
mod error;
pub mod folding;
pub mod generic;
mod higher_order;
mod iterative;
mod work_item;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::chunk::{BytecodeChunk, ChunkBuilder};
use super::opcodes::Opcode;
use crate::backend::models::MettaValue;

pub use context::{CompileContext, Upvalue};
pub use error::{CompileError, CompileResult};

/// Bytecode compiler
pub struct Compiler {
    /// The chunk being built
    pub(crate) builder: ChunkBuilder,
    /// Compilation context
    pub(crate) context: CompileContext,
    /// Current source line
    current_line: u32,
    /// Whether we're compiling in tail position (for TCO)
    pub(crate) in_tail_position: bool,
}

impl Compiler {
    /// Create a new compiler with optimization enabled
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            builder: ChunkBuilder::new_optimized(name),
            context: CompileContext::new(),
            current_line: 1,
            in_tail_position: true, // Top-level is always tail position
        }
    }

    /// Create a compiler with existing context (for nested functions)
    pub fn with_context(name: impl Into<String>, context: CompileContext) -> Self {
        Self {
            builder: ChunkBuilder::new_optimized(name),
            context,
            current_line: 1,
            in_tail_position: true, // Top-level is always tail position
        }
    }

    /// Set the current source line
    pub fn set_line(&mut self, line: u32) {
        self.current_line = line;
        self.builder.set_line(line);
    }

    /// Compile a MettaValue expression
    ///
    /// This method uses an iterative trampoline pattern internally to prevent
    /// stack overflow for deeply nested expressions.
    pub fn compile(&mut self, expr: &MettaValue) -> CompileResult<()> {
        // Use the iterative compiler to prevent stack overflow
        self.compile_iterative(expr)
    }

    /// Compile a long integer
    fn compile_long(&mut self, n: i64) -> CompileResult<()> {
        if n >= -128 && n <= 127 {
            self.builder.emit_byte(Opcode::PushLongSmall, n as u8);
        } else {
            let idx = self.builder.add_constant(MettaValue::Long(n));
            self.builder.emit_u16(Opcode::PushLong, idx);
        }
        Ok(())
    }

    /// Compile a float
    fn compile_float(&mut self, f: f64) -> CompileResult<()> {
        let idx = self.builder.add_constant(MettaValue::Float(f));
        self.builder.emit_u16(Opcode::PushConstant, idx);
        Ok(())
    }

    /// Compile an atom (symbol or variable)
    fn compile_atom(&mut self, name: &str) -> CompileResult<()> {
        // Check if it's a variable (starts with $)
        if let Some(var_name) = name.strip_prefix('$') {
            // First try to resolve as local
            if let Some(slot) = self.context.resolve_local(var_name) {
                if slot <= 255 {
                    self.builder.emit_byte(Opcode::LoadLocal, slot as u8);
                } else {
                    self.builder.emit_u16(Opcode::LoadLocalWide, slot);
                }
                return Ok(());
            }

            // Try to resolve as upvalue
            if let Some(idx) = self.context.resolve_upvalue(var_name) {
                self.builder.emit_u16(Opcode::LoadUpvalue, idx);
                return Ok(());
            }

            // Variable not bound - push as symbol to be resolved at runtime
            let idx = self
                .builder
                .add_constant(MettaValue::Atom(name.to_string()));
            self.builder.emit_u16(Opcode::PushVariable, idx);
        } else {
            // Regular symbol
            let idx = self
                .builder
                .add_constant(MettaValue::Atom(name.to_string()));
            self.builder.emit_u16(Opcode::PushAtom, idx);
        }
        Ok(())
    }


    /// Check arity of an operation
    pub(crate) fn check_arity(&self, op: &str, got: usize, expected: usize) -> CompileResult<()> {
        if got != expected {
            Err(CompileError::InvalidArity {
                op: op.to_string(),
                expected,
                got,
            })
        } else {
            Ok(())
        }
    }

    /// Check arity range of an operation
    pub(crate) fn check_arity_range(
        &self,
        op: &str,
        got: usize,
        min: usize,
        max: usize,
    ) -> CompileResult<()> {
        if got < min || got > max {
            Err(CompileError::InvalidArityRange {
                op: op.to_string(),
                min,
                max,
                got,
            })
        } else {
            Ok(())
        }
    }

    // =========================================================================
    // Constant Folding Wrappers
    // =========================================================================

    /// Try to evaluate an expression to a constant at compile time
    fn try_eval_constant(&self, expr: &MettaValue) -> Option<MettaValue> {
        folding::try_eval_constant(expr)
    }

    /// Try to fold a binary arithmetic operation at compile time
    fn try_fold_binary_arith(
        &self,
        op: &str,
        a: &MettaValue,
        b: &MettaValue,
    ) -> Option<MettaValue> {
        folding::try_fold_binary_arith(op, a, b)
    }

    /// Try to fold a unary arithmetic operation at compile time
    fn try_fold_unary_arith(&self, op: &str, a: &MettaValue) -> Option<MettaValue> {
        folding::try_fold_unary_arith(op, a)
    }

    /// Try to fold a comparison operation at compile time
    fn try_fold_comparison(&self, op: &str, a: &MettaValue, b: &MettaValue) -> Option<MettaValue> {
        folding::try_fold_comparison(op, a, b)
    }

    /// Try to fold a boolean operation at compile time
    fn try_fold_boolean(&self, op: &str, args: &[MettaValue]) -> Option<MettaValue> {
        folding::try_fold_boolean(op, args)
    }

    // =========================================================================
    // Finishing Methods
    // =========================================================================

    /// Finish compilation and return the chunk
    pub fn finish(mut self) -> BytecodeChunk {
        // Add return if not already present
        // We check if the chunk is empty or doesn't end with a terminator
        let offset = self.builder.current_offset();
        let needs_return = offset == 0 || !self.ends_with_terminator();

        if needs_return {
            self.builder.emit(Opcode::Return);
        }

        self.builder.set_local_count(self.context.local_count());
        self.builder.set_upvalue_count(self.context.upvalue_count());

        self.builder.build()
    }

    /// Check if the last emitted instruction is a terminator
    fn ends_with_terminator(&self) -> bool {
        // Build a temporary view to check the last opcode
        // Since we can't peek at the builder's code directly, we'll track this differently
        // For now, just return false to always add a return (safe default)
        false
    }

    /// Finish and wrap in Arc
    pub fn finish_arc(self) -> Arc<BytecodeChunk> {
        Arc::new(self.finish())
    }
}

/// Compile a MettaValue to bytecode
pub fn compile(name: &str, expr: &MettaValue) -> CompileResult<BytecodeChunk> {
    let mut compiler = Compiler::new(name);
    compiler.compile(expr)?;
    Ok(compiler.finish())
}

/// Compile a MettaValue to bytecode wrapped in Arc
pub fn compile_arc(name: &str, expr: &MettaValue) -> CompileResult<Arc<BytecodeChunk>> {
    Ok(Arc::new(compile(name, expr)?))
}
