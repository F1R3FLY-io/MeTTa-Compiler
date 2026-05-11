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
//! both heap-allocated (`MettaValue`) and arena-allocated (`MettaValue`) values.
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

use crate::backend::bytecode::chunk::{BytecodeChunk, GenericBytecodeChunk};
use crate::backend::models::{GenericBindings, MettaValue, MettaValueTrait};

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
    /// Guard evaluation failed (triggers backtracking)
    GuardFailed,
}

impl VmError {
    /// Returns true if this VmError represents a *runtime data error*
    /// (TypeError, DivisionByZero, ArithmeticOverflow) — these should be
    /// converted to first-class Error atoms on the value stack per
    /// HE-aligned errors-as-values semantics (spec K T1.A).
    ///
    /// Returns false for genuinely-internal VM failures (StackUnderflow,
    /// InvalidOpcode, IpOutOfBounds, etc.) which must continue to propagate
    /// as Rust `Err` for the host to handle.
    pub fn is_runtime_data_error(&self) -> bool {
        matches!(
            self,
            Self::TypeError { .. } | Self::DivisionByZero | Self::ArithmeticOverflow
        )
    }

    /// Format a runtime-data error as a `(msg, details_atom)` pair that the
    /// VM uses to build a `(Error msg details)` atom. Spec K T1.A.
    pub fn as_error_strings(&self) -> (String, &'static str) {
        match self {
            Self::TypeError { expected, got } => (
                format!("Type error: expected {}, got {}", expected, got),
                "BadType",
            ),
            Self::DivisionByZero => {
                ("Division by zero".to_string(), "DivisionByZero")
            }
            Self::ArithmeticOverflow => {
                ("Arithmetic overflow".to_string(), "ArithmeticOverflow")
            }
            _ => ("Unexpected error".to_string(), "Unknown"),
        }
    }
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
            Self::GuardFailed => write!(f, "Guard evaluation failed"),
        }
    }
}

impl std::error::Error for VmError {}

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
/// - `V`: The value type (e.g., `MettaValue` or `MettaValue`)
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

    /// Remove a binding by name. Returns the removed value if it existed.
    #[inline]
    pub fn remove(&mut self, name: &str) -> Option<V> {
        if let Some(pos) = self.bindings.iter().position(|(n, _)| n == name) {
            Some(self.bindings.remove(pos).1)
        } else {
            None
        }
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
/// - `V`: The value type (e.g., `MettaValue` or `MettaValue`)
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
    /// Phase 1b-A: a value paired with the bindings under which it was
    /// produced. Restored together on backtrack so nondeterministic
    /// fanout (e.g. multiple rule matches for a sub-expression) carries
    /// each alternative's ambient binding context. Mirrors HE's
    /// `InterpretedAtom(Stack, Bindings)` shape at the VM level.
    BoundValue {
        /// The produced value
        value: V,
        /// Bindings active for this alternative
        bindings: GenericBindings<V>,
    },
}

