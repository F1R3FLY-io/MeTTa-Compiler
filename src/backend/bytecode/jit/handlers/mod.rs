//! Opcode handlers for JIT compilation
//!
//! This module contains handlers for each category of bytecode opcodes.
//! Each handler module compiles specific opcodes to Cranelift IR.

#[cfg(feature = "jit")]
mod stack;
#[cfg(feature = "jit")]
mod values;
#[cfg(feature = "jit")]
mod arithmetic;
#[cfg(feature = "jit")]
mod comparison;

#[cfg(feature = "jit")]
pub use stack::compile_stack_op;
#[cfg(feature = "jit")]
pub use values::{compile_simple_value_op, compile_runtime_value_op, ValueHandlerContext};
#[cfg(feature = "jit")]
pub use arithmetic::{compile_simple_arithmetic_op, compile_pow, ArithmeticHandlerContext};
#[cfg(feature = "jit")]
pub use comparison::{compile_boolean_op, compile_comparison_op};
