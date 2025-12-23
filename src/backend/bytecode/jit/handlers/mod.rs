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
mod locals;
#[cfg(feature = "jit")]
mod type_predicates;
#[cfg(feature = "jit")]
mod math;
#[cfg(feature = "jit")]
mod sexpr;
#[cfg(feature = "jit")]
mod expr;

#[cfg(feature = "jit")]
pub use stack::compile_stack_op;
#[cfg(feature = "jit")]
pub use values::{compile_simple_value_op, compile_runtime_value_op, ValueHandlerContext};
#[cfg(feature = "jit")]
pub use arithmetic::{compile_simple_arithmetic_op, compile_pow, ArithmeticHandlerContext};
#[cfg(feature = "jit")]
pub use comparison::{compile_boolean_op, compile_comparison_op};
#[cfg(feature = "jit")]
pub use locals::compile_local_op;
#[cfg(feature = "jit")]
pub use type_predicates::compile_type_predicate_op;
#[cfg(feature = "jit")]
pub use math::{compile_extended_math_op, MathHandlerContext};
#[cfg(feature = "jit")]
pub use sexpr::{compile_sexpr_access_op, compile_sexpr_create_op, SExprHandlerContext};
#[cfg(feature = "jit")]
pub use expr::{compile_expr_op, ExprHandlerContext};
