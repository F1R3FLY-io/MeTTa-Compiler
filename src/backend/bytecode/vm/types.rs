//! Type definitions for the bytecode VM.
//!
//! This module contains the core types used throughout the VM:
//! - VmError: Error types that can occur during execution
//! - CallFrame: Stack frame for function calls
//! - BindingFrame: Frame for pattern variable bindings
//! - ChoicePoint: Nondeterminism choice point
//! - Alternative: An alternative in a choice point
//! - VmConfig: VM configuration options
//!
//! ## Generic Types
//!
//! The bytecode VM supports generic value types via the `MettaValueTrait` and
//! `MettaValueFactory` traits. This enables zero-conversion evaluation with
//! both heap-allocated (`MettaValue`) and arena-allocated (`ArenaValue`) values.
//!
//! Generic types are prefixed with `Generic`:
//! - `GenericBindingFrame<V>`: Binding frame for any value type
//! - `GenericAlternative<V, C>`: Alternative in a choice point
//! - `GenericChoicePoint<V, C>`: Choice point for nondeterminism
//! - `GenericCallFrame<C>`: Call frame with generic chunk reference
//!
//! Backwards-compatible type aliases are provided:
//! - `BindingFrame = GenericBindingFrame<MettaValue>`
//! - `Alternative = GenericAlternative<MettaValue, BytecodeChunk>`
//! - `ChoicePoint = GenericChoicePoint<MettaValue, BytecodeChunk>`

use smallvec::SmallVec;
use std::sync::Arc;

use crate::backend::bytecode::chunk::BytecodeChunk;
use crate::backend::models::{Bindings, GenericBindings, MettaValue, MettaValueTrait};

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
    TypeError {
        expected: &'static str,
        got: &'static str,
    },
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
    /// Compilation failed
    CompileError,
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
                write!(
                    f,
                    "Index out of bounds: index {} but length is {}",
                    index, len
                )
            }
            Self::CompileError => write!(f, "Compilation failed"),
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
        self.bindings
            .iter()
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

// ============================================================================
// Generic Types - Zero-Conversion Support
// ============================================================================

/// Generic binding frame for pattern variables.
///
/// This is the generic version of `BindingFrame` that works with any value type
/// implementing `MettaValueTrait`. It stores variable bindings using a `SmallVec`
/// for efficiency with typical small binding counts.
///
/// # Type Parameters
///
/// - `V`: The value type (e.g., `MettaValue` or `ArenaValue<'static>`)
#[derive(Debug, Clone)]
pub struct GenericBindingFrame<V>
where
    V: MettaValueTrait + Clone + Send + Sync + 'static,
{
    /// Variable bindings: name -> value
    pub bindings: SmallVec<[(String, V); 8]>,
    /// Scope depth for nested bindings
    pub scope_depth: u32,
}

impl<V> GenericBindingFrame<V>
where
    V: MettaValueTrait + Clone + Send + Sync + 'static,
{
    /// Create a new empty binding frame
    #[inline]
    pub fn new(scope_depth: u32) -> Self {
        Self {
            bindings: SmallVec::new(),
            scope_depth,
        }
    }

    /// Get a binding by name
    #[inline]
    pub fn get(&self, name: &str) -> Option<&V> {
        self.bindings
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }

    /// Set a binding
    #[inline]
    pub fn set(&mut self, name: String, value: V) {
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
    #[inline]
    pub fn has(&self, name: &str) -> bool {
        self.bindings.iter().any(|(n, _)| n == name)
    }

    /// Clear all bindings
    #[inline]
    pub fn clear(&mut self) {
        self.bindings.clear();
    }

    /// Get an iterator over all bindings
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.bindings.iter().map(|(n, v)| (n.as_str(), v))
    }

    /// Get the number of bindings
    #[inline]
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Check if the frame is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Convert from GenericBindings
    pub fn from_generic_bindings(bindings: &GenericBindings<V>, scope_depth: u32) -> Self {
        let mut frame = Self::new(scope_depth);
        for (name, value) in bindings.iter() {
            frame.set(name.to_string(), value.clone());
        }
        frame
    }
}

/// Generic alternative in a choice point.
///
/// Represents one possible branch in nondeterministic evaluation.
///
/// # Type Parameters
///
/// - `V`: The value type (e.g., `MettaValue` or `ArenaValue<'static>`)
/// - `C`: The bytecode chunk type (e.g., `BytecodeChunk` or `GenericBytecodeChunk<V>`)
#[derive(Debug, Clone)]
pub enum GenericAlternative<V, C>
where
    V: MettaValueTrait + Clone + Send + Sync + 'static,
{
    /// A value to push and continue
    Value(V),
    /// A bytecode chunk to execute
    Chunk(Arc<C>),
    /// An index into something (rules, etc)
    Index(usize),
    /// A rule match with compiled body and bindings (for multi-match Call)
    RuleMatch {
        /// The compiled rule body to execute
        chunk: Arc<C>,
        /// Pattern variable bindings from matching
        bindings: GenericBindings<V>,
    },
}

/// Generic call frame on the call stack.
///
/// # Type Parameters
///
/// - `C`: The bytecode chunk type
#[derive(Debug, Clone)]
pub struct GenericCallFrame<C> {
    /// Return instruction pointer
    pub return_ip: usize,
    /// Return chunk
    pub return_chunk: Arc<C>,
    /// Base pointer into value stack
    pub base_ptr: usize,
    /// Base pointer into bindings stack
    pub bindings_base: usize,
}

/// Generic choice point for nondeterminism.
///
/// Stores the state needed to backtrack and try alternative branches.
///
/// # Type Parameters
///
/// - `V`: The value type
/// - `C`: The bytecode chunk type
#[derive(Debug, Clone)]
pub struct GenericChoicePoint<V, C>
where
    V: MettaValueTrait + Clone + Send + Sync + 'static,
{
    /// Saved value stack height
    pub value_stack_height: usize,
    /// Saved call stack height
    pub call_stack_height: usize,
    /// Saved bindings stack height
    pub bindings_stack_height: usize,
    /// Continuation instruction pointer
    pub ip: usize,
    /// Continuation chunk
    pub chunk: Arc<C>,
    /// Remaining alternatives to try
    pub alternatives: Vec<GenericAlternative<V, C>>,
}

// ============================================================================
// Type Aliases for Backwards Compatibility
// ============================================================================

// Note: The concrete types `BindingFrame`, `Alternative`, `ChoicePoint`, and
// `CallFrame` are defined above as non-generic types for backwards compatibility.
// When fully migrated, they can become:
//
// pub type BindingFrame = GenericBindingFrame<MettaValue>;
// pub type Alternative = GenericAlternative<MettaValue, BytecodeChunk>;
// pub type ChoicePoint = GenericChoicePoint<MettaValue, BytecodeChunk>;
// pub type CallFrame = GenericCallFrame<BytecodeChunk>;

/// Type alias for heap-based binding frame (explicit generic usage)
pub type HeapBindingFrame = GenericBindingFrame<MettaValue>;

/// Type alias for heap-based alternative (explicit generic usage)
pub type HeapAlternative = GenericAlternative<MettaValue, BytecodeChunk>;

/// Type alias for heap-based choice point (explicit generic usage)
pub type HeapChoicePoint = GenericChoicePoint<MettaValue, BytecodeChunk>;

/// Type alias for heap-based call frame (explicit generic usage)
pub type HeapCallFrame = GenericCallFrame<BytecodeChunk>;
