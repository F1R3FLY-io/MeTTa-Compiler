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

// Re-export main types
pub use opcodes::Opcode;
pub use chunk::{BytecodeChunk, ChunkBuilder, CompiledPattern, JumpLabel, JumpLabelShort, JumpTable};
pub use vm::{BytecodeVM, VmConfig, VmError, VmResult, CallFrame, BindingFrame, ChoicePoint, Alternative};

/// Feature flag for enabling bytecode VM
///
/// When enabled, the evaluator will attempt to compile and execute
/// expressions via bytecode before falling back to tree-walking.
#[cfg(feature = "bytecode")]
pub const BYTECODE_ENABLED: bool = true;

#[cfg(not(feature = "bytecode"))]
pub const BYTECODE_ENABLED: bool = false;

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
}
