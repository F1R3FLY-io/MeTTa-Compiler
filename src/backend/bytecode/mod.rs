//! Bytecode VM Module
//!
//! This module provides a stack-based bytecode virtual machine for executing
//! MeTTa programs. The VM replaces the tree-walking interpreter with a more
//! efficient bytecode-based execution model.
//!
//! # Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                        MeTTa Source                                │
//! └───────────────────────────────────────────────────────────────────┘
//!                                 │
//!                                 ▼
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                    Parser (Tree-Sitter)                           │
//! │              Source → MettaValue (AST)                            │
//! └───────────────────────────────────────────────────────────────────┘
//!                                 │
//!                                 ▼
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                    Bytecode Compiler                              │
//! │             MettaValue → BytecodeChunk                            │
//! └───────────────────────────────────────────────────────────────────┘
//!                                 │
//!                                 ▼
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                    Bytecode VM                                    │
//! │                                                                   │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐               │
//! │  │ Value Stack │  │ Call Stack  │  │ Bindings    │               │
//! │  │             │  │             │  │ Stack       │               │
//! │  └─────────────┘  └─────────────┘  └─────────────┘               │
//! │                                                                   │
//! │  ┌─────────────┐  ┌─────────────────────────────────────────┐    │
//! │  │ Choice Pts  │  │            MORK Bridge                  │    │
//! │  │ (Nondet)    │  │  - Rule lookup via PathMap              │    │
//! │  └─────────────┘  │  - Pattern matching                     │    │
//! │                   │  - Compiled rule cache                  │    │
//! │                   └─────────────────────────────────────────┘    │
//! └───────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`opcodes`]: Bytecode instruction definitions (~100 opcodes)
//! - [`chunk`]: BytecodeChunk structure with constant pool
//! - [`vm`]: Virtual machine execution engine
//!
//! # Example
//!
//! ```ignore
//! use mettatron::backend::bytecode::{BytecodeChunk, BytecodeVM, Opcode, ChunkBuilder};
//!
//! // Build a simple program: 40 + 2
//! let mut builder = ChunkBuilder::new("example");
//! builder.emit_byte(Opcode::PushLongSmall, 40);
//! builder.emit_byte(Opcode::PushLongSmall, 2);
//! builder.emit(Opcode::Add);
//! builder.emit(Opcode::Return);
//!
//! let chunk = builder.build_arc();
//! let mut vm = BytecodeVM::new(chunk);
//! let results = vm.run().expect("execution failed");
//! assert_eq!(results[0], MettaValue::Long(42));
//! ```
//!
//! # Performance Goals
//!
//! The bytecode VM aims to achieve 15-30% speedup over the tree-walking
//! interpreter by:
//!
//! 1. **Eliminating dispatch overhead**: Direct opcode dispatch vs enum matching
//! 2. **Pre-compiled patterns**: Pattern structure analyzed at compile time
//! 3. **Constant pool**: Reduced allocation for repeated values
//! 4. **Inline caching**: Hot paths get specialized code
//!
//! # Nondeterminism
//!
//! MeTTa supports nondeterministic evaluation (multiple results). The VM
//! handles this via choice points and backtracking:
//!
//! - `Fork`: Create a choice point with multiple alternatives
//! - `Fail`: Backtrack to most recent choice point
//! - `Yield`: Save a result and continue searching for more
//! - `Cut`: Remove choice points (commit to current path)
//!
//! # MORK Integration
//!
//! Rule lookup still uses MORK/PathMap for efficient pattern matching:
//!
//! 1. Expression to evaluate → MORK finds matching rules
//! 2. Matched rules compiled to bytecode (cached)
//! 3. VM executes compiled rule body
//!
//! This hybrid approach keeps MORK's O(k) pattern matching while
//! gaining bytecode's execution efficiency.

pub mod opcodes;
pub mod chunk;
pub mod vm;
pub mod compiler;
pub mod mork_bridge;

// Re-export main types
pub use opcodes::Opcode;
pub use chunk::{BytecodeChunk, ChunkBuilder, CompiledPattern, JumpLabel, JumpLabelShort, JumpTable};
pub use vm::{BytecodeVM, VmConfig, VmError, VmResult, CallFrame, BindingFrame, ChoicePoint, Alternative};
pub use compiler::{Compiler, CompileContext, CompileError, CompileResult, compile, compile_arc};
pub use mork_bridge::{MorkBridge, CompiledRule, BridgeStats};

/// Feature flag for enabling bytecode VM
///
/// When enabled, the evaluator will attempt to compile and execute
/// expressions via bytecode before falling back to tree-walking.
#[cfg(feature = "bytecode")]
pub const BYTECODE_ENABLED: bool = true;

