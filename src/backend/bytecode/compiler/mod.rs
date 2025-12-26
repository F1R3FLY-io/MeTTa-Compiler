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
mod higher_order;
mod patterns;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::backend::models::MettaValue;
use super::chunk::{BytecodeChunk, ChunkBuilder};
use super::opcodes::Opcode;

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
    pub fn compile(&mut self, expr: &MettaValue) -> CompileResult<()> {
        match expr {
            // Literals
            MettaValue::Nil => {
                self.builder.emit(Opcode::PushNil);
            }
            MettaValue::Unit => {
                self.builder.emit(Opcode::PushUnit);
            }
            MettaValue::Bool(true) => {
                self.builder.emit(Opcode::PushTrue);
            }
            MettaValue::Bool(false) => {
                self.builder.emit(Opcode::PushFalse);
            }
            MettaValue::Long(n) => {
                self.compile_long(*n)?;
            }
            MettaValue::Float(f) => {
                self.compile_float(*f)?;
            }
            MettaValue::String(s) => {
                let idx = self.builder.add_constant(MettaValue::String(s.clone()));
                self.builder.emit_u16(Opcode::PushString, idx);
            }

            // Atoms (symbols and variables)
            MettaValue::Atom(name) => {
                self.compile_atom(name)?;
            }

            // S-expressions
            MettaValue::SExpr(items) => {
                self.compile_sexpr(items)?;
            }

            // Type
            MettaValue::Type(t) => {
                let idx = self.builder.add_constant(MettaValue::Type(t.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }

            // Conjunction (multiple values)
            MettaValue::Conjunction(values) => {
                self.compile_conjunction(values)?;
            }

            // Error
            MettaValue::Error(msg, details) => {
                let idx = self.builder.add_constant(MettaValue::Error(msg.clone(), details.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }

            // State cell reference
            MettaValue::State(id) => {
                let idx = self.builder.add_constant(MettaValue::State(*id));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }

            // Memo handle
            MettaValue::Memo(handle) => {
                let idx = self.builder.add_constant(MettaValue::Memo(handle.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }

            // Empty - used in nondeterministic evaluation
            MettaValue::Empty => {
                self.builder.emit(Opcode::PushNil); // Empty is similar to Nil
            }
        }
        Ok(())
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
            let idx = self.builder.add_constant(MettaValue::Atom(name.to_string()));
            self.builder.emit_u16(Opcode::PushVariable, idx);
        } else {
            // Regular symbol
            let idx = self.builder.add_constant(MettaValue::Atom(name.to_string()));
            self.builder.emit_u16(Opcode::PushAtom, idx);
        }
        Ok(())
    }

    /// Compile an S-expression
    fn compile_sexpr(&mut self, items: &[MettaValue]) -> CompileResult<()> {
        if items.is_empty() {
            self.builder.emit(Opcode::PushEmpty);
            return Ok(());
        }

        // Check if the head is a known operation
        if let Some(head) = items.first() {
            if let MettaValue::Atom(op_name) = head {
                // Try to compile as built-in operation
                if let Some(()) = self.try_compile_builtin(op_name, &items[1..])? {
                    return Ok(());
                }

                // Not a builtin - check if it's a potential function call
                // Function calls are atoms that don't start with $ (variable) or & (grounded ref)
                if !op_name.starts_with('$') && !op_name.starts_with('&') {
                    return self.compile_call(op_name, &items[1..]);
                }
            }
        }

        // Fallback: compile as generic S-expression data
        // This handles cases like ($var args...) or other non-callable heads
        for item in items {
            self.compile(item)?;
        }

        let arity = items.len();
        if arity <= 255 {
            self.builder.emit_byte(Opcode::MakeSExpr, arity as u8);
        } else {
            self.builder.emit_u16(Opcode::MakeSExprLarge, arity as u16);
        }

        Ok(())
    }

    /// Compile a function call to a user-defined rule
    ///
    /// Emits Call (or TailCall if in tail position) with head symbol index and arity.
    /// The VM will dispatch to MORK for rule lookup and execution.
    fn compile_call(&mut self, head: &str, args: &[MettaValue]) -> CompileResult<()> {
        let arity = args.len();

        // Compile arguments (left-to-right) - not in tail position
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        for arg in args {
            self.compile(arg)?;
        }
        self.in_tail_position = saved_tail;

        // Add head symbol to constant pool
        let head_index = self.builder.add_constant(MettaValue::Atom(head.to_string()));

        // Emit Call or TailCall based on position
        // Note: arity must fit in u8 (255 max)
        if arity > 255 {
            return Err(CompileError::InvalidArityRange {
                op: head.to_string(),
                min: 0,
                max: 255,
                got: arity,
            });
        }

        if self.in_tail_position {
            self.builder.emit_u16(Opcode::TailCall, head_index);
        } else {
            self.builder.emit_u16(Opcode::Call, head_index);
        }
        self.builder.emit_raw(&[arity as u8]);

        Ok(())
    }

    /// Try to compile a built-in operation, returns Some(()) if handled
    fn try_compile_builtin(&mut self, op: &str, args: &[MettaValue]) -> CompileResult<Option<()>> {
        match op {
            // Arithmetic operations (with constant folding)
            "+" => {
                self.check_arity("+", args.len(), 2)?;
                // Try constant folding
                if let Some(folded) = self.try_fold_binary_arith("+", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Add);
                Ok(Some(()))
            }
            "-" => {
                self.check_arity("-", args.len(), 2)?;
                if let Some(folded) = self.try_fold_binary_arith("-", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Sub);
                Ok(Some(()))
            }
            "*" => {
                self.check_arity("*", args.len(), 2)?;
                if let Some(folded) = self.try_fold_binary_arith("*", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                // Special case: x * 0 = 0, 0 * x = 0 (even if x is not constant)
                if matches!(&args[0], MettaValue::Long(0)) || matches!(&args[1], MettaValue::Long(0)) {
                    self.builder.emit_byte(Opcode::PushLongSmall, 0);
                    return Ok(Some(()));
                }
                // Special case: x * 1 = x, 1 * x = x
                if matches!(&args[0], MettaValue::Long(1)) {
                    return self.compile(&args[1]).map(Some);
                }
                if matches!(&args[1], MettaValue::Long(1)) {
                    return self.compile(&args[0]).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Mul);
                Ok(Some(()))
            }
            "/" => {
                self.check_arity("/", args.len(), 2)?;
                if let Some(folded) = self.try_fold_binary_arith("/", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                // Special case: x / 1 = x
                if matches!(&args[1], MettaValue::Long(1)) {
                    return self.compile(&args[0]).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Div);
                Ok(Some(()))
            }
            "%" | "mod" => {
                self.check_arity("%", args.len(), 2)?;
                if let Some(folded) = self.try_fold_binary_arith(op, &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Mod);
                Ok(Some(()))
            }
            "pow" | "pow-math" => {
                self.check_arity("pow", args.len(), 2)?;
                if let Some(folded) = self.try_fold_binary_arith("pow", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                // Special case: x^0 = 1
                if matches!(&args[1], MettaValue::Long(0)) {
                    self.builder.emit_byte(Opcode::PushLongSmall, 1);
                    return Ok(Some(()));
                }
                // Special case: x^1 = x
                if matches!(&args[1], MettaValue::Long(1)) {
                    return self.compile(&args[0]).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Pow);
                Ok(Some(()))
            }
            "abs" | "abs-math" => {
                self.check_arity("abs", args.len(), 1)?;
                if let Some(folded) = self.try_fold_unary_arith("abs", &args[0]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Abs);
                Ok(Some(()))
            }
            "neg" => {
                self.check_arity("neg", args.len(), 1)?;
                if let Some(folded) = self.try_fold_unary_arith("neg", &args[0]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Neg);
                Ok(Some(()))
            }
            "floor-div" => {
                self.check_arity("floor-div", args.len(), 2)?;
                if let Some(folded) =
                    self.try_fold_binary_arith("floor-div", &args[0], &args[1])
                {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::FloorDiv);
                Ok(Some(()))
            }

            // Comparison operations (with constant folding)
            "<" => {
                self.check_arity("<", args.len(), 2)?;
                if let Some(folded) = self.try_fold_comparison("<", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Lt);
                Ok(Some(()))
            }
            "<=" => {
                self.check_arity("<=", args.len(), 2)?;
                if let Some(folded) = self.try_fold_comparison("<=", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Le);
                Ok(Some(()))
            }
            ">" => {
                self.check_arity(">", args.len(), 2)?;
                if let Some(folded) = self.try_fold_comparison(">", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Gt);
                Ok(Some(()))
            }
            ">=" => {
                self.check_arity(">=", args.len(), 2)?;
                if let Some(folded) = self.try_fold_comparison(">=", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Ge);
                Ok(Some(()))
            }
            "==" => {
                self.check_arity("==", args.len(), 2)?;
                if let Some(folded) = self.try_fold_comparison("==", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Eq);
                Ok(Some(()))
            }
            "!=" => {
                self.check_arity("!=", args.len(), 2)?;
                if let Some(folded) = self.try_fold_comparison("!=", &args[0], &args[1]) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Ne);
                Ok(Some(()))
            }

            // Boolean operations (with constant folding)
            "and" => {
                self.check_arity("and", args.len(), 2)?;
                if let Some(folded) = self.try_fold_boolean("and", args) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::And);
                Ok(Some(()))
            }
            "or" => {
                self.check_arity("or", args.len(), 2)?;
                if let Some(folded) = self.try_fold_boolean("or", args) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Or);
                Ok(Some(()))
            }
            "not" => {
                self.check_arity("not", args.len(), 1)?;
                if let Some(folded) = self.try_fold_boolean("not", args) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Not);
                Ok(Some(()))
            }
            "xor" => {
                self.check_arity("xor", args.len(), 2)?;
                if let Some(folded) = self.try_fold_boolean("xor", args) {
                    return self.compile(&folded).map(Some);
                }
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Xor);
                Ok(Some(()))
            }

            // Control flow (with constant condition folding)
            "if" => {
                // Check for constant condition (with recursive evaluation)
                if args.len() >= 3 {
                    // Try to evaluate the condition to a constant
                    if let Some(cond_val) = self.try_eval_constant(&args[0]) {
                        if let MettaValue::Bool(cond) = cond_val {
                            // Compile only the appropriate branch (recursively evaluate)
                            if cond {
                                return self.compile(&args[1]).map(Some);
                            } else {
                                return self.compile(&args[2]).map(Some);
                            }
                        }
                    }
                }
                self.compile_if(args)?;
                Ok(Some(()))
            }

            // Binding forms
            "let" => {
                self.compile_let(args)?;
                Ok(Some(()))
            }
            "let*" => {
                self.compile_let_star(args)?;
                Ok(Some(()))
            }

            // Quote and eval
            "quote" => {
                self.check_arity("quote", args.len(), 1)?;
                self.compile_quoted(&args[0])?;
                Ok(Some(()))
            }
            "eval" => {
                self.check_arity("eval", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::EvalEval);
                Ok(Some(()))
            }

            // Force evaluation (!)
            "!" => {
                self.check_arity("!", args.len(), 1)?;
                self.compile(&args[0])?;
                // The VM will evaluate the result
                Ok(Some(()))
            }

            // Type operations
            "get-type" => {
                self.check_arity("get-type", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetType);
                Ok(Some(()))
            }
            "check-type" => {
                self.check_arity("check-type", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::CheckType);
                Ok(Some(()))
            }

            // Nondeterminism
            "superpose" => {
                self.compile_superpose(args)?;
                Ok(Some(()))
            }
            "collapse" => {
                self.check_arity("collapse", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::EvalCollapse);
                Ok(Some(()))
            }

            // List operations
            "car-atom" => {
                self.check_arity("car-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetHead);
                Ok(Some(()))
            }
            "cdr-atom" => {
                self.check_arity("cdr-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetTail);
                Ok(Some(()))
            }
            "cons-atom" => {
                self.check_arity("cons-atom", args.len(), 2)?;
                self.compile(&args[0])?;  // head
                self.compile(&args[1])?;  // tail
                // Prepend head to tail S-expression (matches tree-visitor semantics)
                self.builder.emit(Opcode::ConsAtom);
                Ok(Some(()))
            }
            "size-atom" => {
                self.check_arity("size-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetArity);
                Ok(Some(()))
            }
            "empty" => {
                // MeTTa semantics: (empty) returns NO results, not ()
                // This is equivalent to a failing nondeterministic branch
                self.check_arity("empty", args.len(), 0)?;
                self.builder.emit(Opcode::Fail);
                Ok(Some(()))
            }
            "decons-atom" => {
                self.check_arity("decons-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::DeconAtom);
                Ok(Some(()))
            }
            "repr" => {
                self.check_arity("repr", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Repr);
                Ok(Some(()))
            }
            "get-metatype" => {
                self.check_arity("get-metatype", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetMetaType);
                Ok(Some(()))
            }

            // Higher-order list operations
            "map-atom" => {
                self.compile_map_atom(args)?;
                Ok(Some(()))
            }
            "filter-atom" => {
                self.compile_filter_atom(args)?;
                Ok(Some(()))
            }
            "foldl-atom" => {
                self.compile_foldl_atom(args)?;
                Ok(Some(()))
            }

            // Chain operation (sequence/binding)
            "chain" => {
                self.compile_chain(args)?;
                Ok(Some(()))
            }

            // Pattern matching
            "match" => {
                self.compile_match(args)?;
                Ok(Some(()))
            }
            "unify" => {
                self.compile_unify(args)?;
                Ok(Some(()))
            }
            "case" => {
                self.compile_case(args)?;
                Ok(Some(()))
            }

            // Error handling
            "error" => {
                self.check_arity("error", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                // Build error value
                self.builder.emit_byte(Opcode::MakeSExpr, 3);
                Ok(Some(()))
            }
            "is-error" => {
                self.check_arity("is-error", args.len(), 1)?;
                self.compile(&args[0])?;
                // Jump if error
                let not_error = self.builder.emit_jump(Opcode::JumpIfError);
                self.builder.emit(Opcode::PushFalse);
                let done = self.builder.emit_jump(Opcode::Jump);
                self.builder.patch_jump(not_error);
                self.builder.emit(Opcode::PushTrue);
                self.builder.patch_jump(done);
                Ok(Some(()))
            }
            "catch" => {
                self.check_arity("catch", args.len(), 2)?;
                self.compile(&args[0])?;
                let no_error = self.builder.emit_jump(Opcode::JumpIfError);
                // No error - keep the result
                let done = self.builder.emit_jump(Opcode::Jump);
                self.builder.patch_jump(no_error);
                // Error - pop error and evaluate default
                self.builder.emit(Opcode::Pop);
                self.compile(&args[1])?;
                self.builder.patch_jump(done);
                Ok(Some(()))
            }

            // Space operations - compile as special form opcodes
            "new-space" => {
                self.check_arity("new-space", args.len(), 0)?;
                self.builder.emit(Opcode::EvalNew);
                Ok(Some(()))
            }
            "add-atom" => {
                self.check_arity("add-atom", args.len(), 2)?;
                self.compile(&args[0])?; // space
                self.compile(&args[1])?; // atom
                self.builder.emit(Opcode::SpaceAdd);
                Ok(Some(()))
            }
            "remove-atom" => {
                self.check_arity("remove-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::SpaceRemove);
                Ok(Some(()))
            }
            "get-atoms" => {
                self.check_arity("get-atoms", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::SpaceGetAtoms);
                Ok(Some(()))
            }

            // State operations - emit dedicated opcodes for VM/JIT execution
            "new-state" => {
                self.check_arity("new-state", args.len(), 1)?;
                // Compile initial value, then emit NewState opcode
                self.compile(&args[0])?;
                self.builder.emit(Opcode::NewState);
                Ok(Some(()))
            }
            "get-state" => {
                self.check_arity("get-state", args.len(), 1)?;
                // Compile state reference, then emit GetState opcode
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetState);
                Ok(Some(()))
            }
            "change-state!" => {
                self.check_arity("change-state!", args.len(), 2)?;
                // Compile state reference and new value, then emit ChangeState opcode
                // Stack order: [state_ref, new_value] for ChangeState
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::ChangeState);
                Ok(Some(()))
            }

            // Rule definition - compile as data
            "=" => {
                self.check_arity("=", args.len(), 2)?;
                // Compile as literal S-expression for rule definition
                let idx = self.builder.add_constant(MettaValue::Atom("=".to_string()));
                self.builder.emit_u16(Opcode::PushAtom, idx);
                self.compile_quoted(&args[0])?;
                self.compile_quoted(&args[1])?;
                self.builder.emit_byte(Opcode::MakeSExpr, 3);
                Ok(Some(()))
            }

            // I/O operations
            "println!" => {
                self.check_arity("println!", args.len(), 1)?;
                self.compile(&args[0])?;
                // For now, compile as S-expression to be handled by VM
                let idx = self.builder.add_constant(MettaValue::Atom("println!".to_string()));
                self.builder.emit_u16(Opcode::PushAtom, idx);
                self.builder.emit(Opcode::Swap);
                self.builder.emit_byte(Opcode::MakeSExpr, 2);
                Ok(Some(()))
            }
            "trace!" => {
                self.check_arity("trace!", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Trace);
                Ok(Some(()))
            }

            // nop
            "nop" => {
                self.check_arity("nop", args.len(), 0)?;
                self.builder.emit(Opcode::PushUnit);
                Ok(Some(()))
            }

            // Math operations (PR #62)
            "sqrt-math" => {
                self.check_arity("sqrt-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Sqrt);
                Ok(Some(()))
            }
            "log-math" => {
                self.check_arity("log-math", args.len(), 2)?;
                self.compile(&args[0])?;  // base
                self.compile(&args[1])?;  // value
                self.builder.emit(Opcode::Log);
                Ok(Some(()))
            }
            "trunc-math" => {
                self.check_arity("trunc-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Trunc);
                Ok(Some(()))
            }
            "ceil-math" => {
                self.check_arity("ceil-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Ceil);
                Ok(Some(()))
            }
            "floor-math" => {
                self.check_arity("floor-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::FloorMath);
                Ok(Some(()))
            }
            "round-math" => {
                self.check_arity("round-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Round);
                Ok(Some(()))
            }
            "sin-math" => {
                self.check_arity("sin-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Sin);
                Ok(Some(()))
            }
            "cos-math" => {
                self.check_arity("cos-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Cos);
                Ok(Some(()))
            }
            "tan-math" => {
                self.check_arity("tan-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Tan);
                Ok(Some(()))
            }
            "asin-math" => {
                self.check_arity("asin-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Asin);
                Ok(Some(()))
            }
            "acos-math" => {
                self.check_arity("acos-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Acos);
                Ok(Some(()))
            }
            "atan-math" => {
                self.check_arity("atan-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Atan);
                Ok(Some(()))
            }
            "isnan-math" => {
                self.check_arity("isnan-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::IsNan);
                Ok(Some(()))
            }
            "isinf-math" => {
                self.check_arity("isinf-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::IsInf);
                Ok(Some(()))
            }

            // Expression manipulation operations (PR #63)
            "index-atom" => {
                self.check_arity("index-atom", args.len(), 2)?;
                self.compile(&args[0])?;  // expression
                self.compile(&args[1])?;  // index
                self.builder.emit(Opcode::IndexAtom);
                Ok(Some(()))
            }
            "min-atom" => {
                self.check_arity("min-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::MinAtom);
                Ok(Some(()))
            }
            "max-atom" => {
                self.check_arity("max-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::MaxAtom);
                Ok(Some(()))
            }

            // Not a built-in
            _ => Ok(None),
        }
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
    pub(crate) fn check_arity_range(&self, op: &str, got: usize, min: usize, max: usize) -> CompileResult<()> {
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
    fn try_fold_binary_arith(&self, op: &str, a: &MettaValue, b: &MettaValue) -> Option<MettaValue> {
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