/// Generic call frame on the call stack.
///
/// # Type Parameters
///
/// - `C`: The bytecode chunk type
#[derive(Debug, Clone)]
pub struct GenericCallFrame<V, C>
where
    V: MettaValueTrait + Clone + Send + Sync + 'static,
{
    /// Return instruction pointer
    pub return_ip: usize,
    /// Return chunk
    pub return_chunk: Arc<C>,
    /// Base pointer into value stack
    pub base_ptr: usize,
    /// Base pointer into bindings stack
    pub bindings_base: usize,
    /// When true, returning from this frame yields the result to `self.results`
    /// and backtracks via `op_fail` instead of continuing to the calling chunk.
    /// Used for outermost nondeterministic dispatch (multi-match DispatchRules).
    pub yield_on_return: bool,
    /// Phase 1b-A: caller's `current_bindings` captured at call time. Phase 1b-F
    /// composes it back with the frame's final bindings on Return so that
    /// bindings established during RHS execution (e.g. by deeper rule
    /// dispatches) propagate up to the caller's ambient context. Mirrors
    /// HE's `Stack` frame-level binding retention.
    pub saved_bindings: GenericBindings<V>,
    /// Bug-fix 2026-04-follow-up: caller's `self.locals_base` at call time.
    /// On return, `self.locals_base = frame.locals_base` restores the caller's
    /// local-slot namespace. The vector length itself must restore to
    /// `caller_locals_len`, because caller locals can live above `locals_base`.
    pub locals_base: usize,
    /// Bug-fix 2026-05 (within-query cache leak): `self.locals.len()` AT
    /// the moment this frame was pushed. When a `BoundValue` choice
    /// point installed inside this frame backtracks via the
    /// "pre-pop frame" path (`cp_install_coords_for_bound_value`), the
    /// CP's `locals_height` must drop everything the callee added —
    /// which is exactly this snapshot, NOT `self.locals.len()` at the
    /// time the CP was installed (callee locals were added after frame
    /// push). Without this, callee local slots survive backtrack and
    /// pollute the outer chunk's local namespace.
    pub caller_locals_len: usize,
    /// Bug-fix 2026-05: `self.trail.len()` at frame push time. CP backtrack
    /// past this frame must rewind the trail to here so all bindings
    /// the callee made are undone — mirroring what `op_return`'s
    /// composition of `frame.saved_bindings ∘ self.current_bindings`
    /// would have done conceptually for non-conflicting bindings, but
    /// for CP-on-backtrack we replace the inner with the per-alt's
    /// own bindings, so the trail entries from inner execution are
    /// stale.
    pub caller_trail_len: usize,
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
    /// Saved `unreduced` flag — prevents inner dispatches from polluting
    /// the outer unreduced state during nondeterministic backtracking.
    pub saved_unreduced: bool,
    /// Saved trail height for fine-grained binding undo during backtracking.
    ///
    /// When backtracking, the trail is unwound from its current length back to
    /// this saved height, undoing individual bindings that were made since the
    /// choice point was created. This complements the coarse bindings_stack
    /// truncation with fine-grained undo within surviving frames.
    pub trail_height: usize,
    /// Phase 1b-E3: saved VM `current_bindings` at the moment the
    /// choice point was created. On backtrack, `op_fail` restores
    /// this as the default ambient; if the picked alternative is a
    /// `BoundValue`, that alt's bindings then OVERWRITE this default.
    /// Without this, bindings established after the choice point was
    /// pushed would leak into the alt-branch evaluation, violating
    /// HE's per-alt (Stack, Bindings) semantics.
    pub saved_current_bindings: GenericBindings<V>,
    /// Bug-fix 2026-04-follow-up: saved `self.locals.len()` at CP push time.
    /// On backtrack, `op_fail*` truncates `self.locals` to this height so
    /// a CP that was pushed during callee execution and is being resumed
    /// past the call frame correctly restores the caller's local scope.
    pub locals_height: usize,
    /// Bug-fix 2026-04-follow-up: saved `self.locals_base` at CP push time.
    /// When a CP is resumed and its corresponding call frame has been popped
    /// (via `call_stack.truncate(cp.call_stack_height)` in `op_fail`), we
    /// cannot reach the caller's `locals_base` through the call stack —
    /// so we stash it directly on the CP.
    pub locals_base_at_cp: usize,
}

// ============================================================================
// Trail Entry
// ============================================================================

/// A trail entry recording a binding that was made, enabling undo on backtrack.
///
/// Trail entries are cheap: V is typically Copy (MettaValue is 8 bytes).
/// The trail is unwound in reverse order during `op_fail` to restore the
/// binding state from before a failed nondeterministic branch.
#[derive(Debug, Clone)]
pub enum TrailEntry<V: MettaValueTrait + Clone> {
    /// A variable was bound for the first time (was previously unbound).
    /// To undo: remove the binding from the specified frame.
    NewBinding {
        /// Index into the bindings_stack
        frame_index: usize,
        /// Variable name.
        name: String,
    },
    /// A variable was rebound (had a previous value).
    /// To undo: restore the old value in the specified frame.
    Rebinding {
        /// Index into the bindings_stack
        frame_index: usize,
        /// Variable name.
        name: String,
        /// The previous value to restore
        old_value: V,
    },
}

// ============================================================================
// Concrete Type Aliases
// ============================================================================

/// Binding frame for pattern variables (concrete type alias).
pub type BindingFrame = GenericBindingFrame<MettaValue>;