#[cfg(not(feature = "bytecode"))]
pub const BYTECODE_ENABLED: bool = false;

use crate::backend::models::MettaValue;

/// Error type for bytecode evaluation
#[derive(Debug)]
pub enum BytecodeEvalError {
    /// Compilation failed
    CompileError(CompileError),
    /// VM execution failed
    VmError(VmError),
}

impl From<CompileError> for BytecodeEvalError {
    fn from(e: CompileError) -> Self {
        BytecodeEvalError::CompileError(e)
    }
}

impl From<VmError> for BytecodeEvalError {
    fn from(e: VmError) -> Self {
        BytecodeEvalError::VmError(e)
    }
}

impl std::fmt::Display for BytecodeEvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CompileError(e) => write!(f, "Compilation error: {}", e),
            Self::VmError(e) => write!(f, "VM error: {}", e),
        }
    }
}

impl std::error::Error for BytecodeEvalError {}

/// Check if an expression can be compiled to bytecode
///
/// Returns true for expressions that the bytecode compiler supports.
/// Currently supports:
/// - Literals (numbers, bools, strings)
/// - Variables (atoms starting with $)
/// - Arithmetic operations (+, -, *, /, %) with compilable operands
/// - Comparison operations (<, <=, >, >=, ==) with compilable operands
/// - Boolean operations (and, or, not) with compilable operands
/// - Quote (returns argument unevaluated)
/// - Superpose/collapse (nondeterminism) with compilable alternatives
///
/// **Important**: This function is recursive - it checks that ALL subexpressions
/// can also be compiled. This prevents the bytecode VM from returning wrong results
/// when a subexpression needs rule resolution.
pub fn can_compile(expr: &MettaValue) -> bool {
    match expr {
        // Always compilable literals
        MettaValue::Nil | MettaValue::Unit | MettaValue::Bool(_) |
        MettaValue::Long(_) | MettaValue::Float(_) | MettaValue::String(_) => true,

        // Atoms: only variables and known constants are safe
        // - Variables (start with $) are OK - they'll be substituted
        // - Other atoms might need rule resolution, so reject them
        MettaValue::Atom(name) => {
            // Variables are OK
            if name.starts_with('$') {
                return true;
            }
            // Known constants are OK
            match name.as_str() {
                "True" | "False" | "Nil" | "Unit" | "_" => true,
                // Other atoms could be function calls - reject
                _ => false,
            }
        }

        // S-expressions - check head AND all operands recursively
        MettaValue::SExpr(items) if items.is_empty() => true,
        MettaValue::SExpr(items) => {
            // Check head for supported operations
            if let MettaValue::Atom(head) = &items[0] {
                let head_ok = match head.as_str() {
                    // Arithmetic
                    "+" | "-" | "*" | "/" | "%" | "abs" | "pow" => true,
                    // Comparison
                    "<" | "<=" | ">" | ">=" | "==" | "!=" => true,
                    // Boolean
                    "and" | "or" | "not" | "xor" => true,
                    // Control flow - if needs compilable condition and branches
                    "if" => true,
                    // Quote - argument is NOT evaluated, so always OK
                    "quote" => return true, // Early return - don't check args
                    // Nondeterminism
                    "superpose" | "collapse" => true,
                    // List operations
                    "car-atom" | "cdr-atom" | "cons-atom" | "size-atom" => true,
                    // Reject everything else (let, match, unify, etc.)
                    _ => false,
                };

                if !head_ok {
                    return false;
                }

                // IMPORTANT: Recursively check all operands
                // This ensures we don't compile (+ 1 (foo $x)) where (foo $x)
                // would need rule resolution
                items.iter().skip(1).all(can_compile)
            } else {
                // Non-atom head - this is a data list like (1 2 3), not a function call
                // All elements must be compilable
                items.iter().all(can_compile)
            }
        }

        // Errors can be compiled (they just push the error value)
        MettaValue::Error(_, _) => true,

        // Types that need environment or special runtime support
        MettaValue::Space(_) | MettaValue::State(_) | MettaValue::Type(_) |
        MettaValue::Conjunction(_) | MettaValue::Memo(_) => false,
    }
}

/// Evaluate an expression using the bytecode VM
///
/// Compiles the expression to bytecode and executes it.
/// Returns the results or an error if compilation/execution fails.
///
/// # Example
/// ```ignore
/// let expr = MettaValue::SExpr(vec![
///     MettaValue::Atom("+".to_string()),
///     MettaValue::Long(1),
///     MettaValue::Long(2),
/// ]);
/// let results = eval_bytecode(&expr)?;
/// assert_eq!(results[0], MettaValue::Long(3));
/// ```
pub fn eval_bytecode(expr: &MettaValue) -> Result<Vec<MettaValue>, BytecodeEvalError> {
    // Compile expression to bytecode
    let chunk = compile_arc("eval", expr)?;

    // Create and run VM
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run()?;

    Ok(results)
}

