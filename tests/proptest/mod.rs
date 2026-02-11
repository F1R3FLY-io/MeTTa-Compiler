//! Property-based testing for MeTTa language compilation
//!
//! This module provides comprehensive property-based tests for the MeTTa compiler
//! using the `proptest` framework. It verifies that the compiler correctly handles
//! both valid and invalid MeTTa syntax across various language constructs.

pub mod compiling;
pub mod eval;
pub mod evaluating;

pub mod strategies;
