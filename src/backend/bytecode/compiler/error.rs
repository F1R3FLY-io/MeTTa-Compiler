//! Compiler error types for the bytecode compiler.

/// Compiler error types
#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    /// Unknown operation encountered
    UnknownOperation(String),
    /// Invalid arity for operation
    InvalidArity {
        op: String,
        expected: usize,
        got: usize,
    },
    /// Invalid arity range for operation
    InvalidArityRange {
        op: String,
        min: usize,
        max: usize,
        got: usize,
    },
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
                write!(
                    f,
                    "Invalid arity for {}: expected {}, got {}",
                    op, expected, got
                )
            }
            Self::InvalidArityRange { op, min, max, got } => {
                write!(
                    f,
                    "Invalid arity for {}: expected {}-{}, got {}",
                    op, min, max, got
                )
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