/// An alternative in a choice point (concrete type alias).
pub type Alternative = GenericAlternative<MettaValue, BytecodeChunk>;

/// Choice point for nondeterminism (concrete type alias).
pub type ChoicePoint = GenericChoicePoint<MettaValue, BytecodeChunk>;

/// Collapse frame for nondeterminism sandboxing.
///
/// Saves the outer nondeterministic context when entering a `(collapse ...)` scope.
/// Backtracking within the collapse body cannot escape past the barrier.
#[derive(Debug, Clone)]
pub struct GenericCollapseFrame<V: MettaValueTrait + Clone + Send + Sync + 'static> {
    /// Saved outer results vector (swapped out during collapse body)
    pub saved_results: Vec<V>,
    /// Choice point stack height at collapse entry — backtracking barrier
    pub choice_point_base: usize,
    /// Monotonic VM-local id for ordering nested failure barriers.
    pub barrier_id: u64,
    /// Value stack height at collapse entry (for cleanup)
    pub value_stack_height: usize,
    /// IP to resume at after collapse completes (instruction after CollapseEnd)
    pub continuation_ip: usize,
    /// Chunk to resume in (may differ from current chunk after backtracking)
    pub continuation_chunk: Arc<GenericBytecodeChunk<V>>,
}

/// Collapse frame (concrete type alias).
pub type CollapseFrame = GenericCollapseFrame<MettaValue>;

/// Phase C: `collapse-bind` scope frame.
///
/// Superset of [`GenericCollapseFrame`] that preserves per-result bindings
/// alongside values so the scope can emit `(value (Bindings ($var val) …))`
/// pairs at its end. `tracked_vars` filters the sidecar-encoded bindings
/// to just the user-visible set at the observation point (matching HE's
/// `Bindings::resolve()` semantics).
///
/// `continuation_ip` is set *lazily* on the first reach of `CollapseBindEnd`
/// (unlike `CollapseFrame` which sets it eagerly via `CollapseBegin`'s jump
/// operand) because `CollapseBindBegin` carries only `tracked_vars_idx` in
/// its immediate. On subsequent op_collapse_bind_end entries (after back-
/// tracking re-runs the body), the value is already set and preserved.
#[derive(Debug, Clone)]
pub struct GenericCollapseBindFrame<V: MettaValueTrait + Clone + Send + Sync + 'static> {
    /// Saved outer results vector (swapped out during collapse-bind body).
    pub saved_results: Vec<V>,
    /// Saved outer per-result bindings (parallel to `saved_results`).
    pub saved_per_result_bindings: Vec<GenericBindings<V>>,
    /// Choice point stack height at entry — backtracking barrier.
    pub choice_point_base: usize,
    /// Value stack height at entry (for cleanup on exhaustion).
    pub value_stack_height: usize,
    /// IP to resume after the `collapse-bind` scope ends (lazily set on
    /// first `CollapseBindEnd` entry; 0 until then).
    pub continuation_ip: usize,
    /// Chunk to resume in after the scope ends (lazily set).
    pub continuation_chunk: Option<Arc<GenericBytecodeChunk<V>>>,
    /// Free-variable names to project bindings to (the sidecar whitelist).
    /// An empty vector means "project to empty" (no bindings visible); a
    /// `None` would mean "don't project" but we always project — the
    /// compile-time `compile_collapse_bind` currently emits an empty
    /// placeholder; Phase C completion computes it from expr free vars.
    pub tracked_vars: Vec<&'static str>,
}

/// Collapse-bind frame (concrete type alias).
pub type CollapseBindFrame = GenericCollapseBindFrame<MettaValue>;

/// Call frame on the call stack (concrete type alias).
pub type CallFrame = GenericCallFrame<MettaValue, BytecodeChunk>;

/// Phase 1b-A: VM-level `InterpretedAtom` — a produced value paired with
/// the bindings under which it was produced. Matches HE's
/// `InterpretedAtom(Stack, Bindings)` at `interpreter.rs:145`. Used as
/// the return type of template helpers and as the unit of exchange
/// across applicative pre-eval boundaries in the VM.
pub type VmBoundValue<V> = (V, GenericBindings<V>);
