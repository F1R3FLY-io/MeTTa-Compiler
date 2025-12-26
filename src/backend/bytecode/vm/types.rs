//! Type definitions for the bytecode VM.
//!
//! This module contains the core types used throughout the VM:
//! - VmError: Error types that can occur during execution
//! - CallFrame: Stack frame for function calls
//! - BindingFrame: Frame for pattern variable bindings
//! - ChoicePoint: Nondeterminism choice point
//! - Alternative: An alternative in a choice point
//! - VmConfig: VM configuration options

use std::sync::Arc;
use smallvec::SmallVec;

use crate::backend::models::{Bindings, MettaValue};
use crate::backend::bytecode::chunk::BytecodeChunk;

/// Result of VM execution
pub type VmResult<T> = Result<T, VmError>;

/// Errors that can occur during VM execution
#[derive(Debug, Clone)]
pub enum VmError {
    /// Stack underflow
    StackUnderflow,
    /// Invalid opcode byte
    InvalidOpcode(u8),
    /// Invalid constant index
    InvalidConstant(u16),
    /// Invalid local variable index
    InvalidLocal(u16),
    /// Invalid binding name
    InvalidBinding(String),
    /// Type error in operation
    TypeError { expected: &'static str, got: &'static str },
    /// Division by zero
    DivisionByZero,
    /// Arithmetic overflow (e.g., i64::MIN % -1)
    ArithmeticOverflow,
    /// Instruction pointer out of bounds
    IpOutOfBounds,
    /// Call stack overflow
    CallStackOverflow,
    /// Value stack overflow
    ValueStackOverflow,
    /// Halt instruction executed
    Halted,
    /// Runtime error with message
    Runtime(String),
    /// Index out of bounds
    IndexOutOfBounds { index: usize, len: usize },
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StackUnderflow => write!(f, "Stack underflow"),
            Self::InvalidOpcode(b) => write!(f, "Invalid opcode: 0x{:02x}", b),
            Self::InvalidConstant(i) => write!(f, "Invalid constant index: {}", i),
            Self::InvalidLocal(i) => write!(f, "Invalid local variable index: {}", i),
            Self::InvalidBinding(name) => write!(f, "Invalid binding: {}", name),
            Self::TypeError { expected, got } => {
                write!(f, "Type error: expected {}, got {}", expected, got)
            }
            Self::DivisionByZero => write!(f, "Division by zero"),
            Self::ArithmeticOverflow => write!(f, "Arithmetic overflow"),
            Self::IpOutOfBounds => write!(f, "Instruction pointer out of bounds"),
            Self::CallStackOverflow => write!(f, "Call stack overflow"),
            Self::ValueStackOverflow => write!(f, "Value stack overflow"),
            Self::Halted => write!(f, "Execution halted"),
            Self::Runtime(msg) => write!(f, "Runtime error: {}", msg),
            Self::IndexOutOfBounds { index, len } => {
                write!(f, "Index out of bounds: index {} but length is {}", index, len)
            }
        }
    }
}

impl std::error::Error for VmError {}

/// Call frame on the call stack
#[derive(Debug, Clone)]
pub struct CallFrame {
    /// Return instruction pointer
    pub return_ip: usize,
    /// Return chunk
    pub return_chunk: Arc<BytecodeChunk>,
    /// Base pointer into value stack
    pub base_ptr: usize,
    /// Base pointer into bindings stack
    pub bindings_base: usize,
}

/// Binding frame for pattern variables
#[derive(Debug, Clone)]
pub struct BindingFrame {
    /// Variable bindings: name -> value
    pub bindings: SmallVec<[(String, MettaValue); 8]>,
    /// Scope depth for nested bindings
    pub scope_depth: u32,
}

impl BindingFrame {
    /// Create a new empty binding frame
    pub fn new(scope_depth: u32) -> Self {
        Self {
            bindings: SmallVec::new(),
            scope_depth,
        }
    }

    /// Get a binding by name
    pub fn get(&self, name: &str) -> Option<&MettaValue> {
        self.bindings.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }

    /// Set a binding
    pub fn set(&mut self, name: String, value: MettaValue) {
        // Check if binding already exists
        for (n, v) in self.bindings.iter_mut() {
            if n == &name {
                *v = value;
                return;
            }
        }
        self.bindings.push((name, value));
    }

    /// Check if a binding exists
    pub fn has(&self, name: &str) -> bool {
        self.bindings.iter().any(|(n, _)| n == name)
    }

    /// Clear all bindings
    pub fn clear(&mut self) {
        self.bindings.clear();
    }
}

/// Choice point for nondeterminism
#[derive(Debug, Clone)]
pub struct ChoicePoint {
    /// Saved value stack height
    pub value_stack_height: usize,
    /// Saved call stack height
    pub call_stack_height: usize,
    /// Saved bindings stack height
    pub bindings_stack_height: usize,
    /// Continuation instruction pointer
    pub ip: usize,
    /// Continuation chunk
    pub chunk: Arc<BytecodeChunk>,
    /// Remaining alternatives to try
    pub alternatives: Vec<Alternative>,
}

/// An alternative in a choice point
#[derive(Debug, Clone)]
pub enum Alternative {
    /// A value to push and continue
    Value(MettaValue),
    /// A bytecode chunk to execute
    Chunk(Arc<BytecodeChunk>),
    /// An index into something (rules, etc)
    Index(usize),
    /// A rule match with compiled body and bindings (for multi-match Call)
    RuleMatch {
        /// The compiled rule body to execute
        chunk: Arc<BytecodeChunk>,
        /// Pattern variable bindings from matching
        bindings: Bindings,
    },
}

/// Configuration for the VM
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Maximum value stack size
    pub max_value_stack: usize,
    /// Maximum call stack size
    pub max_call_stack: usize,
    /// Maximum choice point stack size
    pub max_choice_points: usize,
    /// Enable tracing
    pub trace: bool,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            max_value_stack: 65536,
            max_call_stack: 1024,
            max_choice_points: 4096,
            trace: false,
        }
    }
}
