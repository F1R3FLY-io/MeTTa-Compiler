//! Generic Bytecode Compiler for MeTTa expressions
//!
//! This module provides `GenericCompiler<V, F>` which compiles expressions of any
//! value type implementing `MettaValueTrait` to `GenericBytecodeChunk<V>`.
//!
//! This enables zero-conversion bytecode compilation for both heap-allocated
//! (`MettaValue`) and arena-allocated (`ArenaValue<'static>`) values.

use std::sync::Arc;

use super::context::CompileContext;
use super::error::{CompileError, CompileResult};
use crate::backend::bytecode::chunk::{GenericBytecodeChunk, GenericChunkBuilder};
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Generic bytecode compiler that works with any value type.
///
/// # Type Parameters
///
/// - `V`: The value type (e.g., `MettaValue` or `ArenaValue<'static>`)
/// - `F`: The factory type for constructing values
pub struct GenericCompiler<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// The chunk being built
    pub(crate) builder: GenericChunkBuilder<V, F>,
    /// Compilation context
    pub(crate) context: CompileContext,
    /// Factory for creating values
    pub(crate) factory: F,
    /// Current source line
    current_line: u32,
    /// Whether we're compiling in tail position (for TCO)
    pub(crate) in_tail_position: bool,
}

impl<V, F> GenericCompiler<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Create a new generic compiler
    pub fn new(name: impl Into<String>, factory: F) -> Self {
        let mut builder = GenericChunkBuilder::new(name, factory.clone());
        builder.set_optimize(true);
        Self {
            builder,
            context: CompileContext::new(),
            factory,
            current_line: 1,
            in_tail_position: true,
        }
    }

    /// Create a compiler with existing context (for nested functions)
    pub fn with_context(name: impl Into<String>, context: CompileContext, factory: F) -> Self {
        let mut builder = GenericChunkBuilder::new(name, factory.clone());
        builder.set_optimize(true);
        Self {
            builder,
            context,
            factory,
            current_line: 1,
            in_tail_position: true,
        }
    }

    /// Set the current source line
    pub fn set_line(&mut self, line: u32) {
        self.current_line = line;
        self.builder.set_line(line);
    }

    /// Compile a value expression
    pub fn compile(&mut self, expr: &V) -> CompileResult<()> {
        // Dispatch based on value type using trait methods
        if expr.is_nil() {
            self.builder.emit(Opcode::PushNil);
            return Ok(());
        }

        if expr.is_unit() {
            self.builder.emit(Opcode::PushUnit);
            return Ok(());
        }

        if let Some(b) = expr.as_bool() {
            if b {
                self.builder.emit(Opcode::PushTrue);
            } else {
                self.builder.emit(Opcode::PushFalse);
            }
            return Ok(());
        }

        if let Some(n) = expr.as_long() {
            return self.compile_long(n);
        }

        if let Some(f) = expr.as_float() {
            return self.compile_float(f);
        }

        if let Some(s) = expr.as_string() {
            let val = self.factory.string(s);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushConstant, idx);
            return Ok(());
        }

        if let Some(name) = expr.as_atom() {
            return self.compile_atom(name);
        }

        if let Some(items) = expr.as_sexpr() {
            return self.compile_sexpr(items);
        }

        if expr.is_error() {
            // Compile error as-is (push the error value)
            let idx = self.builder.add_constant(expr.clone());
            self.builder.emit_u16(Opcode::PushConstant, idx);
            return Ok(());
        }

        // Fallback: push as constant
        let idx = self.builder.add_constant(expr.clone());
        self.builder.emit_u16(Opcode::PushConstant, idx);
        Ok(())
    }

    /// Compile a long integer
    fn compile_long(&mut self, n: i64) -> CompileResult<()> {
        if n >= -128 && n <= 127 {
            self.builder.emit_byte(Opcode::PushLongSmall, n as u8);
        } else {
            let val = self.factory.long(n);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushLong, idx);
        }
        Ok(())
    }

    /// Compile a float
    fn compile_float(&mut self, f: f64) -> CompileResult<()> {
        let val = self.factory.float(f);
        let idx = self.builder.add_constant(val);
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
            let val = self.factory.atom(name);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushVariable, idx);
        } else {
            // Regular symbol
            let val = self.factory.atom(name);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushAtom, idx);
        }
        Ok(())
    }

    /// Compile an S-expression
    fn compile_sexpr(&mut self, items: &[V]) -> CompileResult<()> {
        if items.is_empty() {
            self.builder.emit(Opcode::PushEmpty);
            return Ok(());
        }

        // Check if the head is a known operation
        if let Some(head) = items.first() {
            if let Some(op_name) = head.as_atom() {
                // Try to compile as built-in operation
                if let Some(()) = self.try_compile_builtin(op_name, &items[1..])? {
                    return Ok(());
                }

                // Not a builtin - check if it's a potential function call
                if !op_name.starts_with('$') && !op_name.starts_with('&') {
                    return self.compile_call(op_name, &items[1..]);
                }
            }
        }

        // Fallback: compile as generic S-expression data
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
    fn compile_call(&mut self, head: &str, args: &[V]) -> CompileResult<()> {
        let arity = args.len();

        // Compile arguments (left-to-right) - not in tail position
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        for arg in args {
            self.compile(arg)?;
        }
        self.in_tail_position = saved_tail;

        // Add head symbol to constant pool
        let head_val = self.factory.atom(head);
        let head_index = self.builder.add_constant(head_val);

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

    /// Try to compile a built-in operation
    fn try_compile_builtin(&mut self, op: &str, args: &[V]) -> CompileResult<Option<()>> {
        match op {
            // Arithmetic operations
            "+" => {
                self.check_arity("+", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Add);
                Ok(Some(()))
            }
            "-" => {
                self.check_arity("-", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Sub);
                Ok(Some(()))
            }
            "*" => {
                self.check_arity("*", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Mul);
                Ok(Some(()))
            }
            "/" => {
                self.check_arity("/", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Div);
                Ok(Some(()))
            }
            "%" | "mod" => {
                self.check_arity("%", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Mod);
                Ok(Some(()))
            }
            "pow" | "pow-math" => {
                self.check_arity("pow", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Pow);
                Ok(Some(()))
            }
            "abs" | "abs-math" => {
                self.check_arity("abs", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Abs);
                Ok(Some(()))
            }
            "neg" => {
                self.check_arity("neg", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Neg);
                Ok(Some(()))
            }
            "floor-div" => {
                self.check_arity("floor-div", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::FloorDiv);
                Ok(Some(()))
            }

            // Comparison operations
            "<" => {
                self.check_arity("<", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Lt);
                Ok(Some(()))
            }
            "<=" => {
                self.check_arity("<=", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Le);
                Ok(Some(()))
            }
            ">" => {
                self.check_arity(">", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Gt);
                Ok(Some(()))
            }
            ">=" => {
                self.check_arity(">=", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Ge);
                Ok(Some(()))
            }
            "==" => {
                self.check_arity("==", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Eq);
                Ok(Some(()))
            }
            "!=" => {
                self.check_arity("!=", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Ne);
                Ok(Some(()))
            }

            // Boolean operations
            "and" => {
                self.check_arity("and", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::And);
                Ok(Some(()))
            }
            "or" => {
                self.check_arity("or", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Or);
                Ok(Some(()))
            }
            "not" => {
                self.check_arity("not", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Not);
                Ok(Some(()))
            }
            "xor" => {
                self.check_arity("xor", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Xor);
                Ok(Some(()))
            }

            // Control flow
            "if" => {
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

            // Force evaluation
            "!" => {
                self.check_arity("!", args.len(), 1)?;
                self.compile(&args[0])?;
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
            "get-metatype" => {
                self.check_arity("get-metatype", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetMetaType);
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
                self.compile(&args[0])?;
                self.compile(&args[1])?;
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

            // Chain operation
            "chain" => {
                self.compile_chain(args)?;
                Ok(Some(()))
            }

            // Error handling
            "error" => {
                self.check_arity("error", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit_byte(Opcode::MakeSExpr, 3);
                Ok(Some(()))
            }
            "is-error" => {
                self.check_arity("is-error", args.len(), 1)?;
                self.compile(&args[0])?;
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
                let done = self.builder.emit_jump(Opcode::Jump);
                self.builder.patch_jump(no_error);
                self.builder.emit(Opcode::Pop);
                self.compile(&args[1])?;
                self.builder.patch_jump(done);
                Ok(Some(()))
            }

            // Space operations
            "new-space" => {
                self.check_arity("new-space", args.len(), 0)?;
                self.builder.emit(Opcode::EvalNew);
                Ok(Some(()))
            }
            "add-atom" => {
                self.check_arity("add-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
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

            // State operations
            "new-state" => {
                self.check_arity("new-state", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::NewState);
                Ok(Some(()))
            }
            "get-state" => {
                self.check_arity("get-state", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetState);
                Ok(Some(()))
            }
            "change-state!" => {
                self.check_arity("change-state!", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::ChangeState);
                Ok(Some(()))
            }

            // Rule definition
            "=" => {
                self.check_arity("=", args.len(), 2)?;
                let eq_val = self.factory.atom("=");
                let idx = self.builder.add_constant(eq_val);
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
                let print_val = self.factory.atom("println!");
                let idx = self.builder.add_constant(print_val);
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

            // Math operations
            "sqrt-math" => {
                self.check_arity("sqrt-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Sqrt);
                Ok(Some(()))
            }
            "log-math" => {
                self.check_arity("log-math", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
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

            // Expression manipulation
            "index-atom" => {
                self.check_arity("index-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
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

    /// Compile an if expression
    fn compile_if(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() < 2 || args.len() > 3 {
            return Err(CompileError::InvalidArityRange {
                op: "if".to_string(),
                min: 2,
                max: 3,
                got: args.len(),
            });
        }

        // Compile condition (not in tail position)
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(&args[0])?;
        self.in_tail_position = saved_tail;

        // Jump to else if false
        let else_jump = self.builder.emit_jump(Opcode::JumpIfFalse);

        // Compile then branch (in tail position if we're in tail position)
        self.compile(&args[1])?;
        let end_jump = self.builder.emit_jump(Opcode::Jump);

        // Else branch
        self.builder.patch_jump(else_jump);
        if args.len() == 3 {
            self.compile(&args[2])?;
        } else {
            self.builder.emit(Opcode::PushNil);
        }

        self.builder.patch_jump(end_jump);
        Ok(())
    }

    /// Compile a let expression
    fn compile_let(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() < 3 {
            return Err(CompileError::InvalidArityRange {
                op: "let".to_string(),
                min: 3,
                max: usize::MAX,
                got: args.len(),
            });
        }

        // (let pattern value body)
        let pattern = &args[0];
        let value = &args[1];
        let body = &args[2];

        // Compile the value (not in tail position)
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(value)?;
        self.in_tail_position = saved_tail;

        // Bind the pattern
        self.context.begin_scope();
        self.bind_pattern(pattern)?;

        // Compile the body (in original tail position)
        self.compile(body)?;

        // End scope
        let local_count = self.context.end_scope();
        for _ in 0..local_count {
            self.builder.emit(Opcode::Pop);
        }

        Ok(())
    }

    /// Compile a let* expression
    fn compile_let_star(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() < 2 {
            return Err(CompileError::InvalidArityRange {
                op: "let*".to_string(),
                min: 2,
                max: usize::MAX,
                got: args.len(),
            });
        }

        // (let* ((var1 val1) (var2 val2) ...) body)
        let bindings = &args[0];
        let body = &args[1];

        self.context.begin_scope();

        // Process bindings
        if let Some(items) = bindings.as_sexpr() {
            let saved_tail = self.in_tail_position;
            self.in_tail_position = false;

            for binding in items {
                if let Some(pair) = binding.as_sexpr() {
                    if pair.len() == 2 {
                        // Compile value
                        self.compile(&pair[1])?;
                        // Bind pattern
                        self.bind_pattern(&pair[0])?;
                    }
                }
            }

            self.in_tail_position = saved_tail;
        }

        // Compile body
        self.compile(body)?;

        // End scope
        let local_count = self.context.end_scope();
        for _ in 0..local_count {
            self.builder.emit(Opcode::Pop);
        }

        Ok(())
    }

    /// Bind a pattern to the value on stack top
    fn bind_pattern(&mut self, pattern: &V) -> CompileResult<()> {
        if let Some(name) = pattern.as_atom() {
            if let Some(var_name) = name.strip_prefix('$') {
                // Variable binding
                self.context.declare_local(var_name.to_string())?;
                return Ok(());
            }
        }
        // Non-variable pattern - just pop for now
        self.builder.emit(Opcode::Pop);
        Ok(())
    }

    /// Compile a quoted expression (no evaluation)
    fn compile_quoted(&mut self, expr: &V) -> CompileResult<()> {
        // Push the value as-is without evaluation
        let idx = self.builder.add_constant(expr.clone());
        self.builder.emit_u16(Opcode::PushConstant, idx);
        Ok(())
    }

    /// Compile superpose (nondeterminism)
    fn compile_superpose(&mut self, args: &[V]) -> CompileResult<()> {
        self.check_arity("superpose", args.len(), 1)?;

        let list = &args[0];
        if let Some(items) = list.as_sexpr() {
            if items.is_empty() {
                // Empty superpose = fail
                self.builder.emit(Opcode::Fail);
                return Ok(());
            }

            if items.len() == 1 {
                // Single item - just compile it
                return self.compile(&items[0]);
            }

            // Multiple items - create choice point
            // Compile sub-chunks for each alternative
            let mut sub_indices = Vec::with_capacity(items.len());
            for item in items {
                let mut sub_compiler = GenericCompiler::with_context(
                    "superpose_alt",
                    self.context.clone(),
                    self.factory.clone(),
                );
                sub_compiler.compile(item)?;
                sub_compiler.builder.emit(Opcode::Return);
                let sub_chunk = sub_compiler.builder.build();
                let idx = self.builder.add_chunk_constant(sub_chunk);
                sub_indices.push(idx);
            }

            // Emit Fork with alternatives
            let count = sub_indices.len();
            self.builder.emit_byte(Opcode::Fork, count as u8);
            for idx in sub_indices {
                self.builder.emit_raw(&(idx as u16).to_le_bytes());
            }
        } else {
            // Not a list - compile the list expression and it will be dynamically superposed
            self.compile(list)?;
            // Dynamic superpose not yet supported in generic VM, just return the value
        }

        Ok(())
    }

    /// Compile chain operation
    fn compile_chain(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() != 3 {
            return Err(CompileError::InvalidArity {
                op: "chain".to_string(),
                expected: 3,
                got: args.len(),
            });
        }

        let expr = &args[0];
        let var = &args[1];
        let body = &args[2];

        // Compile expression
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(expr)?;
        self.in_tail_position = saved_tail;

        // Bind result to variable
        self.context.begin_scope();
        self.bind_pattern(var)?;

        // Compile body
        self.compile(body)?;

        // End scope
        let local_count = self.context.end_scope();
        for _ in 0..local_count {
            self.builder.emit(Opcode::Pop);
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

    /// Finish compilation and return the chunk
    pub fn finish(mut self) -> GenericBytecodeChunk<V> {
        let offset = self.builder.current_offset();
        if offset == 0 || !self.ends_with_terminator() {
            self.builder.emit(Opcode::Return);
        }

        self.builder.set_local_count(self.context.local_count());
        self.builder.set_upvalue_count(self.context.upvalue_count());

        self.builder.build()
    }

    /// Check if the last emitted instruction is a terminator
    fn ends_with_terminator(&self) -> bool {
        false // Safe default
    }

    /// Finish and wrap in Arc
    pub fn finish_arc(self) -> Arc<GenericBytecodeChunk<V>> {
        Arc::new(self.finish())
    }
}

// =============================================================================
// Public API Functions
// =============================================================================

/// Compile a generic value to bytecode
pub fn compile_generic<V, F>(
    name: &str,
    expr: &V,
    factory: F,
) -> CompileResult<GenericBytecodeChunk<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let mut compiler = GenericCompiler::new(name, factory);
    compiler.compile(expr)?;
    Ok(compiler.finish())
}

/// Compile a generic value to bytecode wrapped in Arc
pub fn compile_generic_arc<V, F>(
    name: &str,
    expr: &V,
    factory: F,
) -> CompileResult<Arc<GenericBytecodeChunk<V>>>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    Ok(Arc::new(compile_generic(name, expr, factory)?))
}

// =============================================================================
// Arena-Specific Entry Points
// =============================================================================

use crate::backend::models::{ArenaValue, ArenaValueFactory};
use crate::backend::eval::trampoline::get_static_factory;

/// Compile an ArenaValue expression to bytecode (zero-conversion).
///
/// Uses the static arena factory from the thread-local context.
pub fn compile_arena_bytecode(
    name: &str,
    expr: &ArenaValue<'static>,
) -> CompileResult<GenericBytecodeChunk<ArenaValue<'static>>> {
    let factory = get_static_factory();
    compile_generic(name, expr, factory)
}

/// Compile an ArenaValue expression to bytecode wrapped in Arc (zero-conversion).
pub fn compile_arena_bytecode_arc(
    name: &str,
    expr: &ArenaValue<'static>,
) -> CompileResult<Arc<GenericBytecodeChunk<ArenaValue<'static>>>> {
    let factory = get_static_factory();
    compile_generic_arc(name, expr, factory)
}

/// Type alias for arena bytecode compiler
pub type ArenaCompiler = GenericCompiler<ArenaValue<'static>, ArenaValueFactory<'static>>;
