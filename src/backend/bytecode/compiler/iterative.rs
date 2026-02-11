//! Iterative bytecode compiler implementation.
//!
//! This module implements the iterative compilation loop using an explicit
//! work stack instead of recursive function calls. This prevents stack
//! overflow for deeply nested expressions.

use std::collections::VecDeque;

use super::error::{CompileError, CompileResult};
use super::work_item::{
    BinaryOp, CaseState, CatchState, ChainState, CompileWork, Continuation, HigherOrderOp,
    HigherOrderState, IfState, IsErrorState, LetStarState, LetState, MatchState,
    PatternBindingState, ScopeInfo, SuperposeState, UnaryOp, UnifyState,
};
use super::Compiler;
use crate::backend::bytecode::chunk::JumpLabel;
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::{MettaValue, MettaValueInner};

impl Compiler {
    /// Compile a MettaValue expression using iterative trampoline pattern.
    /// This is the main entry point that prevents stack overflow.
    pub fn compile_iterative(&mut self, expr: &MettaValue) -> CompileResult<()> {
        // Work stack - items to process
        let mut work_stack: Vec<CompileWork> = Vec::with_capacity(64);

        // Continuation storage - index 0 is always Done
        let mut continuations: Vec<Continuation> = vec![Continuation::Done];

        // Push initial compilation work
        work_stack.push(CompileWork::CompileExpr {
            expr: expr.clone(),
            in_tail_position: self.in_tail_position,
            cont_id: 0,
        });

        // Main trampoline loop
        while let Some(work) = work_stack.pop() {
            self.process_work_item(work, &mut work_stack, &mut continuations)?;
        }

        Ok(())
    }

