//! Pattern matching compilation for the bytecode compiler.
//!
//! This module implements compilation of pattern matching constructs:
//! - match: Space pattern matching
//! - unify: Pattern unification with success/failure branches
//! - case: Multi-branch pattern matching
//! - chain: Sequential evaluation with binding

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::MettaValue;

use super::error::{CompileError, CompileResult};
use super::Compiler;

impl Compiler {
    /// Compile match expression: (match space pattern template [default])
    pub(crate) fn compile_match(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity_range("match", args.len(), 3, 4)?;

        // Compile space
        self.compile(&args[0])?;

        // Compile pattern (quoted)
        self.compile_quoted(&args[1])?;

        // Compile template (quoted)
        self.compile_quoted(&args[2])?;

        // Compile default if present
        if args.len() == 4 {
            self.compile(&args[3])?;
            self.builder.emit_byte(Opcode::MakeSExpr, 4);
        } else {
            self.builder.emit_byte(Opcode::MakeSExpr, 3);
        }

        // Emit match operation
        self.builder.emit(Opcode::EvalMatch);

        Ok(())
    }

    /// Compile unify expression: (unify a b success failure)
    pub(crate) fn compile_unify(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("unify", args.len(), 4)?;

        // Compile the two expressions to unify
        self.compile(&args[0])?;
        self.compile(&args[1])?;

        // Unify them
        self.builder.emit(Opcode::UnifyBind);

        // Jump to failure if unification failed
        let failure_jump = self.builder.emit_jump(Opcode::JumpIfFalse);

        // Unification succeeded - compile success expression
        self.compile(&args[2])?;
        let done_jump = self.builder.emit_jump(Opcode::Jump);

        // Unification failed - compile failure expression
        self.builder.patch_jump(failure_jump);
        self.compile(&args[3])?;

        self.builder.patch_jump(done_jump);

        Ok(())
    }

    /// Compile case expression: (case expr ((pattern1 result1) (pattern2 result2) ...))
    pub(crate) fn compile_case(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        // Syntax: (case scrutinee ((pattern1 result1) (pattern2 result2) ...))
        self.check_arity("case", args.len(), 2)?;

        // Compile the scrutinee
        self.compile(&args[0])?;

        // args[1] contains ALL case branches wrapped in an SExpr
        let cases = match &args[1] {
            MettaValue::SExpr(items) => items,
            _ => return Err(CompileError::InvalidExpression(
                "case branches must be an S-expression".to_string()
            )),
        };

        // Compile each case as pattern matching
        let mut end_jumps = Vec::new();

        for case in cases {
            let (pattern, result) = match case {
                MettaValue::SExpr(items) if items.len() == 2 => (&items[0], &items[1]),
                _ => return Err(CompileError::InvalidExpression(
                    "case branch must be (pattern result)".to_string()
                )),
            };

            // Duplicate scrutinee for matching
            self.builder.emit(Opcode::Dup);

            // Compile pattern as constant
            self.compile_quoted(pattern)?;

            // Try to match
            self.builder.emit(Opcode::MatchBind);

            // Jump to next case if no match
            let next_case = self.builder.emit_jump(Opcode::JumpIfFalse);

            // Pop scrutinee (match succeeded)
            self.builder.emit(Opcode::Pop);

            // Compile result
            self.compile(result)?;

            // Jump to end
            end_jumps.push(self.builder.emit_jump(Opcode::Jump));

            // Patch next case jump
            self.builder.patch_jump(next_case);
        }

        // No match - keep scrutinee as result (or could emit Fail)

        // Patch all end jumps
        for jump in end_jumps {
            self.builder.patch_jump(jump);
        }

        Ok(())
    }

    /// Compile chain: (chain expr $var body)
    /// Chain evaluates expr, binds result to $var, then evaluates body
    pub(crate) fn compile_chain(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("chain", args.len(), 3)?;

        let expr = &args[0];
        let var = &args[1];
        let body = &args[2];

        // Begin new scope for the chain variable
        self.context.begin_scope();

        // Expression is NOT in tail position
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(expr)?;
        self.in_tail_position = saved_tail;

        // Bind the result to the variable
        self.compile_pattern_binding(var)?;

        // Body inherits parent's tail position
        self.compile(body)?;

        // End scope and clean up locals
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

    /// Compile a conjunction (multiple values)
    pub(crate) fn compile_conjunction(&mut self, values: &[MettaValue]) -> CompileResult<()> {
        if values.is_empty() {
            self.builder.emit(Opcode::Fail);
            return Ok(());
        }

        if values.len() == 1 {
            return self.compile(&values[0]);
        }

        // Multiple values - use Fork
        let mut alt_indices = Vec::new();
        for v in values {
            let idx = self.builder.add_constant(v.clone());
            alt_indices.push(idx);
        }

        self.builder.emit_u16(Opcode::Fork, alt_indices.len() as u16);
        for idx in alt_indices {
            self.builder.emit_raw(&idx.to_be_bytes());
        }

        Ok(())
    }
}
