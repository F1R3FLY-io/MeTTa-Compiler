//! Non-determinism types for JIT execution.
//!
//! This module defines types for managing non-deterministic choice points:
//! - [`JitBailoutReason`]: Error codes for bailout
//! - [`JitAlternativeTag`]: Type tag for alternatives
//! - [`JitAlternative`]: Alternative in a choice point
//! - [`JitChoicePoint`]: Choice point for backtracking

use super::binding::{JitBindingEntry, JitBindingFrame};
use super::constants::MAX_ALTERNATIVES_INLINE;
use super::value::JitValue;

// =============================================================================
// JitBailoutReason - Error codes for bailout
// =============================================================================

/// Reason for JIT code bailing out to the bytecode VM.
///
/// This is `#[repr(u8)]` for efficient storage and FFI compatibility.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitBailoutReason {
    /// No bailout occurred
    None = 0,
    /// Type error (expected different type)
    TypeError = 1,
    /// Division by zero
    DivisionByZero = 2,
    /// Stack overflow
    StackOverflow = 3,
    /// Stack underflow
    StackUnderflow = 4,
    /// Invalid opcode encountered
    InvalidOpcode = 5,
    /// Unsupported operation (requires bytecode VM)
    UnsupportedOperation = 6,
    /// Integer overflow
    IntegerOverflow = 7,
    /// Non-determinism (Fork/Choice opcodes require VM)
    NonDeterminism = 8,
    /// Call operation needs VM for rule dispatch
    Call = 9,
    /// TailCall operation needs VM for rule dispatch
    TailCall = 10,
    /// Fork operation needs VM for choice point management
    Fork = 11,
    /// Yield operation needs VM for backtracking
    Yield = 12,
    /// Collect operation needs VM to gather results
    Collect = 13,
    /// Invalid binding (variable not found in any scope)
    InvalidBinding = 14,
    /// Binding frame stack overflow
    BindingFrameOverflow = 15,
    /// Higher-order operation (map, filter, fold) needs VM
    HigherOrderOp = 16,
}

// =============================================================================
// JIT Choice Point Types for Non-Determinism
// =============================================================================

/// Type tag for JitAlternative
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitAlternativeTag {
    /// Alternative is a NaN-boxed value to push
    Value = 0,
    /// Alternative is a pointer to a BytecodeChunk to execute
    Chunk = 1,
    /// Alternative is a rule match (chunk + bindings pointer)
    RuleMatch = 2,
    /// Alternative is a space match result (template + bindings + saved frames)
    /// - payload: NaN-boxed template expression pointer
    /// - payload2: pointer to bindings array (JitBindingEntry*)
    /// - payload3: pointer to saved binding frames for restoration
    SpaceMatch = 3,
}

/// An alternative in a JIT choice point.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Each alternative represents one branch in a non-deterministic choice.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JitAlternative {
    /// Type tag indicating what kind of alternative this is
    pub tag: JitAlternativeTag,
    /// Primary payload - interpretation depends on tag:
    /// - Value: NaN-boxed JitValue bits
    /// - Chunk: pointer to BytecodeChunk
    /// - RuleMatch: pointer to BytecodeChunk
    /// - SpaceMatch: NaN-boxed template expression pointer
    pub payload: u64,
    /// Secondary payload:
    /// - RuleMatch: pointer to Bindings
    /// - SpaceMatch: pointer to bindings array (JitBindingEntry*)
    pub payload2: u64,
    /// Tertiary payload (only used for SpaceMatch):
    /// - SpaceMatch: pointer to saved binding frames for restoration
    pub payload3: u64,
}

impl JitAlternative {
    /// Create a value alternative
    #[inline]
    pub fn value(val: JitValue) -> Self {
        Self {
            tag: JitAlternativeTag::Value,
            payload: val.to_bits(),
            payload2: 0,
            payload3: 0,
        }
    }

    /// Create a chunk alternative
    #[inline]
    pub fn chunk(chunk_ptr: *const ()) -> Self {
        Self {
            tag: JitAlternativeTag::Chunk,
            payload: chunk_ptr as u64,
            payload2: 0,
            payload3: 0,
        }
    }

    /// Create a rule match alternative
    #[inline]
    pub fn rule_match(chunk_ptr: *const (), bindings_ptr: *const ()) -> Self {
        Self {
            tag: JitAlternativeTag::RuleMatch,
            payload: chunk_ptr as u64,
            payload2: bindings_ptr as u64,
            payload3: 0,
        }
    }

    /// Create a space match alternative
    ///
    /// # Arguments
    /// - `template_ptr`: Pointer to the template expression to instantiate
    /// - `bindings_ptr`: Pointer to bindings array (JitBindingEntry*)
    /// - `saved_frames_ptr`: Pointer to saved binding frames for restoration
    #[inline]
    pub fn space_match(
        template_ptr: *const (),
        bindings_ptr: *const JitBindingEntry,
        saved_frames_ptr: *const JitBindingFrame,
    ) -> Self {
        Self {
            tag: JitAlternativeTag::SpaceMatch,
            payload: template_ptr as u64,
            payload2: bindings_ptr as u64,
            payload3: saved_frames_ptr as u64,
        }
    }
}

/// A JIT choice point for native non-determinism.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Choice points are created by Fork opcodes and consumed by Fail/Yield.
///
/// # Optimization 5.2: Pre-allocation
///
/// Alternatives are embedded inline to avoid per-fork allocation.
/// Saved stack uses a pool index instead of leaked Box allocation.
/// This eliminates memory leaks and reduces allocation overhead.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct JitChoicePoint {
    /// Saved stack pointer (for restoring on backtrack)
    pub saved_sp: u64,
    /// Number of alternatives in this choice point (max MAX_ALTERNATIVES_INLINE)
    pub alt_count: u64,
    /// Current alternative index (0..alt_count)
    pub current_index: u64,
    /// Saved instruction pointer for continuation
    pub saved_ip: u64,
    /// Pointer to saved chunk (for chunk switching)
    pub saved_chunk: *const (),
    /// Number of saved stack values (max MAX_STACK_SAVE_VALUES)
    pub saved_stack_count: usize,
    /// Fork depth at creation (for nested nondeterminism)
    pub fork_depth: usize,
    // Phase 1.4: Fields for enhanced backtracking
    /// Saved binding frames count for nested scope restoration
    pub saved_binding_frames_count: usize,
    /// Whether this choice point is a Collect boundary
    /// When true, backtracking from this point should collect results
    pub is_collect_boundary: bool,

    // Optimization 5.2: Embedded alternatives (eliminates Box::leak allocation)
    /// Inline array of alternatives (avoids heap allocation per Fork)
    pub alternatives_inline: [JitAlternative; MAX_ALTERNATIVES_INLINE],

    // Optimization 5.2: Pool-based stack save (eliminates Box::leak allocation)
    /// Index into JitContext.stack_save_pool (-1 = no saved stack)
    /// Using isize to allow -1 sentinel for "no saved stack"
    pub saved_stack_pool_idx: isize,
}

impl Default for JitChoicePoint {
    fn default() -> Self {
        Self {
            saved_sp: 0,
            alt_count: 0,
            current_index: 0,
            saved_ip: 0,
            saved_chunk: std::ptr::null(),
            saved_stack_count: 0,
            fork_depth: 0,
            saved_binding_frames_count: 0,
            is_collect_boundary: false,
            // Initialize all alternatives to empty value alternatives
            alternatives_inline: [JitAlternative::value(JitValue::nil()); MAX_ALTERNATIVES_INLINE],
            saved_stack_pool_idx: -1, // No saved stack
        }
    }
}
