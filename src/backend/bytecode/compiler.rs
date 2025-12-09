//! Bytecode compiler for MeTTa expressions
//!
//! This module compiles MettaValue expressions to bytecode chunks.
//! The compiler handles:
//! - Literals (numbers, booleans, strings, etc.)
//! - Symbols and variables
//! - S-expressions (recursive compilation)
//! - Grounded operations (+, -, *, /, comparisons, etc.)
//! - Special forms (if, let, quote, etc.)

use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::models::MettaValue;
use super::chunk::{BytecodeChunk, ChunkBuilder};
use super::opcodes::Opcode;

/// Compiler error types
#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    /// Unknown operation encountered
    UnknownOperation(String),
    /// Invalid arity for operation
    InvalidArity { op: String, expected: usize, got: usize },
    /// Invalid arity range for operation
    InvalidArityRange { op: String, min: usize, max: usize, got: usize },
    /// Too many constants in chunk
    TooManyConstants,
    /// Too many locals in scope
    TooManyLocals,
    /// Invalid expression structure
    InvalidExpression(String),
    /// Variable not found
    VariableNotFound(String),
    /// Nested function depth exceeded
    NestingTooDeep,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOperation(op) => write!(f, "Unknown operation: {}", op),
            Self::InvalidArity { op, expected, got } => {
                write!(f, "Invalid arity for {}: expected {}, got {}", op, expected, got)
            }
            Self::InvalidArityRange { op, min, max, got } => {
                write!(f, "Invalid arity for {}: expected {}-{}, got {}", op, min, max, got)
            }
            Self::TooManyConstants => write!(f, "Too many constants (max 65535)"),
            Self::TooManyLocals => write!(f, "Too many local variables (max 65535)"),
            Self::InvalidExpression(msg) => write!(f, "Invalid expression: {}", msg),
            Self::VariableNotFound(name) => write!(f, "Variable not found: {}", name),
            Self::NestingTooDeep => write!(f, "Function nesting too deep"),
        }
    }
}

impl std::error::Error for CompileError {}

/// Result type for compilation
pub type CompileResult<T> = Result<T, CompileError>;

/// Compilation context for tracking local variables and scopes
#[derive(Debug, Clone)]
pub struct CompileContext {
    /// Local variable names to slot indices
    locals: HashMap<String, u16>,
    /// Stack of scope depths for local variables
    scope_depths: Vec<u16>,
    /// Current scope depth
    current_scope: u16,
    /// Next available local slot
    next_local: u16,
    /// Parent context for nested functions
    parent: Option<Box<CompileContext>>,
    /// Captured variables (upvalues)
    upvalues: Vec<Upvalue>,
}

/// Upvalue reference
#[derive(Debug, Clone)]
pub struct Upvalue {
    /// Index in parent's locals or upvalues
    pub index: u16,
    /// True if capturing from parent's locals, false if from parent's upvalues
    pub is_local: bool,
}

impl Default for CompileContext {
    fn default() -> Self {
        Self::new()
    }
}

impl CompileContext {
    /// Create a new root context
    pub fn new() -> Self {
        Self {
            locals: HashMap::new(),
            scope_depths: Vec::new(),
            current_scope: 0,
            next_local: 0,
            parent: None,
            upvalues: Vec::new(),
        }
    }

    /// Create a child context for nested functions
    pub fn child(parent: CompileContext) -> Self {
        Self {
            locals: HashMap::new(),
            scope_depths: Vec::new(),
            current_scope: 0,
            next_local: 0,
            parent: Some(Box::new(parent)),
            upvalues: Vec::new(),
        }
    }

    /// Begin a new scope
    pub fn begin_scope(&mut self) {
        self.current_scope += 1;
    }

    /// End current scope, returns number of locals to pop
    pub fn end_scope(&mut self) -> u16 {
        let mut count = 0;
        let scope = self.current_scope;

        // Remove locals from this scope
        self.locals.retain(|_, slot| {
            if self.scope_depths.get(*slot as usize).copied() == Some(scope) {
                count += 1;
                false
            } else {
                true
            }
        });

        // Trim scope_depths
        while self.scope_depths.last().copied() == Some(scope) {
            self.scope_depths.pop();
        }

        self.current_scope -= 1;
        count
    }

    /// Declare a local variable, returns its slot index
    pub fn declare_local(&mut self, name: String) -> CompileResult<u16> {
        if self.next_local >= u16::MAX {
            return Err(CompileError::TooManyLocals);
        }

        let slot = self.next_local;
        self.next_local += 1;
        self.locals.insert(name, slot);
        self.scope_depths.push(self.current_scope);
        Ok(slot)
    }