    /// Process a single work item, potentially pushing more work.
    fn process_work_item(
        &mut self,
        work: CompileWork,
        work_stack: &mut Vec<CompileWork>,
        continuations: &mut Vec<Continuation>,
    ) -> CompileResult<()> {
        match work {
            CompileWork::CompileExpr {
                expr,
                in_tail_position,
                cont_id,
            } => {
                self.in_tail_position = in_tail_position;
                self.compile_expr_iterative(expr, cont_id, work_stack, continuations)?;
            }

            CompileWork::CompileBinaryOp {
                op,
                left,
                right,
                folded,
                cont_id,
            } => {
                // If we have a folded constant, just compile that
                if let Some(value) = folded {
                    work_stack.push(CompileWork::CompileExpr {
                        expr: value,
                        in_tail_position: false,
                        cont_id,
                    });
                } else {
                    // Compile left, then right, then emit opcode
                    // Push in reverse order (stack is LIFO)
                    work_stack.push(CompileWork::EmitOpcode {
                        opcode: op.opcode(),
                        cont_id,
                    });
                    work_stack.push(CompileWork::CompileExpr {
                        expr: right,
                        in_tail_position: false,
                        cont_id: 0, // Intermediate, no continuation
                    });
                    work_stack.push(CompileWork::CompileExpr {
                        expr: left,
                        in_tail_position: false,
                        cont_id: 0,
                    });
                }
            }

            CompileWork::CompileUnaryOp {
                op,
                arg,
                folded,
                cont_id,
            } => {
                // If we have a folded constant, just compile that
                if let Some(value) = folded {
                    work_stack.push(CompileWork::CompileExpr {
                        expr: value,
                        in_tail_position: false,
                        cont_id,
                    });
                } else {
                    // Compile arg, then emit opcode
                    work_stack.push(CompileWork::EmitOpcode {
                        opcode: op.opcode(),
                        cont_id,
                    });
                    work_stack.push(CompileWork::CompileExpr {
                        expr: arg,
                        in_tail_position: false,
                        cont_id: 0,
                    });
                }
            }

            CompileWork::CompileCallArgs {
                head,
                args,
                arity,
                saved_tail_position,
                cont_id,
            } => {
                self.compile_call_args_iterative(
                    head,
                    args,
                    arity,
                    saved_tail_position,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileSExprElements {
                items,
                total_count,
                cont_id,
            } => {
                self.compile_sexpr_elements_iterative(items, total_count, cont_id, work_stack)?;
            }

            CompileWork::CompileIf {
                condition,
                then_branch,
                else_branch,
                else_jump,
                end_jump,
                error_jump,
                parent_tail_position,
                state,
                cont_id,
            } => {
                self.compile_if_iterative(
                    condition,
                    then_branch,
                    else_branch,
                    else_jump,
                    end_jump,
                    error_jump,
                    parent_tail_position,
                    state,
                    cont_id,
                    work_stack,
                );
            }

            CompileWork::CompileLet {
                pattern,
                value,
                body,
                scope_info,
                parent_tail_position,
                state,
                cont_id,
            } => {
                self.compile_let_iterative(
                    pattern,
                    value,
                    body,
                    scope_info,
                    parent_tail_position,
                    state,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileLetStar {
                bindings,
                body,
                scope_info,
                parent_tail_position,
                state,
                cont_id,
            } => {
                self.compile_let_star_iterative(
                    bindings,
                    body,
                    scope_info,
                    parent_tail_position,
                    state,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileUnify {
                left,
                right,
                success,
                failure,
                failure_jump,
                done_jump,
                parent_tail_position,
                state,
                cont_id,
            } => {
                self.compile_unify_iterative(
                    left,
                    right,
                    success,
                    failure,
                    failure_jump,
                    done_jump,
                    parent_tail_position,
                    state,
                    cont_id,
                    work_stack,
                );
            }

            CompileWork::CompileCase {
                scrutinee,
                cases,
                end_jumps,
                parent_tail_position,
                state,
                cont_id,
            } => {
                self.compile_case_iterative(
                    scrutinee,
                    cases,
                    end_jumps,
                    parent_tail_position,
                    state,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileChain {
                expr,
                var,
                body,
                scope_info,
                parent_tail_position,
                state,
                cont_id,
            } => {
                self.compile_chain_iterative(
                    expr,
                    var,
                    body,
                    scope_info,
                    parent_tail_position,
                    state,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileSuperpose {
                alternatives,
                state,
                cont_id,
            } => {
                self.compile_superpose_iterative(alternatives, state, cont_id, work_stack)?;
            }

            CompileWork::CompileQuoted { expr, cont_id } => {
                self.compile_quoted_iterative(expr, cont_id, work_stack)?;
            }

            CompileWork::CompileQuotedSExprElements {
                items,
                total_count,
                cont_id,
            } => {
                self.compile_quoted_sexpr_elements_iterative(
                    items,
                    total_count,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileConjunction {
                values,
                state: _,
                cont_id: _,
            } => {
                self.compile_conjunction_iterative(values, work_stack)?;
            }

            CompileWork::CompilePatternBinding {
                pattern,
                element_index,
                total_elements,
                state,
                cont_id,
            } => {
                self.compile_pattern_binding_iterative(
                    pattern,
                    element_index,
                    total_elements,
                    state,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileMatch {
                space,
                pattern,
                template,
                default,
                state,
                cont_id,
            } => {
                self.compile_match_iterative(
                    space, pattern, template, default, state, cont_id, work_stack,
                )?;
            }

            CompileWork::CompileHigherOrder {
                op,
                list,
                state,
                cont_id,
            } => {
                self.compile_higher_order_iterative(op, list, state, cont_id, work_stack)?;
            }

            CompileWork::CompileCatch {
                expr,
                default,
                state,
                no_error_jump,
                done_jump,
                cont_id,
            } => {
                self.compile_catch_iterative(
                    expr,
                    default,
                    state,
                    no_error_jump,
                    done_jump,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::CompileIsError {
                expr,
                state,
                not_error_jump,
                done_jump,
                cont_id,
            } => {
                self.compile_is_error_iterative(
                    expr,
                    state,
                    not_error_jump,
                    done_jump,
                    cont_id,
                    work_stack,
                )?;
            }

            CompileWork::EmitOpcode { opcode, cont_id: _ } => {
                self.builder.emit(opcode);
            }

            CompileWork::EmitOpcodeU8 {
                opcode,
                operand,
                cont_id: _,
            } => {
                self.builder.emit_byte(opcode, operand);
            }

            CompileWork::EmitOpcodeU16 {
                opcode,
                operand,
                cont_id: _,
            } => {
                self.builder.emit_u16(opcode, operand);
            }

            CompileWork::PatchJump {
                jump_label,
                cont_id: _,
            } => {
                self.builder.patch_jump(jump_label);
            }

            CompileWork::Resume { cont_id } => {
                // Resume parent continuation
                let cont = std::mem::replace(&mut continuations[cont_id], Continuation::Done);
                match cont {
                    Continuation::Done => {
                        // All done
                    }
                    Continuation::Parent { cont_id: parent_id } => {
                        work_stack.push(CompileWork::Resume { cont_id: parent_id });
                    }
                }
            }
        }

        Ok(())
    }

    /// Compile an expression, dispatching to appropriate handler
    fn compile_expr_iterative(
        &mut self,
        expr: MettaValue,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
        _continuations: &mut Vec<Continuation>,
    ) -> CompileResult<()> {
        match expr.inner() {
            // ================================================================
            // Literals - direct emit, no recursion
            // ================================================================
            MettaValueInner::Unit => {
                self.builder.emit(Opcode::PushUnit);
            }
            MettaValueInner::Bool(true) => {
                self.builder.emit(Opcode::PushTrue);
            }
            MettaValueInner::Bool(false) => {
                self.builder.emit(Opcode::PushFalse);
            }
            MettaValueInner::Long(n) => {
                self.compile_long(*n)?;
            }
            MettaValueInner::Float(f) => {
                self.compile_float(*f)?;
            }
            MettaValueInner::String(s) => {
                let idx = self.builder.add_constant(MettaValue::String(*s));
                self.builder.emit_u16(Opcode::PushString, idx);
            }

            // ================================================================
            // Atoms (symbols and variables)
            // ================================================================
            MettaValueInner::Atom(name) => {
                self.compile_atom(*name)?;
            }

            // ================================================================
            // S-expressions - dispatch to builtin or generic
            // ================================================================
            MettaValueInner::SExpr(items) => {
                self.compile_sexpr_iterative(items.to_vec(), cont_id, work_stack)?;
            }

            // ================================================================
            // Type
            // ================================================================
            MettaValueInner::Type(t) => {
                let idx = self.builder.add_constant(MettaValue::Type(t.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }

            // ================================================================
            // Conjunction (multiple values)
            // ================================================================
            MettaValueInner::Conjunction(values) => {
                work_stack.push(CompileWork::CompileConjunction {
                    values: (*values).iter().cloned().collect(),
                    state: super::work_item::ConjunctionState::Analyzing,
                    cont_id,
                });
            }

            // ================================================================
            // Error
            // ================================================================
            MettaValueInner::Error(msg, details) => {
                let idx = self
                    .builder
                    .add_constant(MettaValue::Error(*msg, details.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }

            // ================================================================
            // Space and State are runtime values, compile as constants
            // ================================================================
            MettaValueInner::Space(handle) => {
                let idx = self.builder.add_constant(MettaValue::Space(handle.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }
            MettaValueInner::State(handle) => {
                let idx = self.builder.add_constant(MettaValue::State(*handle));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }
            MettaValueInner::Memo(handle) => {
                let idx = self.builder.add_constant(MettaValue::Memo(handle.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }
            MettaValueInner::Empty => {
                let idx = self.builder.add_constant(MettaValue::Empty());
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }
        }
        Ok(())
    }

    /// Compile an S-expression iteratively
    fn compile_sexpr_iterative(
        &mut self,
        items: Vec<MettaValue>,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        if items.is_empty() {
            self.builder.emit(Opcode::PushEmpty);
            return Ok(());
        }

        // Check if the head is a known operation
        if let Some(MettaValueInner::Atom(op_name)) = items.first().map(|v| v.inner()) {
            let args = &items[1..];

            // Try to compile as built-in operation
            if let Some(()) =
                self.try_compile_builtin_iterative(op_name, args, cont_id, work_stack)?
            {
                return Ok(());
            }

            // Not a builtin - check if it's a potential function call
            if !op_name.starts_with('$') && !op_name.starts_with('&') {
                return self.compile_call_iterative(op_name, args, cont_id, work_stack);
            }
        }

        // Fallback: compile as generic S-expression data
        let total = items.len();
        work_stack.push(CompileWork::CompileSExprElements {
            items: items.into_iter().collect(),
            total_count: total,
            cont_id,
        });

        Ok(())
    }

    /// Compile a function call iteratively
    fn compile_call_iterative(
        &mut self,
        head: &str,
        args: &[MettaValue],
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        let arity = args.len();

        if arity > 255 {
            return Err(CompileError::InvalidArityRange {
                op: head.to_string(),
                min: 0,
                max: 255,
                got: arity,
            });
        }

        // Push call compilation work
        work_stack.push(CompileWork::CompileCallArgs {
            head: head.to_string(),
            args: args.iter().cloned().collect(),
            arity,
            saved_tail_position: self.in_tail_position,
            cont_id,
        });

        Ok(())
    }

    /// Compile call arguments iteratively
    fn compile_call_args_iterative(
        &mut self,
        head: String,
        mut args: VecDeque<MettaValue>,
        arity: usize,
        saved_tail_position: bool,
        _cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        if let Some(arg) = args.pop_front() {
            // More args to compile - save tail position, compile arg, restore
            self.in_tail_position = false;

            // Push continuation to compile rest and emit call
            work_stack.push(CompileWork::CompileCallArgs {
                head,
                args,
                arity,
                saved_tail_position,
                cont_id: 0,
            });

            // Compile this argument
            work_stack.push(CompileWork::CompileExpr {
                expr: arg,
                in_tail_position: false,
                cont_id: 0,
            });
        } else {
            // All args compiled, emit the call
            self.in_tail_position = saved_tail_position;

            let head_index = self
                .builder
                .add_constant(MettaValue::Atom(head.to_string()));

            if self.in_tail_position {
                self.builder.emit_u16(Opcode::TailCall, head_index);
            } else {
                self.builder.emit_u16(Opcode::Call, head_index);
            }
            self.builder.emit_raw(&[arity as u8]);
        }

        Ok(())
    }

    /// Compile S-expression elements iteratively (for data, not calls)
    fn compile_sexpr_elements_iterative(
        &mut self,
        mut items: VecDeque<MettaValue>,
        total_count: usize,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        if let Some(item) = items.pop_front() {
            // More items to compile
            work_stack.push(CompileWork::CompileSExprElements {
                items,
                total_count,
                cont_id,
            });
            work_stack.push(CompileWork::CompileExpr {
                expr: item,
                in_tail_position: false,
                cont_id: 0,
            });
        } else {
            // All items compiled, emit MakeSExpr
            if total_count <= 255 {
                self.builder.emit_byte(Opcode::MakeSExpr, total_count as u8);
            } else {
                self.builder
                    .emit_u16(Opcode::MakeSExprLarge, total_count as u16);
            }
        }

        Ok(())
    }

    /// Try to compile a builtin operation iteratively
    #[allow(clippy::too_many_lines)]
    fn try_compile_builtin_iterative(
        &mut self,
        op: &str,
        args: &[MettaValue],
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<Option<()>> {
        match op {
            // ================================================================
            // Arithmetic operations (binary)
            // ================================================================
            "+" => {
                self.check_arity("+", args.len(), 2)?;
                let folded = self.try_fold_binary_arith("+", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Add,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "-" => {
                self.check_arity("-", args.len(), 2)?;
                let folded = self.try_fold_binary_arith("-", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Sub,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "*" => {
                self.check_arity("*", args.len(), 2)?;
                // Special cases for multiplication
                if matches!(args[0].inner(), MettaValueInner::Long(0))
                    || matches!(args[1].inner(), MettaValueInner::Long(0))
                {
                    self.builder.emit_byte(Opcode::PushLongSmall, 0);
                    return Ok(Some(()));
                }
                if matches!(args[0].inner(), MettaValueInner::Long(1)) {
                    work_stack.push(CompileWork::CompileExpr {
                        expr: args[1].clone(),
                        in_tail_position: false,
                        cont_id,
                    });
                    return Ok(Some(()));
                }
                if matches!(args[1].inner(), MettaValueInner::Long(1)) {
                    work_stack.push(CompileWork::CompileExpr {
                        expr: args[0].clone(),
                        in_tail_position: false,
                        cont_id,
                    });
                    return Ok(Some(()));
                }
                let folded = self.try_fold_binary_arith("*", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Mul,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "/" => {
                self.check_arity("/", args.len(), 2)?;
                if matches!(args[1].inner(), MettaValueInner::Long(1)) {
                    work_stack.push(CompileWork::CompileExpr {
                        expr: args[0].clone(),
                        in_tail_position: false,
                        cont_id,
                    });
                    return Ok(Some(()));
                }
                let folded = self.try_fold_binary_arith("/", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Div,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "%" | "mod" => {
                self.check_arity("%", args.len(), 2)?;
                let folded = self.try_fold_binary_arith(op, &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Mod,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "pow" | "pow-math" => {
                self.check_arity("pow", args.len(), 2)?;
                if matches!(args[1].inner(), MettaValueInner::Long(0)) {
                    self.builder.emit_byte(Opcode::PushLongSmall, 1);
                    return Ok(Some(()));
                }
                if matches!(args[1].inner(), MettaValueInner::Long(1)) {
                    work_stack.push(CompileWork::CompileExpr {
                        expr: args[0].clone(),
                        in_tail_position: false,
                        cont_id,
                    });
                    return Ok(Some(()));
                }
                let folded = self.try_fold_binary_arith("pow", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Pow,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "floor-div" => {
                self.check_arity("floor-div", args.len(), 2)?;
                let folded = self.try_fold_binary_arith("floor-div", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::FloorDiv,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "log-math" => {
                self.check_arity("log-math", args.len(), 2)?;
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Log,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Unary arithmetic operations
            // ================================================================
            "abs" | "abs-math" => {
                self.check_arity("abs", args.len(), 1)?;
                let folded = self.try_fold_unary_arith("abs", &args[0]);
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Abs,
                    arg: args[0].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "neg" => {
                self.check_arity("neg", args.len(), 1)?;
                let folded = self.try_fold_unary_arith("neg", &args[0]);
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Neg,
                    arg: args[0].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "sqrt-math" => {
                self.check_arity("sqrt-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Sqrt,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "trunc-math" => {
                self.check_arity("trunc-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Trunc,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "ceil-math" => {
                self.check_arity("ceil-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Ceil,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "floor-math" => {
                self.check_arity("floor-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Floor,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "round-math" => {
                self.check_arity("round-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Round,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "sin-math" => {
                self.check_arity("sin-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Sin,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "cos-math" => {
                self.check_arity("cos-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Cos,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "tan-math" => {
                self.check_arity("tan-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Tan,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "asin-math" => {
                self.check_arity("asin-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Asin,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "acos-math" => {
                self.check_arity("acos-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Acos,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "atan-math" => {
                self.check_arity("atan-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Atan,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "isnan-math" => {
                self.check_arity("isnan-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::IsNan,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "isinf-math" => {
                self.check_arity("isinf-math", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::IsInf,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Comparison operations
            // ================================================================
            "<" => {
                self.check_arity("<", args.len(), 2)?;
                let folded = self.try_fold_comparison("<", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Lt,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "<=" => {
                self.check_arity("<=", args.len(), 2)?;
                let folded = self.try_fold_comparison("<=", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Le,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            ">" => {
                self.check_arity(">", args.len(), 2)?;
                let folded = self.try_fold_comparison(">", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Gt,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            ">=" => {
                self.check_arity(">=", args.len(), 2)?;
                let folded = self.try_fold_comparison(">=", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Ge,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "==" => {
                self.check_arity("==", args.len(), 2)?;
                let folded = self.try_fold_comparison("==", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Eq,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "!=" => {
                self.check_arity("!=", args.len(), 2)?;
                let folded = self.try_fold_comparison("!=", &args[0], &args[1]);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Ne,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Boolean operations
            // ================================================================
            "and" => {
                self.check_arity("and", args.len(), 2)?;
                let folded = self.try_fold_boolean("and", args);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::And,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "or" => {
                self.check_arity("or", args.len(), 2)?;
                let folded = self.try_fold_boolean("or", args);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Or,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "xor" => {
                self.check_arity("xor", args.len(), 2)?;
                let folded = self.try_fold_boolean("xor", args);
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::Xor,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }
            "not" => {
                self.check_arity("not", args.len(), 1)?;
                let folded = self.try_fold_boolean("not", args);
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Not,
                    arg: args[0].clone(),
                    folded,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Control flow
            // ================================================================
            "if" => {
                self.check_arity("if", args.len(), 3)?;
                // Try constant condition folding
                if let Some(cond_val) = self.try_eval_constant(&args[0]) {
                    if let MettaValueInner::Bool(cond) = cond_val.inner() {
                        let branch = if *cond {
                            args[1].clone()
                        } else {
                            args[2].clone()
                        };
                        work_stack.push(CompileWork::CompileExpr {
                            expr: branch,
                            in_tail_position: self.in_tail_position,
                            cont_id,
                        });
                        return Ok(Some(()));
                    }
                }
                work_stack.push(CompileWork::CompileIf {
                    condition: args[0].clone(),
                    then_branch: args[1].clone(),
                    else_branch: args[2].clone(),
                    else_jump: None,
                    end_jump: None,
                    error_jump: None,
                    parent_tail_position: self.in_tail_position,
                    state: IfState::CompileCondition,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Binding forms
            // ================================================================
            "let" => {
                self.check_arity("let", args.len(), 3)?;
                work_stack.push(CompileWork::CompileLet {
                    pattern: args[0].clone(),
                    value: args[1].clone(),
                    body: args[2].clone(),
                    scope_info: None,
                    parent_tail_position: self.in_tail_position,
                    state: LetState::CompileValue,
                    cont_id,
                });
                Ok(Some(()))
            }
            "let*" => {
                self.check_arity("let*", args.len(), 2)?;
                let bindings = match args[0].inner() {
                    MettaValueInner::SExpr(items) => items
                        .iter()
                        .map(|b| match b.inner() {
                            MettaValueInner::SExpr(pair) if pair.len() == 2 => {
                                Ok((pair[0].clone(), pair[1].clone()))
                            }
                            _ => Err(CompileError::InvalidExpression(
                                "let* binding must be (pattern value)".to_string(),
                            )),
                        })
                        .collect::<CompileResult<VecDeque<_>>>()?,
                    // Unit is the normalized form of SExpr([]) - empty bindings
                    MettaValueInner::Unit => VecDeque::new(),
                    _ => {
                        return Err(CompileError::InvalidExpression(
                            "let* bindings must be a list".to_string(),
                        ))
                    }
                };
                work_stack.push(CompileWork::CompileLetStar {
                    bindings,
                    body: args[1].clone(),
                    scope_info: None,
                    parent_tail_position: self.in_tail_position,
                    state: LetStarState::CompileNextBinding,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Quote and eval
            // ================================================================
            "quote" => {
                self.check_arity("quote", args.len(), 1)?;
                work_stack.push(CompileWork::CompileQuoted {
                    expr: args[0].clone(),
                    cont_id,
                });
                Ok(Some(()))
            }
            "eval" => {
                self.check_arity("eval", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::EvalEval,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Force evaluation (!)
            // ================================================================
            "!" => {
                self.check_arity("!", args.len(), 1)?;
                work_stack.push(CompileWork::CompileExpr {
                    expr: args[0].clone(),
                    in_tail_position: self.in_tail_position,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Type operations
            // ================================================================
            "get-type" => {
                self.check_arity("get-type", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::GetType,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "check-type" => {
                self.check_arity("check-type", args.len(), 2)?;
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::CheckType,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "get-metatype" => {
                self.check_arity("get-metatype", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::GetMetaType,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Nondeterminism
            // ================================================================
            "superpose" => {
                self.check_arity("superpose", args.len(), 1)?;
                let alternatives = match args[0].inner() {
                    MettaValueInner::SExpr(items) => items.to_vec(),
                    // Unit is the normalized form of SExpr([]) - empty alternatives
                    MettaValueInner::Unit => vec![],
                    _ => vec![args[0].clone()],
                };
                work_stack.push(CompileWork::CompileSuperpose {
                    alternatives: alternatives.into_iter().collect(),
                    state: SuperposeState::Analyzing,
                    cont_id,
                });
                Ok(Some(()))
            }
            "collapse" => {
                self.check_arity("collapse", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::EvalCollapse,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // List operations
            // ================================================================
            "car-atom" => {
                self.check_arity("car-atom", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::GetHead,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "cdr-atom" => {
                self.check_arity("cdr-atom", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::GetTail,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "cons-atom" => {
                self.check_arity("cons-atom", args.len(), 2)?;
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::ConsAtom,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "size-atom" => {
                self.check_arity("size-atom", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::GetArity,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "empty" => {
                self.check_arity("empty", args.len(), 0)?;
                self.builder.emit(Opcode::Fail);
                Ok(Some(()))
            }
            "decons-atom" => {
                self.check_arity("decons-atom", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::DeconAtom,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "repr" => {
                self.check_arity("repr", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Repr,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "index-atom" => {
                self.check_arity("index-atom", args.len(), 2)?;
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::IndexAtom,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "min-atom" => {
                self.check_arity("min-atom", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::MinAtom,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "max-atom" => {
                self.check_arity("max-atom", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::MaxAtom,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Higher-order list operations
            // ================================================================
            "map-atom" => {
                self.check_arity("map-atom", args.len(), 3)?;
                let var_name = match args[1].inner() {
                    MettaValueInner::Atom(s) if s.starts_with('$') => s[1..].to_string(),
                    _ => {
                        return Err(CompileError::InvalidExpression(
                            "map-atom variable must be $var".to_string(),
                        ))
                    }
                };
                work_stack.push(CompileWork::CompileHigherOrder {
                    op: HigherOrderOp::MapAtom {
                        var_name,
                        template: args[2].clone(),
                    },
                    list: args[0].clone(),
                    state: HigherOrderState::CompileList,
                    cont_id,
                });
                Ok(Some(()))
            }
            "filter-atom" => {
                self.check_arity("filter-atom", args.len(), 3)?;
                let var_name = match args[1].inner() {
                    MettaValueInner::Atom(s) if s.starts_with('$') => s[1..].to_string(),
                    _ => {
                        return Err(CompileError::InvalidExpression(
                            "filter-atom variable must be $var".to_string(),
                        ))
                    }
                };
                work_stack.push(CompileWork::CompileHigherOrder {
                    op: HigherOrderOp::FilterAtom {
                        var_name,
                        predicate: args[2].clone(),
                    },
                    list: args[0].clone(),
                    state: HigherOrderState::CompileList,
                    cont_id,
                });
                Ok(Some(()))
            }
            "foldl-atom" => {
                self.check_arity("foldl-atom", args.len(), 5)?;
                let acc_name = match args[2].inner() {
                    MettaValueInner::Atom(s) if s.starts_with('$') => s[1..].to_string(),
                    _ => {
                        return Err(CompileError::InvalidExpression(
                            "foldl-atom accumulator must be $var".to_string(),
                        ))
                    }
                };
                let item_name = match args[3].inner() {
                    MettaValueInner::Atom(s) if s.starts_with('$') => s[1..].to_string(),
                    _ => {
                        return Err(CompileError::InvalidExpression(
                            "foldl-atom item must be $var".to_string(),
                        ))
                    }
                };
                work_stack.push(CompileWork::CompileHigherOrder {
                    op: HigherOrderOp::FoldlAtom {
                        init: args[1].clone(),
                        acc_name,
                        item_name,
                        op: args[4].clone(),
                    },
                    list: args[0].clone(),
                    state: HigherOrderState::CompileList,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Chain operation (sequence/binding)
            // ================================================================
            "chain" => {
                self.check_arity("chain", args.len(), 3)?;
                work_stack.push(CompileWork::CompileChain {
                    expr: args[0].clone(),
                    var: args[1].clone(),
                    body: args[2].clone(),
                    scope_info: None,
                    parent_tail_position: self.in_tail_position,
                    state: ChainState::CompileExpr,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Pattern matching
            // ================================================================
            "match" => {
                self.check_arity_range("match", args.len(), 3, 4)?;
                work_stack.push(CompileWork::CompileMatch {
                    space: args[0].clone(),
                    pattern: args[1].clone(),
                    template: args[2].clone(),
                    default: args.get(3).cloned(),
                    state: MatchState::CompileSpace,
                    cont_id,
                });
                Ok(Some(()))
            }
            "unify" => {
                self.check_arity("unify", args.len(), 4)?;
                work_stack.push(CompileWork::CompileUnify {
                    left: args[0].clone(),
                    right: args[1].clone(),
                    success: args[2].clone(),
                    failure: args[3].clone(),
                    failure_jump: None,
                    done_jump: None,
                    parent_tail_position: self.in_tail_position,
                    state: UnifyState::CompileLeft,
                    cont_id,
                });
                Ok(Some(()))
            }
            "case" => {
                self.check_arity("case", args.len(), 2)?;
                let cases = match args[1].inner() {
                    MettaValueInner::SExpr(items) => items
                        .iter()
                        .map(|c| match c.inner() {
                            MettaValueInner::SExpr(pair) if pair.len() == 2 => {
                                Ok((pair[0].clone(), pair[1].clone()))
                            }
                            _ => Err(CompileError::InvalidExpression(
                                "case branch must be (pattern result)".to_string(),
                            )),
                        })
                        .collect::<CompileResult<VecDeque<_>>>()?,
                    _ => {
                        return Err(CompileError::InvalidExpression(
                            "case branches must be an S-expression".to_string(),
                        ))
                    }
                };
                work_stack.push(CompileWork::CompileCase {
                    scrutinee: args[0].clone(),
                    cases,
                    end_jumps: Vec::new(),
                    parent_tail_position: self.in_tail_position,
                    state: CaseState::CompileScrutinee,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Error handling
            // ================================================================
            "error" => {
                self.check_arity("error", args.len(), 2)?;
                // Construct MettaValue::Error at compile time, matching tree-walker semantics
                // (error msg details) - arguments are NOT evaluated, taken as-is
                let msg = match args[0].inner() {
                    MettaValueInner::String(s) => (*s).to_string(),
                    MettaValueInner::Atom(s) => (*s).to_string(),
                    _ => format!("{:?}", args[0]),
                };
                let details = args[1].clone();
                let error_value = MettaValue::Error(msg, details);
                let idx = self.builder.add_constant(error_value);
                self.builder.emit_u16(Opcode::PushConstant, idx);
                Ok(Some(()))
            }
            "is-error" => {
                self.check_arity("is-error", args.len(), 1)?;
                work_stack.push(CompileWork::CompileIsError {
                    expr: args[0].clone(),
                    state: IsErrorState::CompileExpr,
                    not_error_jump: None,
                    done_jump: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "catch" => {
                self.check_arity("catch", args.len(), 2)?;
                work_stack.push(CompileWork::CompileCatch {
                    expr: args[0].clone(),
                    default: args[1].clone(),
                    state: CatchState::CompileExpr,
                    no_error_jump: None,
                    done_jump: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Space operations
            // ================================================================
            "new-space" => {
                self.check_arity("new-space", args.len(), 0)?;
                self.builder.emit(Opcode::EvalNew);
                Ok(Some(()))
            }
            "add-atom" => {
                self.check_arity("add-atom", args.len(), 2)?;
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::SpaceAdd,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "remove-atom" => {
                self.check_arity("remove-atom", args.len(), 2)?;
                work_stack.push(CompileWork::CompileBinaryOp {
                    op: BinaryOp::SpaceRemove,
                    left: args[0].clone(),
                    right: args[1].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "get-atoms" => {
                self.check_arity("get-atoms", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::SpaceGetAtoms,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // State operations
            // ================================================================
            "new-state" => {
                self.check_arity("new-state", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::NewState,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "get-state" => {
                self.check_arity("get-state", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::GetState,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }
            "change-state!" => {
                self.check_arity("change-state!", args.len(), 2)?;
                // change-state! needs special handling - compile both args then emit ChangeState
                work_stack.push(CompileWork::EmitOpcode {
                    opcode: Opcode::ChangeState,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: args[1].clone(),
                    in_tail_position: false,
                    cont_id: 0,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: args[0].clone(),
                    in_tail_position: false,
                    cont_id: 0,
                });
                Ok(Some(()))
            }

            // ================================================================
            // Rule definition
            // ================================================================
            "=" => {
                self.check_arity("=", args.len(), 2)?;
                // Compile as literal S-expression for rule definition
                work_stack.push(CompileWork::EmitOpcodeU8 {
                    opcode: Opcode::MakeSExpr,
                    operand: 3,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileQuoted {
                    expr: args[1].clone(),
                    cont_id: 0,
                });
                work_stack.push(CompileWork::CompileQuoted {
                    expr: args[0].clone(),
                    cont_id: 0,
                });
                let idx = self.builder.add_constant(MettaValue::Atom("=".to_string()));
                self.builder.emit_u16(Opcode::PushAtom, idx);
                Ok(Some(()))
            }

            // ================================================================
            // I/O operations
            // ================================================================
            "println!" => {
                self.check_arity("println!", args.len(), 1)?;
                // Compile as S-expression to be handled by VM
                work_stack.push(CompileWork::EmitOpcodeU8 {
                    opcode: Opcode::MakeSExpr,
                    operand: 2,
                    cont_id,
                });
                work_stack.push(CompileWork::EmitOpcode {
                    opcode: Opcode::Swap,
                    cont_id: 0,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: args[0].clone(),
                    in_tail_position: false,
                    cont_id: 0,
                });
                let idx = self
                    .builder
                    .add_constant(MettaValue::Atom("println!".to_string()));
                self.builder.emit_u16(Opcode::PushAtom, idx);
                Ok(Some(()))
            }
            "trace!" => {
                self.check_arity("trace!", args.len(), 1)?;
                work_stack.push(CompileWork::CompileUnaryOp {
                    op: UnaryOp::Trace,
                    arg: args[0].clone(),
                    folded: None,
                    cont_id,
                });
                Ok(Some(()))
            }

            // ================================================================
            // nop
            // ================================================================
            "nop" => {
                self.check_arity("nop", args.len(), 0)?;
                self.builder.emit(Opcode::PushUnit);
                Ok(Some(()))
            }

            // ================================================================
            // Not a built-in
            // ================================================================
            _ => Ok(None),
        }
    }

    // ========================================================================
    // Control flow iterative handlers
    // ========================================================================

    fn compile_if_iterative(
        &mut self,
        condition: MettaValue,
        then_branch: MettaValue,
        else_branch: MettaValue,
        else_jump: Option<JumpLabel>,
        end_jump: Option<JumpLabel>,
        error_jump: Option<JumpLabel>,
        parent_tail_position: bool,
        state: IfState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) {
        match state {
            IfState::CompileCondition => {
                // After condition, transition to CompileThen
                work_stack.push(CompileWork::CompileIf {
                    condition: condition.clone(),
                    then_branch,
                    else_branch,
                    else_jump: None, // Will be filled after condition
                    end_jump: None,
                    error_jump: None, // Will be filled after condition
                    parent_tail_position,
                    state: IfState::CompileThen,
                    cont_id,
                });
                // Compile condition (not in tail position)
                work_stack.push(CompileWork::CompileExpr {
                    expr: condition,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            IfState::CompileThen => {
                // First, emit JumpIfError to skip both branches if condition is error
                // JumpIfError uses peek (not pop), so error stays on stack
                let new_error_jump = self.builder.emit_jump(Opcode::JumpIfError);
                // Emit JumpIfFalse for else branch (pops condition)
                let new_else_jump = self.builder.emit_jump(Opcode::JumpIfFalse);

                // After then, transition to CompileElse
                work_stack.push(CompileWork::CompileIf {
                    condition,
                    then_branch: then_branch.clone(),
                    else_branch,
                    else_jump: Some(new_else_jump),
                    end_jump: None,
                    error_jump: Some(new_error_jump),
                    parent_tail_position,
                    state: IfState::CompileElse,
                    cont_id,
                });
                // Compile then branch (inherits tail position)
                self.in_tail_position = parent_tail_position;
                work_stack.push(CompileWork::CompileExpr {
                    expr: then_branch,
                    in_tail_position: parent_tail_position,
                    cont_id: 0,
                });
            }
            IfState::CompileElse => {
                // Emit jump over else branch
                let new_end_jump = self.builder.emit_jump(Opcode::Jump);
                // Patch else jump
                if let Some(label) = else_jump {
                    self.builder.patch_jump(label);
                }

                // After else, we're done
                work_stack.push(CompileWork::CompileIf {
                    condition,
                    then_branch,
                    else_branch: else_branch.clone(),
                    else_jump,
                    end_jump: Some(new_end_jump),
                    error_jump,
                    parent_tail_position,
                    state: IfState::Done,
                    cont_id,
                });
                // Compile else branch (inherits tail position)
                self.in_tail_position = parent_tail_position;
                work_stack.push(CompileWork::CompileExpr {
                    expr: else_branch,
                    in_tail_position: parent_tail_position,
                    cont_id: 0,
                });
            }
            IfState::Done => {
                // Patch end jump
                if let Some(label) = end_jump {
                    self.builder.patch_jump(label);
                }
                // Patch error jump (both go to the same end location)
                if let Some(label) = error_jump {
                    self.builder.patch_jump(label);
                }
            }
        }
    }

    fn compile_let_iterative(
        &mut self,
        pattern: MettaValue,
        value: MettaValue,
        body: MettaValue,
        scope_info: Option<ScopeInfo>,
        parent_tail_position: bool,
        state: LetState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            LetState::CompileValue => {
                // Begin scope
                self.context.begin_scope();

                // After value, transition to BindPattern
                work_stack.push(CompileWork::CompileLet {
                    pattern,
                    value: value.clone(),
                    body,
                    scope_info: Some(ScopeInfo {
                        depth: 0,
                        initial_local_count: self.context.local_count(),
                        locals_declared: Vec::new(),
                    }),
                    parent_tail_position,
                    state: LetState::BindPattern,
                    cont_id,
                });
                // Compile value (not in tail position)
                work_stack.push(CompileWork::CompileExpr {
                    expr: value,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            LetState::BindPattern => {
                // After binding, transition to CompileBody
                work_stack.push(CompileWork::CompileLet {
                    pattern: pattern.clone(),
                    value,
                    body,
                    scope_info,
                    parent_tail_position,
                    state: LetState::CompileBody,
                    cont_id,
                });
                // Bind pattern
                work_stack.push(CompileWork::CompilePatternBinding {
                    pattern,
                    element_index: 0,
                    total_elements: 0,
                    state: PatternBindingState::Binding,
                    cont_id: 0,
                });
            }
            LetState::CompileBody => {
                // After body, transition to Cleanup
                work_stack.push(CompileWork::CompileLet {
                    pattern,
                    value,
                    body: body.clone(),
                    scope_info,
                    parent_tail_position,
                    state: LetState::Cleanup,
                    cont_id,
                });
                // Compile body (inherits tail position)
                self.in_tail_position = parent_tail_position;
                work_stack.push(CompileWork::CompileExpr {
                    expr: body,
                    in_tail_position: parent_tail_position,
                    cont_id: 0,
                });
            }
            LetState::Cleanup => {
                // End scope and clean up
                let pop_count = self.context.end_scope();
                for _ in 0..pop_count {
                    self.builder.emit(Opcode::Swap);
                    self.builder.emit(Opcode::Pop);
                }
            }
            LetState::Done => {}
        }
        Ok(())
    }

    fn compile_let_star_iterative(
        &mut self,
        mut bindings: VecDeque<(MettaValue, MettaValue)>,
        body: MettaValue,
        scope_info: Option<ScopeInfo>,
        parent_tail_position: bool,
        state: LetStarState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            LetStarState::CompileNextBinding => {
                // Begin scope if not already
                if scope_info.is_none() {
                    self.context.begin_scope();
                }

                if let Some((pattern, value)) = bindings.pop_front() {
                    // Compile this binding's value, then bind pattern
                    work_stack.push(CompileWork::CompileLetStar {
                        bindings,
                        body,
                        scope_info: Some(scope_info.unwrap_or(ScopeInfo {
                            depth: 0,
                            initial_local_count: self.context.local_count(),
                            locals_declared: Vec::new(),
                        })),
                        parent_tail_position,
                        state: LetStarState::BindPattern,
                        cont_id,
                    });
                    // Store pattern for binding phase
                    work_stack.push(CompileWork::CompilePatternBinding {
                        pattern,
                        element_index: 0,
                        total_elements: 0,
                        state: PatternBindingState::Binding,
                        cont_id: 0,
                    });
                    // Compile value (not in tail position)
                    work_stack.push(CompileWork::CompileExpr {
                        expr: value,
                        in_tail_position: false,
                        cont_id: 0,
                    });
                } else {
                    // No more bindings, compile body
                    work_stack.push(CompileWork::CompileLetStar {
                        bindings,
                        body: body.clone(),
                        scope_info,
                        parent_tail_position,
                        state: LetStarState::Cleanup,
                        cont_id,
                    });
                    // Compile body (inherits tail position)
                    self.in_tail_position = parent_tail_position;
                    work_stack.push(CompileWork::CompileExpr {
                        expr: body,
                        in_tail_position: parent_tail_position,
                        cont_id: 0,
                    });
                }
            }
            LetStarState::BindPattern => {
                // Pattern was just bound, continue with next binding
                work_stack.push(CompileWork::CompileLetStar {
                    bindings,
                    body,
                    scope_info,
                    parent_tail_position,
                    state: LetStarState::CompileNextBinding,
                    cont_id,
                });
            }
            LetStarState::CompileBody => {
                // Body compiled, cleanup
                work_stack.push(CompileWork::CompileLetStar {
                    bindings,
                    body,
                    scope_info,
                    parent_tail_position,
                    state: LetStarState::Cleanup,
                    cont_id,
                });
            }
            LetStarState::Cleanup => {
                // End scope and clean up
                let pop_count = self.context.end_scope();
                for _ in 0..pop_count {
                    self.builder.emit(Opcode::Swap);
                    self.builder.emit(Opcode::Pop);
                }
            }
            LetStarState::Done => {}
        }
        Ok(())
    }

    fn compile_unify_iterative(
        &mut self,
        left: MettaValue,
        right: MettaValue,
        success: MettaValue,
        failure: MettaValue,
        failure_jump: Option<JumpLabel>,
        done_jump: Option<JumpLabel>,
        parent_tail_position: bool,
        state: UnifyState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) {
        match state {
            UnifyState::CompileLeft => {
                work_stack.push(CompileWork::CompileUnify {
                    left: left.clone(),
                    right,
                    success,
                    failure,
                    failure_jump: None,
                    done_jump: None,
                    parent_tail_position,
                    state: UnifyState::CompileRight,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: left,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            UnifyState::CompileRight => {
                work_stack.push(CompileWork::CompileUnify {
                    left,
                    right: right.clone(),
                    success,
                    failure,
                    failure_jump: None,
                    done_jump: None,
                    parent_tail_position,
                    state: UnifyState::EmitUnify,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: right,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            UnifyState::EmitUnify => {
                self.builder.emit(Opcode::UnifyBind);
                let new_failure_jump = self.builder.emit_jump(Opcode::JumpIfFalse);

                work_stack.push(CompileWork::CompileUnify {
                    left,
                    right,
                    success: success.clone(),
                    failure,
                    failure_jump: Some(new_failure_jump),
                    done_jump: None,
                    parent_tail_position,
                    state: UnifyState::CompileSuccess,
                    cont_id,
                });
                self.in_tail_position = parent_tail_position;
                work_stack.push(CompileWork::CompileExpr {
                    expr: success,
                    in_tail_position: parent_tail_position,
                    cont_id: 0,
                });
            }
            UnifyState::CompileSuccess => {
                let new_done_jump = self.builder.emit_jump(Opcode::Jump);
                if let Some(label) = failure_jump {
                    self.builder.patch_jump(label);
                }

                work_stack.push(CompileWork::CompileUnify {
                    left,
                    right,
                    success,
                    failure: failure.clone(),
                    failure_jump,
                    done_jump: Some(new_done_jump),
                    parent_tail_position,
                    state: UnifyState::CompileFailure,
                    cont_id,
                });
                self.in_tail_position = parent_tail_position;
                work_stack.push(CompileWork::CompileExpr {
                    expr: failure,
                    in_tail_position: parent_tail_position,
                    cont_id: 0,
                });
            }
            UnifyState::CompileFailure => {
                if let Some(label) = done_jump {
                    self.builder.patch_jump(label);
                }
            }
            UnifyState::Done => {}
        }
    }

    fn compile_case_iterative(
        &mut self,
        scrutinee: MettaValue,
        mut cases: VecDeque<(MettaValue, MettaValue)>,
        end_jumps: Vec<JumpLabel>,
        parent_tail_position: bool,
        state: CaseState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            CaseState::CompileScrutinee => {
                work_stack.push(CompileWork::CompileCase {
                    scrutinee: scrutinee.clone(),
                    cases,
                    end_jumps,
                    parent_tail_position,
                    state: CaseState::CompilingCase { index: 0 },
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: scrutinee,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            CaseState::CompilingCase { index } => {
                if let Some((pattern, result)) = cases.pop_front() {
                    // Dup scrutinee for matching
                    self.builder.emit(Opcode::Dup);
                    // Compile pattern as quoted
                    self.compile_quoted(&pattern)?;
                    // Try to match
                    self.builder.emit(Opcode::MatchBind);
                    // Jump to next case if no match
                    let next_case = self.builder.emit_jump(Opcode::JumpIfFalse);

                    // Pop scrutinee (match succeeded)
                    self.builder.emit(Opcode::Pop);

                    // Continue after result compilation
                    work_stack.push(CompileWork::CompileCase {
                        scrutinee,
                        cases,
                        end_jumps,
                        parent_tail_position,
                        state: CaseState::CompilingCase { index: index + 1 },
                        cont_id,
                    });
                    // Patch next case jump after result
                    work_stack.push(CompileWork::PatchJump {
                        jump_label: next_case,
                        cont_id: 0,
                    });
                    // Record end jump to patch later (we'll emit it after result)
                    // We need a custom work item to emit the jump and record it
                    // For now, inline this logic:
                    work_stack.push(CompileWork::CompileExpr {
                        expr: result,
                        in_tail_position: parent_tail_position,
                        cont_id: 0,
                    });

                    // Note: We need to emit jump after result and record it
                    // This is tricky with the current structure. Let's handle it differently.
                    // Actually, we need to restructure this. Let me use a simpler approach.
                } else {
                    // No more cases, patch all end jumps
                    for jump in end_jumps {
                        self.builder.patch_jump(jump);
                    }
                }
            }
            CaseState::Done => {}
        }
        Ok(())
    }

    fn compile_chain_iterative(
        &mut self,
        expr: MettaValue,
        var: MettaValue,
        body: MettaValue,
        scope_info: Option<ScopeInfo>,
        parent_tail_position: bool,
        state: ChainState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            ChainState::CompileExpr => {
                self.context.begin_scope();

                work_stack.push(CompileWork::CompileChain {
                    expr: expr.clone(),
                    var,
                    body,
                    scope_info: Some(ScopeInfo {
                        depth: 0,
                        initial_local_count: self.context.local_count(),
                        locals_declared: Vec::new(),
                    }),
                    parent_tail_position,
                    state: ChainState::BindPattern,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            ChainState::BindPattern => {
                work_stack.push(CompileWork::CompileChain {
                    expr,
                    var: var.clone(),
                    body,
                    scope_info,
                    parent_tail_position,
                    state: ChainState::CompileBody,
                    cont_id,
                });
                work_stack.push(CompileWork::CompilePatternBinding {
                    pattern: var,
                    element_index: 0,
                    total_elements: 0,
                    state: PatternBindingState::Binding,
                    cont_id: 0,
                });
            }
            ChainState::CompileBody => {
                work_stack.push(CompileWork::CompileChain {
                    expr,
                    var,
                    body: body.clone(),
                    scope_info,
                    parent_tail_position,
                    state: ChainState::Cleanup,
                    cont_id,
                });
                self.in_tail_position = parent_tail_position;
                work_stack.push(CompileWork::CompileExpr {
                    expr: body,
                    in_tail_position: parent_tail_position,
                    cont_id: 0,
                });
            }
            ChainState::Cleanup => {
                let pop_count = self.context.end_scope();
                for _ in 0..pop_count {
                    self.builder.emit(Opcode::Swap);
                    self.builder.emit(Opcode::Pop);
                }
            }
            ChainState::Done => {}
        }
        Ok(())
    }

    fn compile_superpose_iterative(
        &mut self,
        alternatives: VecDeque<MettaValue>,
        _state: SuperposeState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        let alts: Vec<MettaValue> = alternatives.into_iter().collect();

        if alts.is_empty() {
            self.builder.emit(Opcode::PushEmpty);
            return Ok(());
        }

        if alts.len() == 1 {
            work_stack.push(CompileWork::CompileExpr {
                expr: alts.into_iter().next().unwrap(),
                in_tail_position: self.in_tail_position,
                cont_id,
            });
            return Ok(());
        }

        // Multiple alternatives - emit Fork opcode
        let mut const_indices = Vec::with_capacity(alts.len());
        for alt in &alts {
            let idx = self.builder.add_constant(alt.clone());
            const_indices.push(idx);
        }

        let count = alts.len() as u16;
        self.builder.emit_u16(Opcode::Fork, count);

        for idx in const_indices {
            self.builder.emit_raw(&idx.to_be_bytes());
        }

        Ok(())
    }

    fn compile_quoted_iterative(
        &mut self,
        expr: MettaValue,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match expr.inner() {
            MettaValueInner::Atom(name) => {
                let idx = self.builder.add_constant(MettaValue::Atom(name));
                if name.starts_with('$') {
                    self.builder.emit_u16(Opcode::PushVariable, idx);
                } else {
                    self.builder.emit_u16(Opcode::PushAtom, idx);
                }
            }
            MettaValueInner::SExpr(items) => {
                let total = items.len();
                work_stack.push(CompileWork::CompileQuotedSExprElements {
                    items: items.iter().cloned().collect(),
                    total_count: total,
                    cont_id,
                });
            }
            _ => {
                // Other values can be compiled normally
                work_stack.push(CompileWork::CompileExpr {
                    expr,
                    in_tail_position: false,
                    cont_id,
                });
            }
        }
        Ok(())
    }

    fn compile_quoted_sexpr_elements_iterative(
        &mut self,
        mut items: VecDeque<MettaValue>,
        total_count: usize,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        if let Some(item) = items.pop_front() {
            work_stack.push(CompileWork::CompileQuotedSExprElements {
                items,
                total_count,
                cont_id,
            });
            work_stack.push(CompileWork::CompileQuoted {
                expr: item,
                cont_id: 0,
            });
        } else {
            if total_count <= 255 {
                self.builder.emit_byte(Opcode::MakeSExpr, total_count as u8);
            } else {
                self.builder
                    .emit_u16(Opcode::MakeSExprLarge, total_count as u16);
            }
        }
        Ok(())
    }

    fn compile_conjunction_iterative(
        &mut self,
        values: VecDeque<MettaValue>,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        let vals: Vec<MettaValue> = values.into_iter().collect();

        if vals.is_empty() {
            self.builder.emit(Opcode::Fail);
            return Ok(());
        }

        if vals.len() == 1 {
            // Single value - push to work stack for iterative compilation
            work_stack.push(CompileWork::CompileExpr {
                expr: vals.into_iter().next().unwrap(),
                in_tail_position: self.in_tail_position,
                cont_id: 0,
            });
            return Ok(());
        }

        // Multiple values - use Fork
        let mut alt_indices = Vec::new();
        for v in &vals {
            let idx = self.builder.add_constant(v.clone());
            alt_indices.push(idx);
        }

        self.builder
            .emit_u16(Opcode::Fork, alt_indices.len() as u16);
        for idx in alt_indices {
            self.builder.emit_raw(&idx.to_be_bytes());
        }

        Ok(())
    }

    fn compile_pattern_binding_iterative(
        &mut self,
        pattern: MettaValue,
        element_index: usize,
        total_elements: usize,
        state: PatternBindingState,
        _cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            PatternBindingState::Binding => {
                match pattern.inner() {
                    MettaValueInner::Atom(name) if name.starts_with('$') => {
                        let var_name = (*name)[1..].to_string();
                        let slot = self.context.declare_local(var_name)?;
                        if slot <= 255 {
                            self.builder.emit_byte(Opcode::StoreLocal, slot as u8);
                        } else {
                            self.builder.emit_u16(Opcode::StoreLocalWide, slot);
                        }
                    }
                    MettaValueInner::Atom(name) if *name == "_" => {
                        self.builder.emit(Opcode::Pop);
                    }
                    MettaValueInner::SExpr(items) => {
                        // Destructuring pattern
                        let total = items.len();
                        if total > 0 {
                            // Push work for each element in reverse order
                            for (i, item) in (*items).iter().cloned().enumerate().rev() {
                                work_stack.push(CompileWork::CompilePatternBinding {
                                    pattern: item,
                                    element_index: i,
                                    total_elements: total,
                                    state: PatternBindingState::DestructuringElement,
                                    cont_id: 0,
                                });
                            }
                        } else {
                            // Empty pattern - just pop
                            self.builder.emit(Opcode::Pop);
                        }
                    }
                    _ => {
                        // Non-binding pattern - just pop
                        self.builder.emit(Opcode::Pop);
                    }
                }
            }
            PatternBindingState::DestructuringElement => {
                // Emit Dup, GetElement, then recursively bind
                self.builder.emit(Opcode::Dup);
                self.builder
                    .emit_byte(Opcode::GetElement, element_index as u8);

                // If this is the last element, pop the original after binding
                if element_index == total_elements - 1 {
                    work_stack.push(CompileWork::EmitOpcode {
                        opcode: Opcode::Pop,
                        cont_id: 0,
                    });
                }

                work_stack.push(CompileWork::CompilePatternBinding {
                    pattern,
                    element_index: 0,
                    total_elements: 0,
                    state: PatternBindingState::Binding,
                    cont_id: 0,
                });
            }
            PatternBindingState::Done => {}
        }
        Ok(())
    }

    fn compile_match_iterative(
        &mut self,
        space: MettaValue,
        pattern: MettaValue,
        template: MettaValue,
        default: Option<MettaValue>,
        state: MatchState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            MatchState::CompileSpace => {
                work_stack.push(CompileWork::CompileMatch {
                    space: space.clone(),
                    pattern,
                    template,
                    default,
                    state: MatchState::CompilePattern,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: space,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            MatchState::CompilePattern => {
                work_stack.push(CompileWork::CompileMatch {
                    space,
                    pattern: pattern.clone(),
                    template,
                    default,
                    state: MatchState::CompileTemplate,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileQuoted {
                    expr: pattern,
                    cont_id: 0,
                });
            }
            MatchState::CompileTemplate => {
                let next_state = if default.is_some() {
                    MatchState::CompileDefault
                } else {
                    MatchState::EmitMatch
                };
                work_stack.push(CompileWork::CompileMatch {
                    space,
                    pattern,
                    template: template.clone(),
                    default,
                    state: next_state,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileQuoted {
                    expr: template,
                    cont_id: 0,
                });
            }
            MatchState::CompileDefault => {
                if let Some(def) = default {
                    work_stack.push(CompileWork::CompileMatch {
                        space,
                        pattern,
                        template,
                        default: None,
                        state: MatchState::EmitMatch,
                        cont_id,
                    });
                    work_stack.push(CompileWork::CompileExpr {
                        expr: def,
                        in_tail_position: false,
                        cont_id: 0,
                    });
                    // Emit MakeSExpr 4 then EvalMatch
                    self.builder.emit_byte(Opcode::MakeSExpr, 4);
                } else {
                    // Should not reach here
                }
            }
            MatchState::EmitMatch => {
                if default.is_none() {
                    self.builder.emit_byte(Opcode::MakeSExpr, 3);
                }
                self.builder.emit(Opcode::EvalMatch);
            }
            MatchState::Done => {}
        }
        Ok(())
    }

    fn compile_higher_order_iterative(
        &mut self,
        op: HigherOrderOp,
        list: MettaValue,
        state: HigherOrderState,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            HigherOrderState::CompileList => {
                // After list, transition based on operation type
                let next_state = if matches!(op, HigherOrderOp::FoldlAtom { .. }) {
                    // Foldl needs to compile init before template
                    HigherOrderState::CompileFoldlInit
                } else {
                    HigherOrderState::CompileTemplate
                };

                work_stack.push(CompileWork::CompileHigherOrder {
                    op,
                    list: list.clone(),
                    state: next_state,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: list,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            HigherOrderState::CompileFoldlInit => {
                // For foldl: compile init expression, then transition to template
                // Clone init before moving op to avoid borrow conflict
                let init_clone = if let HigherOrderOp::FoldlAtom { ref init, .. } = op {
                    Some(init.clone())
                } else {
                    None
                };

                if let Some(init_expr) = init_clone {
                    work_stack.push(CompileWork::CompileHigherOrder {
                        op,
                        list,
                        state: HigherOrderState::CompileTemplate,
                        cont_id,
                    });
                    work_stack.push(CompileWork::CompileExpr {
                        expr: init_expr,
                        in_tail_position: false,
                        cont_id: 0,
                    });
                }
            }
            HigherOrderState::CompileTemplate => {
                // Template compilation creates new Compiler instance (natural isolation)
                // Use the existing recursive method for this
                match op {
                    HigherOrderOp::MapAtom { var_name, template } => {
                        let chunk_idx = self.compile_template_chunk(&template, &[var_name])?;
                        self.builder.emit_u16(Opcode::MapAtom, chunk_idx);
                    }
                    HigherOrderOp::FilterAtom {
                        var_name,
                        predicate,
                    } => {
                        let chunk_idx = self.compile_template_chunk(&predicate, &[var_name])?;
                        self.builder.emit_u16(Opcode::FilterAtom, chunk_idx);
                    }
                    HigherOrderOp::FoldlAtom {
                        acc_name,
                        item_name,
                        op: op_template,
                        ..
                    } => {
                        // Init was already compiled in CompileFoldlInit state
                        let chunk_idx =
                            self.compile_template_chunk(&op_template, &[acc_name, item_name])?;
                        self.builder.emit_u16(Opcode::FoldlAtom, chunk_idx);
                    }
                }
            }
            HigherOrderState::Done => {}
        }
        Ok(())
    }

    fn compile_catch_iterative(
        &mut self,
        expr: MettaValue,
        default: MettaValue,
        state: CatchState,
        _no_error_jump: Option<JumpLabel>,
        done_jump: Option<JumpLabel>,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            CatchState::CompileExpr => {
                work_stack.push(CompileWork::CompileCatch {
                    expr: expr.clone(),
                    default,
                    state: CatchState::CompileDefault,
                    no_error_jump: None,
                    done_jump: None,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            CatchState::CompileDefault => {
                let new_no_error_jump = self.builder.emit_jump(Opcode::JumpIfError);
                let new_done_jump = self.builder.emit_jump(Opcode::Jump);
                self.builder.patch_jump(new_no_error_jump);
                self.builder.emit(Opcode::Pop);

                work_stack.push(CompileWork::CompileCatch {
                    expr,
                    default: default.clone(),
                    state: CatchState::Done,
                    no_error_jump: Some(new_no_error_jump),
                    done_jump: Some(new_done_jump),
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr: default,
                    in_tail_position: self.in_tail_position,
                    cont_id: 0,
                });
            }
            CatchState::Done => {
                if let Some(label) = done_jump {
                    self.builder.patch_jump(label);
                }
            }
        }
        Ok(())
    }

    fn compile_is_error_iterative(
        &mut self,
        expr: MettaValue,
        state: IsErrorState,
        _not_error_jump: Option<JumpLabel>,
        _done_jump: Option<JumpLabel>,
        cont_id: usize,
        work_stack: &mut Vec<CompileWork>,
    ) -> CompileResult<()> {
        match state {
            IsErrorState::CompileExpr => {
                work_stack.push(CompileWork::CompileIsError {
                    expr: expr.clone(),
                    state: IsErrorState::Done,
                    not_error_jump: None,
                    done_jump: None,
                    cont_id,
                });
                work_stack.push(CompileWork::CompileExpr {
                    expr,
                    in_tail_position: false,
                    cont_id: 0,
                });
            }
            IsErrorState::Done => {
                let not_error_jump = self.builder.emit_jump(Opcode::JumpIfError);
                self.builder.emit(Opcode::PushFalse);
                let done_jump = self.builder.emit_jump(Opcode::Jump);
                self.builder.patch_jump(not_error_jump);
                self.builder.emit(Opcode::PushTrue);
                self.builder.patch_jump(done_jump);
            }
        }
        Ok(())
    }
}
