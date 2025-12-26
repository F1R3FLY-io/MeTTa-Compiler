//! JIT compilation infrastructure for MeTTa bytecode
//!
//! This module provides Just-In-Time compilation support for frequently
//! executed bytecode sequences, improving performance for hot paths.
//!
//! # Module Structure
//!
//! - `types`: Core type definitions for JIT values and bindings
//! - `profile`: Runtime profiling for tiered compilation decisions

pub mod profile;
pub mod types;

pub use profile::JitProfile;
pub use types::{
    JitBindingEntry, JitBindingFrame, JitClosure, JitContext, JitError, JitResult, JitValue,
};