/// Evaluate an expression using the bytecode VM with configuration
pub fn eval_bytecode_with_config(
    expr: &MettaValue,
    config: VmConfig,
) -> Result<Vec<MettaValue>, BytecodeEvalError> {
    let chunk = compile_arc("eval", expr)?;
    let mut vm = BytecodeVM::with_config(chunk, config);
    let results = vm.run()?;
    Ok(results)
}

/// Try to evaluate via bytecode, falling back to provided fallback function
///
/// This is the recommended integration point for the main eval loop.
/// If bytecode is enabled and the expression is compilable, it tries bytecode.
/// On success, returns bytecode results. On failure, calls the fallback.
pub fn try_bytecode_eval<F>(expr: &MettaValue, fallback: F) -> Vec<MettaValue>
where
    F: FnOnce() -> Vec<MettaValue>,
{
    if BYTECODE_ENABLED && can_compile(expr) {
        match eval_bytecode(expr) {
            Ok(results) => results,
            Err(_) => fallback(),
        }
    } else {
        fallback()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_integration_arithmetic() {
        // Test: (+ (* 3 4) 5) = 17
        let mut builder = ChunkBuilder::new("arithmetic");

        // Push 3, 4, multiply
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::PushLongSmall, 4);
        builder.emit(Opcode::Mul);

        // Push 5, add
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Add);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(17));
    }

    #[test]
    fn test_integration_conditionals() {
        // Test: if (< 5 10) then 1 else 2
        let mut builder = ChunkBuilder::new("conditionals");

        // Compare 5 < 10
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Lt);

        // Jump to else if false
        let else_jump = builder.emit_jump(Opcode::JumpIfFalse);

        // Then branch: push 1
        builder.emit_byte(Opcode::PushLongSmall, 1);
        let end_jump = builder.emit_jump(Opcode::Jump);

        // Else branch: push 2
        builder.patch_jump(else_jump);
        builder.emit_byte(Opcode::PushLongSmall, 2);

        builder.patch_jump(end_jump);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1)); // 5 < 10 is true
    }

    #[test]
    fn test_integration_boolean_logic() {
        // Test: (and (or True False) (not False))
        let mut builder = ChunkBuilder::new("boolean");

        // (or True False)
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);

        // (not False)
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Not);

        // (and ...)
        builder.emit(Opcode::And);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_integration_stack_operations() {
        // Test: dup, swap, over operations
        let mut builder = ChunkBuilder::new("stack_ops");

        // Push 1, 2, swap -> [2, 1]
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit(Opcode::Swap);

        // Now we have [2, 1], subtract -> 2 - 1 = 1
        builder.emit(Opcode::Sub);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));
    }

    #[test]
    fn test_integration_sexpr_construction() {
        // Test: Build (foo 1 2)
        let mut builder = ChunkBuilder::new("sexpr");

        let foo_idx = builder.add_constant(MettaValue::sym("foo"));
        builder.emit_u16(Opcode::PushAtom, foo_idx);
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::MakeSExpr, 3);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::sym("foo"));
                assert_eq!(items[1], MettaValue::Long(1));
                assert_eq!(items[2], MettaValue::Long(2));
            }
            _ => panic!("Expected S-expression"),
        }
    }

    #[test]
    fn test_chunk_disassembly() {
        let mut builder = ChunkBuilder::new("disasm_test");
        builder.set_line(1);
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.set_line(2);
        builder.emit(Opcode::Dup);
        builder.emit(Opcode::Add);
        builder.set_line(3);
        builder.emit(Opcode::Return);

        let chunk = builder.build();
        let disasm = chunk.disassemble();

        assert!(disasm.contains("disasm_test"));
        assert!(disasm.contains("push_long_small 42"));
        assert!(disasm.contains("dup"));
        assert!(disasm.contains("add"));
        assert!(disasm.contains("return"));
    }

    // Integration function tests

    #[test]
    fn test_can_compile_literals() {
        // Compilable literals
        assert!(can_compile(&MettaValue::Nil));
        assert!(can_compile(&MettaValue::Unit));
        assert!(can_compile(&MettaValue::Bool(true)));
        assert!(can_compile(&MettaValue::Long(42)));
        assert!(can_compile(&MettaValue::Float(3.14)));
        assert!(can_compile(&MettaValue::String("hello".to_string())));

        // Variables are compilable
        assert!(can_compile(&MettaValue::Atom("$x".to_string())));

        // Known constants are compilable
        assert!(can_compile(&MettaValue::Atom("True".to_string())));
        assert!(can_compile(&MettaValue::Atom("False".to_string())));
        assert!(can_compile(&MettaValue::Atom("Nil".to_string())));
        assert!(can_compile(&MettaValue::Atom("Unit".to_string())));
        assert!(can_compile(&MettaValue::Atom("_".to_string()))); // Wildcard

        // Plain atoms are NOT compilable (they could be function calls)
        assert!(!can_compile(&MettaValue::Atom("foo".to_string())));
        assert!(!can_compile(&MettaValue::Atom("bar".to_string())));
    }

    #[test]
    fn test_can_compile_arithmetic() {
        // Arithmetic operations
        let add = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(can_compile(&add));

        let mul = MettaValue::SExpr(vec![
            MettaValue::sym("*"),
            MettaValue::Long(3),
            MettaValue::Long(4),
        ]);
        assert!(can_compile(&mul));
    }

    #[test]
    fn test_can_compile_comparisons() {
        let lt = MettaValue::SExpr(vec![
            MettaValue::sym("<"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(can_compile(&lt));

        let eq = MettaValue::SExpr(vec![
            MettaValue::sym("=="),
            MettaValue::Long(1),
            MettaValue::Long(1),
        ]);
        assert!(can_compile(&eq));
    }

    #[test]
    fn test_can_compile_control_flow() {
        let if_expr = MettaValue::SExpr(vec![
            MettaValue::sym("if"),
            MettaValue::Bool(true),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(can_compile(&if_expr));

        let quote_expr = MettaValue::SExpr(vec![
            MettaValue::sym("quote"),
            MettaValue::Long(42),
        ]);
        assert!(can_compile(&quote_expr));
    }

    #[test]
    fn test_can_compile_nondeterminism() {
        let superpose = MettaValue::SExpr(vec![
            MettaValue::sym("superpose"),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        assert!(can_compile(&superpose));

        let collapse = MettaValue::SExpr(vec![
            MettaValue::sym("collapse"),
            MettaValue::Long(42),
        ]);
        assert!(can_compile(&collapse));
    }

    #[test]
    fn test_can_compile_not_compilable() {
        use crate::backend::models::SpaceHandle;

        // Special forms that need environment
        let let_expr = MettaValue::SExpr(vec![
            MettaValue::sym("let"),
            MettaValue::sym("$x"),
            MettaValue::Long(1),
            MettaValue::sym("$x"),
        ]);
        assert!(!can_compile(&let_expr));

        // Rule definition
        let rule = MettaValue::SExpr(vec![
            MettaValue::sym("="),
            MettaValue::sym("foo"),
            MettaValue::Long(42),
        ]);
        assert!(!can_compile(&rule));

        // Space values
        let space = MettaValue::Space(SpaceHandle::new(1, "test".to_string()));
        assert!(!can_compile(&space));

        // State values
        let state = MettaValue::State(123);
        assert!(!can_compile(&state));
    }

    #[test]
    fn test_eval_bytecode_simple() {
        // Test simple arithmetic: (+ 1 2) = 3
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_eval_bytecode_nested() {
        // Test nested: (+ (* 2 3) 4) = 10
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::SExpr(vec![
                MettaValue::sym("*"),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Long(4),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    #[test]
    fn test_eval_bytecode_boolean() {
        // Test: (and True False) = False
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("and"),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_eval_bytecode_comparison() {
        // Test: (< 5 10) = True
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("<"),
            MettaValue::Long(5),
            MettaValue::Long(10),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_try_bytecode_eval_success() {
        // When bytecode is enabled and expression is compilable, use bytecode
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]);

        let results = try_bytecode_eval(&expr, || {
            vec![MettaValue::Long(999)] // Fallback should not be called
        });

        // If bytecode is enabled, result should be 30
        // If bytecode is disabled, fallback returns 999
        #[cfg(feature = "bytecode")]
        assert_eq!(results[0], MettaValue::Long(30));
        #[cfg(not(feature = "bytecode"))]
        assert_eq!(results[0], MettaValue::Long(999));
    }

    #[test]
    fn test_try_bytecode_eval_fallback() {
        // Non-compilable expression should use fallback
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("let"),
            MettaValue::sym("$x"),
            MettaValue::Long(1),
            MettaValue::sym("$x"),
        ]);

        let results = try_bytecode_eval(&expr, || {
            vec![MettaValue::Long(42)] // Fallback should be called
        });

        assert_eq!(results[0], MettaValue::Long(42));
    }
}
