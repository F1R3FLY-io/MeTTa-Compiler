//! Control flow compilation for the bytecode compiler.
//!
//! This module implements compilation of control flow constructs:
//! - if: Conditional branching
//! - let/let*: Variable binding forms
//! - superpose: Non-deterministic choice
//! - collapse: Collect non-deterministic results

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::MettaValue;

use super::error::{CompileError, CompileResult};
use super::Compiler;

impl Compiler {
    /// Compile superpose: (superpose (alt1 alt2 ...))
    ///
    /// Creates a Fork choice point with all alternatives. Each alternative
    /// will be explored via backtracking when Fail is executed.
    pub(crate) fn compile_superpose(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("superpose", args.len(), 1)?;

        // The argument should be a list of alternatives
        match &args[0] {
            MettaValue::SExpr(alternatives) => {
                if alternatives.is_empty() {
                    // Empty superposition - return Empty
                    self.builder.emit(Opcode::PushEmpty);
                    return Ok(());
                }

                if alternatives.len() == 1 {
                    // Single alternative - just compile it directly
                    return self.compile(&alternatives[0]);
                }

                // Multiple alternatives - emit Fork opcode
                // Fork format: Fork count:u16 followed by count constant indices
                // Each constant is an alternative value

                // Add all alternatives to constant pool
                let mut const_indices = Vec::with_capacity(alternatives.len());
                for alt in alternatives {
                    let idx = self.builder.add_constant(alt.clone());
                    const_indices.push(idx);
                }

                // Emit Fork with count
                let count = alternatives.len() as u16;
                self.builder.emit_u16(Opcode::Fork, count);

                // Emit all constant indices (big-endian to match chunk.read_u16)
                for idx in const_indices {
                    self.builder.emit_raw(&idx.to_be_bytes());
                }

                Ok(())
            }
            // If not an S-expression, just evaluate the argument
            other => self.compile(other),
        }
    }

    /// Compile collapse: (collapse expr)
    ///
    /// Collects all non-deterministic results from evaluating expr into a list.
    /// Uses BeginNondet/Yield/Collect pattern.
    #[allow(dead_code)]
    pub(crate) fn compile_collapse(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("collapse", args.len(), 1)?;

        // Mark start of non-deterministic region
        self.builder.emit(Opcode::BeginNondet);

        // Compile the expression (not in tail position)
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(&args[0])?;
        self.in_tail_position = saved_tail;

        // Yield current result and backtrack for more
        self.builder.emit(Opcode::Yield);

        // Collect all results into S-expression
        // Collect takes chunk_index:u16 (0 = current chunk)
        self.builder.emit_u16(Opcode::Collect, 0);

        Ok(())
    }

    /// Compile if expression: (if cond then else)
    pub(crate) fn compile_if(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("if", args.len(), 3)?;

        // Condition is NOT in tail position
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(&args[0])?;
        self.in_tail_position = saved_tail;

        // Jump to else if false
        let else_jump = self.builder.emit_jump(Opcode::JumpIfFalse);

        // Then branch inherits parent's tail position
        self.compile(&args[1])?;

        // Jump over else branch
        let end_jump = self.builder.emit_jump(Opcode::Jump);

        // Patch else jump
        self.builder.patch_jump(else_jump);

        // Else branch inherits parent's tail position
        self.compile(&args[2])?;

        // Patch end jump
        self.builder.patch_jump(end_jump);

        Ok(())
    }

    /// Compile let expression: (let pattern value body)
    pub(crate) fn compile_let(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("let", args.len(), 3)?;

        // Begin new scope
        self.context.begin_scope();

        // Value is NOT in tail position
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(&args[1])?;
        self.in_tail_position = saved_tail;

        // Bind the pattern
        self.compile_pattern_binding(&args[0])?;

        // Body inherits parent's tail position
        self.compile(&args[2])?;

        // End scope
        let pop_count = self.context.end_scope();
        if pop_count > 0 {
            // Swap result under locals and pop them
            for _ in 0..pop_count {
                self.builder.emit(Opcode::Swap);
                self.builder.emit(Opcode::Pop);
            }
        }

        Ok(())
    }