    /// Resolve a local variable, returns slot index if found
    pub fn resolve_local(&self, name: &str) -> Option<u16> {
        self.locals.get(name).copied()
    }

    /// Resolve an upvalue (captured variable)
    pub fn resolve_upvalue(&mut self, name: &str) -> Option<u16> {
        // Check parent's locals first
        if let Some(parent) = &self.parent {
            if let Some(local_idx) = parent.resolve_local(name) {
                // Add as upvalue capturing from parent's local
                return Some(self.add_upvalue(local_idx, true));
            }
        }

        // Check parent's upvalues
        if let Some(parent) = &mut self.parent {
            if let Some(upvalue_idx) = parent.resolve_upvalue(name) {
                // Add as upvalue capturing from parent's upvalue
                return Some(self.add_upvalue(upvalue_idx, false));
            }
        }

        None
    }

    /// Add an upvalue, returns its index
    fn add_upvalue(&mut self, index: u16, is_local: bool) -> u16 {
        // Check if already captured
        for (i, upvalue) in self.upvalues.iter().enumerate() {
            if upvalue.index == index && upvalue.is_local == is_local {
                return i as u16;
            }
        }

        let idx = self.upvalues.len() as u16;
        self.upvalues.push(Upvalue { index, is_local });
        idx
    }

    /// Get the number of locals
    pub fn local_count(&self) -> u16 {
        self.next_local
    }

    /// Get the number of upvalues
    pub fn upvalue_count(&self) -> u16 {
        self.upvalues.len() as u16
    }
}

/// Bytecode compiler
pub struct Compiler {
    /// The chunk being built
    builder: ChunkBuilder,
    /// Compilation context
    context: CompileContext,
    /// Current source line
    current_line: u32,
}

