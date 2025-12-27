//! JIT error types.
//!
//! This module defines [`JitError`] and [`JitResult`] for JIT compilation
//! and execution error handling.

use std::fmt;

// =============================================================================
// JitResult and JitError
// =============================================================================

/// Error types for JIT compilation and execution
#[derive(Debug, Clone)]
pub enum JitError {
    /// Bytecode chunk cannot be JIT compiled
    NotCompilable(String),

    /// Cranelift compilation error
    CompilationError(String),

    /// Type error during JIT execution
    TypeError { expected: &'static str, got: u64 },

    /// Stack overflow
    StackOverflow,

    /// Stack underflow
    StackUnderflow,

    /// Division by zero
    DivisionByZero,

    /// Invalid opcode encountered
    InvalidOpcode(u8),

    /// Bailout to bytecode VM required
    Bailout { ip: usize, reason: String },

    /// Invalid local variable index
    InvalidLocalIndex(usize),

    /// Invalid binding (variable not found)
    InvalidBinding(String),

    /// Binding frame overflow
    BindingFrameOverflow,
}

impl fmt::Display for JitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JitError::NotCompilable(msg) => write!(f, "Not compilable: {}", msg),
            JitError::CompilationError(msg) => write!(f, "Compilation error: {}", msg),
            JitError::TypeError { expected, got } => {
                write!(f, "Type error: expected {}, got tag {:#x}", expected, got)
            }
            JitError::StackOverflow => write!(f, "Stack overflow"),
            JitError::StackUnderflow => write!(f, "Stack underflow"),
            JitError::DivisionByZero => write!(f, "Division by zero"),
            JitError::InvalidOpcode(op) => write!(f, "Invalid opcode: {:#x}", op),
            JitError::Bailout { ip, reason } => {
                write!(f, "Bailout at ip {}: {}", ip, reason)
            }
            JitError::InvalidLocalIndex(idx) => write!(f, "Invalid local index: {}", idx),
            JitError::InvalidBinding(name) => write!(f, "Invalid binding: {}", name),
            JitError::BindingFrameOverflow => write!(f, "Binding frame stack overflow"),
        }
    }
}

impl std::error::Error for JitError {}

/// Result type for JIT operations
pub type JitResult<T> = Result<T, JitError>;
