//! Type predicate runtime functions for JIT compilation
//!
//! This module provides FFI-callable type checking predicates:
//! - is_long - Check if value is a Long
//! - is_bool - Check if value is a Bool
//! - is_nil - Check if value is unit (ABI name kept for backward compat)
//! - get_tag - Get the type tag as an integer

use super::helpers::box_long;
use crate::backend::bytecode::jit::types::{TAG_BOOL, TAG_LONG, TAG_MASK, TAG_UNIT};

// =============================================================================
// Type Checking Runtime
// =============================================================================

/// Check if value is a Long
#[no_mangle]
pub extern "C" fn jit_runtime_is_long(val: u64) -> u64 {
    let is_long = (val & TAG_MASK) == TAG_LONG;
    TAG_BOOL | (is_long as u64)
}

/// Check if value is a Bool
#[no_mangle]
pub extern "C" fn jit_runtime_is_bool(val: u64) -> u64 {
    let is_bool = (val & TAG_MASK) == TAG_BOOL;
    TAG_BOOL | (is_bool as u64)
}

/// Check if value is unit (TAG_UNIT).
#[no_mangle]
pub extern "C" fn jit_runtime_is_unit(val: u64) -> u64 {
    let tag = val & TAG_MASK;
    let is_unit = tag == TAG_UNIT;
    TAG_BOOL | (is_unit as u64)
}

/// Get the type tag as an integer (for switch statements)
#[no_mangle]
pub extern "C" fn jit_runtime_get_tag(val: u64) -> u64 {
    let tag = (val & TAG_MASK) >> 48;
    box_long(tag as i64)
}