impl Compiler {
    /// Create a new compiler
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            builder: ChunkBuilder::new(name),
            context: CompileContext::new(),
            current_line: 1,
        }
    }

    /// Create a compiler with existing context (for nested functions)
    pub fn with_context(name: impl Into<String>, context: CompileContext) -> Self {
        Self {
            builder: ChunkBuilder::new(name),
            context,
            current_line: 1,
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

            // Space and State are runtime values, compile as constants
            MettaValue::Space(handle) => {
                let idx = self.builder.add_constant(MettaValue::Space(handle.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }
            MettaValue::State(handle) => {
                let idx = self.builder.add_constant(MettaValue::State(handle.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
            }
            MettaValue::Memo(handle) => {
                let idx = self.builder.add_constant(MettaValue::Memo(handle.clone()));
                self.builder.emit_u16(Opcode::PushConstant, idx);
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
            }
        }

        // Not a built-in - compile as generic S-expression
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

    /// Try to compile a built-in operation, returns Some(()) if handled
    fn try_compile_builtin(&mut self, op: &str, args: &[MettaValue]) -> CompileResult<Option<()>> {
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
            "pow" => {
                self.check_arity("pow", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Pow);
                Ok(Some(()))
            }
            "abs" => {
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
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                // Build a new S-expression with head + tail
                self.builder.emit_byte(Opcode::MakeList, 2);
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
                self.builder.emit(Opcode::PushEmpty);
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

            // State operations
            "new-state" => {
                self.check_arity("new-state", args.len(), 1)?;
                self.compile(&args[0])?;
                // Create state via special form
                let idx = self.builder.add_constant(MettaValue::Atom("new-state".to_string()));
                self.builder.emit_u16(Opcode::PushAtom, idx);
                self.builder.emit(Opcode::Swap);
                self.builder.emit_byte(Opcode::MakeSExpr, 2);
                Ok(Some(()))
            }
            "get-state" => {
                self.check_arity("get-state", args.len(), 1)?;
                self.compile(&args[0])?;
                let idx = self.builder.add_constant(MettaValue::Atom("get-state".to_string()));
                self.builder.emit_u16(Opcode::PushAtom, idx);
                self.builder.emit(Opcode::Swap);
                self.builder.emit_byte(Opcode::MakeSExpr, 2);
                Ok(Some(()))
            }
            "change-state!" => {
                self.check_arity("change-state!", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                let idx = self.builder.add_constant(MettaValue::Atom("change-state!".to_string()));
                self.builder.emit_u16(Opcode::PushAtom, idx);
                self.builder.emit(Opcode::Rot3);
                self.builder.emit_byte(Opcode::MakeSExpr, 3);
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

            // Not a built-in
            _ => Ok(None),
        }
    }

    /// Check arity of an operation
    fn check_arity(&self, op: &str, got: usize, expected: usize) -> CompileResult<()> {
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
    fn check_arity_range(&self, op: &str, got: usize, min: usize, max: usize) -> CompileResult<()> {
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

    /// Compile if expression: (if cond then else)
    fn compile_if(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("if", args.len(), 3)?;

        // Compile condition
        self.compile(&args[0])?;

        // Jump to else if false
        let else_jump = self.builder.emit_jump(Opcode::JumpIfFalse);

        // Compile then branch
        self.compile(&args[1])?;

        // Jump over else branch
        let end_jump = self.builder.emit_jump(Opcode::Jump);

        // Patch else jump
        self.builder.patch_jump(else_jump);

        // Compile else branch
        self.compile(&args[2])?;

        // Patch end jump
        self.builder.patch_jump(end_jump);

        Ok(())
    }

    /// Compile let expression: (let pattern value body)
    fn compile_let(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("let", args.len(), 3)?;

        // Begin new scope
        self.context.begin_scope();

        // Compile the value
        self.compile(&args[1])?;

        // Bind the pattern
        self.compile_pattern_binding(&args[0])?;

        // Compile the body
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
    fn compile_let_star(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("let*", args.len(), 2)?;

        // Get bindings list
        let bindings = match &args[0] {
            MettaValue::SExpr(items) => items,
            _ => return Err(CompileError::InvalidExpression(
                "let* bindings must be a list".to_string()
            )),
        };

        // Begin scope
        self.context.begin_scope();

        // Process each binding
        for binding in bindings {
            let (pattern, value) = match binding {
                MettaValue::SExpr(pair) if pair.len() == 2 => (&pair[0], &pair[1]),
                _ => return Err(CompileError::InvalidExpression(
                    "let* binding must be (pattern value)".to_string()
                )),
            };

            // Compile value
            self.compile(value)?;

            // Bind pattern
            self.compile_pattern_binding(pattern)?;
        }

        // Compile body
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
    fn compile_pattern_binding(&mut self, pattern: &MettaValue) -> CompileResult<()> {
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
    fn compile_quoted(&mut self, expr: &MettaValue) -> CompileResult<()> {
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
                    self.builder.emit_u16(Opcode::MakeSExprLarge, items.len() as u16);
                }
            }
            // Other values can be compiled normally (they're already values)
            _ => self.compile(expr)?,
        }
        Ok(())
    }

    /// Compile superpose: (superpose (a b c ...))
    fn compile_superpose(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("superpose", args.len(), 1)?;

        // Get the list of alternatives
        let alternatives = match &args[0] {
            MettaValue::SExpr(items) => items,
            _ => {
                // Single value - just compile it
                return self.compile(&args[0]);
            }
        };

        if alternatives.is_empty() {
            // Empty superpose - fail
            self.builder.emit(Opcode::Fail);
            return Ok(());
        }

        if alternatives.len() == 1 {
            // Single alternative - no nondeterminism
            return self.compile(&alternatives[0]);
        }

        // Multiple alternatives - use Fork
        // First, compile all alternatives as constants
        let mut alt_indices = Vec::new();
        for alt in alternatives {
            let idx = self.builder.add_constant(alt.clone());
            alt_indices.push(idx);
        }

        // Emit Fork with number of alternatives
        self.builder.emit_u16(Opcode::Fork, alt_indices.len() as u16);

        // Emit indices of alternatives
        for idx in alt_indices {
            self.builder.emit_raw(&idx.to_be_bytes());
        }

        Ok(())
    }

    /// Compile match expression: (match space pattern template [default])
    fn compile_match(&mut self, args: &[MettaValue]) -> CompileResult<()> {
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
    fn compile_unify(&mut self, args: &[MettaValue]) -> CompileResult<()> {
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
    fn compile_case(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity_range("case", args.len(), 2, usize::MAX)?;

        // Compile the scrutinee
        self.compile(&args[0])?;

        // Get the cases
        let cases = &args[1..];

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

    /// Compile a conjunction (multiple values)
    fn compile_conjunction(&mut self, values: &[MettaValue]) -> CompileResult<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to compile and disassemble
    fn compile_and_disasm(expr: &MettaValue) -> String {
        let chunk = compile("test", expr).expect("compilation should succeed");
        chunk.disassemble()
    }

    // ========================================================================
    // Literal Compilation Tests
    // ========================================================================

    #[test]
    fn test_compile_nil() {
        let chunk = compile("test", &MettaValue::Nil).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushNil));
    }

    #[test]
    fn test_compile_unit() {
        let chunk = compile("test", &MettaValue::Unit).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushUnit));
    }

    #[test]
    fn test_compile_true() {
        let chunk = compile("test", &MettaValue::Bool(true)).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushTrue));
    }

    #[test]
    fn test_compile_false() {
        let chunk = compile("test", &MettaValue::Bool(false)).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushFalse));
    }

    #[test]
    fn test_compile_small_int() {
        let chunk = compile("test", &MettaValue::Long(42)).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushLongSmall));
        assert_eq!(chunk.read_byte(1), Some(42));
    }

    #[test]
    fn test_compile_negative_small_int() {
        let chunk = compile("test", &MettaValue::Long(-10)).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushLongSmall));
        assert_eq!(chunk.read_byte(1), Some((-10i8) as u8));
    }

    #[test]
    fn test_compile_large_int() {
        let chunk = compile("test", &MettaValue::Long(1000)).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushLong));
        assert_eq!(chunk.get_constant(0), Some(&MettaValue::Long(1000)));
    }

    #[test]
    fn test_compile_string() {
        let chunk = compile("test", &MettaValue::String("hello".to_string())).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushString));
        assert_eq!(chunk.get_constant(0), Some(&MettaValue::String("hello".to_string())));
    }

    #[test]
    fn test_compile_float() {
        let chunk = compile("test", &MettaValue::Float(3.14)).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushConstant));
        assert_eq!(chunk.get_constant(0), Some(&MettaValue::Float(3.14)));
    }

    // ========================================================================
    // Symbol and Variable Tests
    // ========================================================================

    #[test]
    fn test_compile_symbol() {
        let chunk = compile("test", &MettaValue::Atom("foo".to_string())).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushAtom));
        assert_eq!(chunk.get_constant(0), Some(&MettaValue::Atom("foo".to_string())));
    }

    #[test]
    fn test_compile_variable() {
        let chunk = compile("test", &MettaValue::Atom("$x".to_string())).unwrap();
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushVariable));
        assert_eq!(chunk.get_constant(0), Some(&MettaValue::Atom("$x".to_string())));
    }

    // ========================================================================
    // Arithmetic Operations Tests
    // ========================================================================

    #[test]
    fn test_compile_add() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("push_long_small 1"));
        assert!(disasm.contains("push_long_small 2"));
        assert!(disasm.contains("add"));
    }

    #[test]
    fn test_compile_sub() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("-".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(3),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("sub"));
    }

    #[test]
    fn test_compile_mul() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Long(3),
            MettaValue::Long(4),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("mul"));
    }

    #[test]
    fn test_compile_div() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("/".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(2),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("div"));
    }

    #[test]
    fn test_compile_mod() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(3),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("mod"));
    }

    #[test]
    fn test_compile_nested_arithmetic() {
        // (+ (* 3 4) 5)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
            MettaValue::Long(5),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("push_long_small 3"));
        assert!(disasm.contains("push_long_small 4"));
        assert!(disasm.contains("mul"));
        assert!(disasm.contains("push_long_small 5"));
        assert!(disasm.contains("add"));
    }

    // ========================================================================
    // Comparison Operations Tests
    // ========================================================================

    #[test]
    fn test_compile_lt() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("lt"));
    }

    #[test]
    fn test_compile_le() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("<=".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("le"));
    }

    #[test]
    fn test_compile_gt() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom(">".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(1),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("gt"));
    }

    #[test]
    fn test_compile_ge() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom(">=".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(1),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("ge"));
    }

    #[test]
    fn test_compile_eq() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("==".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(1),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("eq"));
    }

    #[test]
    fn test_compile_ne() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("!=".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("ne"));
    }

    // ========================================================================
    // Boolean Operations Tests
    // ========================================================================

    #[test]
    fn test_compile_and() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("and".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("and"));
    }

    #[test]
    fn test_compile_or() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("or"));
    }

    #[test]
    fn test_compile_not() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("not".to_string()),
            MettaValue::Bool(true),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("not"));
    }

    // ========================================================================
    // Control Flow Tests
    // ========================================================================

    #[test]
    fn test_compile_if() {
        // (if True 1 2)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(true),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("push_true"));
        assert!(disasm.contains("jump_if_false"));
        assert!(disasm.contains("push_long_small 1"));
        assert!(disasm.contains("jump"));
        assert!(disasm.contains("push_long_small 2"));
    }

    #[test]
    fn test_compile_nested_if() {
        // (if (< 1 2) (if True 10 20) 30)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("<".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::Bool(true),
                MettaValue::Long(10),
                MettaValue::Long(20),
            ]),
            MettaValue::Long(30),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("lt"));
        // Should have multiple jumps for nested ifs
        assert!(disasm.matches("jump").count() >= 2);
    }

    // ========================================================================
    // Quote and Eval Tests
    // ========================================================================

    #[test]
    fn test_compile_quote() {
        // (quote (+ 1 2))
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        // Should build S-expression, not execute add
        assert!(disasm.contains("make_sexpr"));
        assert!(!disasm.contains("\nadd\n")); // No add operation
    }

    #[test]
    fn test_compile_eval() {
        // (eval expr)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("eval".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("eval_eval"));
    }

    // ========================================================================
    // Let Binding Tests
    // ========================================================================

    #[test]
    fn test_compile_let() {
        // (let $x 10 (+ $x 1))
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("push_long_small 10"));
        assert!(disasm.contains("store_local"));
        assert!(disasm.contains("load_local"));
        assert!(disasm.contains("add"));
    }

    #[test]
    fn test_compile_let_star() {
        // (let* (($x 1) ($y 2)) (+ $x $y))
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("let*".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::SExpr(vec![
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(1),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("$y".to_string()),
                    MettaValue::Long(2),
                ]),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("store_local"));
        assert!(disasm.contains("add"));
    }

    // ========================================================================
    // Type Operations Tests
    // ========================================================================

    #[test]
    fn test_compile_get_type() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Long(42),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("get_type"));
    }

    #[test]
    fn test_compile_check_type() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("Number".to_string()),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("check_type"));
    }

    // ========================================================================
    // List Operations Tests
    // ========================================================================

    #[test]
    fn test_compile_car_atom() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("car-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("get_head"));
    }

    #[test]
    fn test_compile_cdr_atom() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("cdr-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("get_tail"));
    }

    #[test]
    fn test_compile_size_atom() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("size-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("get_arity"));
    }

    #[test]
    fn test_compile_empty() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("empty".to_string()),
        ]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("push_empty"));
    }

    // ========================================================================
    // Generic S-Expression Tests
    // ========================================================================

    #[test]
    fn test_compile_unknown_operation() {
        // (foo 1 2 3) - unknown operation, compile as S-expression
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("push_atom")); // foo
        assert!(disasm.contains("make_sexpr 4"));
    }

    #[test]
    fn test_compile_empty_sexpr() {
        let expr = MettaValue::SExpr(vec![]);
        let chunk = compile("test", &expr).unwrap();
        assert!(chunk.disassemble().contains("push_empty"));
    }

    // ========================================================================
    // Error Handling Tests
    // ========================================================================

    #[test]
    fn test_compile_is_error() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::Long(42),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("jump_if_error"));
    }

    #[test]
    fn test_compile_catch() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::Long(42),
            MettaValue::Long(0),
        ]);
        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();
        assert!(disasm.contains("jump_if_error"));
    }

    // ========================================================================
    // Arity Error Tests
    // ========================================================================

    #[test]
    fn test_compile_add_wrong_arity() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
        ]);
        let result = compile("test", &expr);
        assert!(matches!(result, Err(CompileError::InvalidArity { .. })));
    }

    #[test]
    fn test_compile_if_wrong_arity() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(true),
            MettaValue::Long(1),
        ]);
        let result = compile("test", &expr);
        assert!(matches!(result, Err(CompileError::InvalidArity { .. })));
    }

    // ========================================================================
    // Integration Tests
    // ========================================================================

    #[test]
    fn test_compile_complex_expression() {
        // (let $x (+ 1 2) (if (< $x 5) (* $x 2) $x))
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("<".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(5),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(2),
                ]),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        let chunk = compile("test", &expr).unwrap();
        let disasm = chunk.disassemble();

        // Should contain all the expected operations
        assert!(disasm.contains("add"));
        assert!(disasm.contains("store_local"));
        assert!(disasm.contains("load_local"));
        assert!(disasm.contains("lt"));
        assert!(disasm.contains("jump_if_false"));
        assert!(disasm.contains("mul"));
    }

    #[test]
    fn test_constant_deduplication() {
        // Same constant used multiple times should be deduplicated
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1000), // Large int goes to constant pool
            MettaValue::Long(1000), // Same value
        ]);
        let chunk = compile("test", &expr).unwrap();
        // Should only have one constant for 1000
        assert_eq!(chunk.constant_count(), 1);
    }
}
