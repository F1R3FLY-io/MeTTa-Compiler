//! JIT Type Definitions
//!
//! This module defines the core types used by the Cranelift JIT compiler:
//! - [`JitValue`]: NaN-boxed 64-bit value representation
//! - [`JitContext`]: Runtime context passed to compiled code
//! - [`JitResult`] and [`JitError`]: Result types for JIT operations
//! - [`JitChoicePoint`] and [`JitAlternative`]: Nondeterminism support
//! - [`JitBindingEntry`] and [`JitBindingFrame`]: Variable binding support
//! - [`JitClosure`]: Lambda closure representation

mod binding;
mod closure;
mod constants;
mod context;
mod error;
mod nondet;
mod value;

#[cfg(test)]
mod tests;

// Re-export constants
pub use constants::{
    JIT_SIGNAL_BAILOUT,
    JIT_SIGNAL_ERROR,
    JIT_SIGNAL_FAIL,
    JIT_SIGNAL_HALT,
    // JIT signals
    JIT_SIGNAL_OK,
    JIT_SIGNAL_YIELD,
    // Choice point constants
    MAX_ALTERNATIVES_INLINE,
    MAX_STACK_SAVE_VALUES,
    PAYLOAD_MASK,
    STACK_SAVE_POOL_SIZE,
    STATE_CACHE_MASK,
    // Cache constants
    STATE_CACHE_SIZE,
    // NaN-boxing tags
    TAG_ATOM,
    TAG_BOOL,
    TAG_ERROR,
    TAG_HEAP,
    TAG_LONG,
    TAG_MASK,
    TAG_NIL,
    TAG_UNIT,
    TAG_VAR,
    VAR_INDEX_CACHE_SIZE,
};

// Re-export binding types
pub use binding::{JitBindingEntry, JitBindingFrame};

// Re-export closure type
pub use closure::JitClosure;

// Re-export context
pub use context::JitContext;

// Re-export error types
pub use error::{JitError, JitResult};

// Re-export nondeterminism types
pub use nondet::{JitAlternative, JitAlternativeTag, JitBailoutReason, JitChoicePoint};

// Re-export value type
pub use value::JitValue;