    /// Compile let* expression: (let* ((var1 val1) (var2 val2) ...) body)
    pub(crate) fn compile_let_star(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("let*", args.len(), 2)?;

        // Get bindings list
        let bindings = match &args[0] {
            MettaValue::SExpr(items) => items,
            _ => {
                return Err(CompileError::InvalidExpression(
                    "let* bindings must be a list".to_string(),
                ))
            }
        };

        // Begin scope
        self.context.begin_scope();

        // Binding values are NOT in tail position
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;

        // Process each binding
        for binding in bindings {
            let (pattern, value) = match binding {
                MettaValue::SExpr(pair) if pair.len() == 2 => (&pair[0], &pair[1]),
                _ => {
                    return Err(CompileError::InvalidExpression(
                        "let* binding must be (pattern value)".to_string(),
                    ))
                }
            };

            // Compile value (not in tail position)
            self.compile(value)?;

            // Bind pattern
            self.compile_pattern_binding(pattern)?;
        }

        // Restore tail position for body
        self.in_tail_position = saved_tail;

        // Compile body (inherits parent's tail position)
        self.compile(&args[1])?;

        // End scope
        let pop_count = self.context.end_scope();
        for _ in 0..pop_count {
            self.builder.emit(Opcode::Swap);
            self.builder.emit(Opcode::Pop);
        }

        Ok(())
    }

    /// Compile a pattern binding (creates local variables)
    pub(crate) fn compile_pattern_binding(&mut self, pattern: &MettaValue) -> CompileResult<()> {
        match pattern {
            MettaValue::Atom(name) if name.starts_with('$') => {
                // Simple variable binding
                let var_name = name[1..].to_string();
                let slot = self.context.declare_local(var_name)?;
                if slot <= 255 {
                    self.builder.emit_byte(Opcode::StoreLocal, slot as u8);
                } else {
                    self.builder.emit_u16(Opcode::StoreLocalWide, slot);
                }
            }
            MettaValue::Atom(name) if name == "_" => {
                // Wildcard - just pop the value
                self.builder.emit(Opcode::Pop);
            }
            MettaValue::SExpr(items) => {
                // Destructuring pattern
                // For each element, dup the value, extract element, bind
                for (i, item) in items.iter().enumerate() {
                    self.builder.emit(Opcode::Dup);
                    self.builder.emit_byte(Opcode::GetElement, i as u8);
                    self.compile_pattern_binding(item)?;
                }
                // Pop the original value
                self.builder.emit(Opcode::Pop);
            }
            _ => {
                // Non-binding pattern - just pop
                self.builder.emit(Opcode::Pop);
            }
        }
        Ok(())
    }

    /// Compile a quoted expression (no evaluation)
    pub(crate) fn compile_quoted(&mut self, expr: &MettaValue) -> CompileResult<()> {
        match expr {
            // Atoms can be pushed directly
            MettaValue::Atom(name) => {
                let idx = self.builder.add_constant(MettaValue::Atom(name.clone()));
                if name.starts_with('$') {
                    self.builder.emit_u16(Opcode::PushVariable, idx);
                } else {
                    self.builder.emit_u16(Opcode::PushAtom, idx);
                }
            }
            // S-expressions need to be built
            MettaValue::SExpr(items) => {
                for item in items {
                    self.compile_quoted(item)?;
                }
                if items.len() <= 255 {
                    self.builder.emit_byte(Opcode::MakeSExpr, items.len() as u8);
                } else {
                    self.builder
                        .emit_u16(Opcode::MakeSExprLarge, items.len() as u16);
                }
            }
            // Other values can be compiled normally (they're already values)
            _ => self.compile(expr)?,
        }
        Ok(())
    }
}
