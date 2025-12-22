//! JIT Runtime Support Functions
//!
//! This module provides runtime helper functions that can be called from
//! JIT-compiled code. These handle operations that are too complex to inline
//! or require access to Rust runtime features.
//!
//! # Calling Convention
//!
//! All runtime functions use the C ABI (`extern "C"`) for stable calling
//! from generated machine code. They take raw pointers and return raw values.

use super::types::{
    JitBailoutReason, JitContext, JitValue, JitChoicePoint, JitAlternative, JitAlternativeTag,
    TAG_ERROR, TAG_LONG, TAG_HEAP, TAG_ATOM, TAG_VAR, TAG_BOOL, TAG_NIL, TAG_UNIT, PAYLOAD_MASK,
    // Stage 2: Signal constants for native nondeterminism
    JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL, JIT_SIGNAL_ERROR,
};
use crate::backend::models::{MettaValue, Bindings};
use crate::backend::bytecode::mork_bridge::{MorkBridge, CompiledRule};
use crate::backend::bytecode::chunk::BytecodeChunk;
use crate::backend::bytecode::vm::BytecodeVM;
use crate::backend::bytecode::external_registry::{ExternalRegistry, ExternalContext};
use crate::backend::eval::{apply_bindings, pattern_match};
use std::sync::Arc;

// =============================================================================
// Error Handling Runtime
// =============================================================================

/// Runtime function called on type error
///
/// Sets the bailout flag in the context and records the error location.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_type_error(ctx: *mut JitContext, ip: u64, _expected: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::TypeError;
    }
}

/// Runtime function called on division by zero
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_div_by_zero(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::DivisionByZero;
    }
}

/// Runtime function called on stack overflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_stack_overflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::StackOverflow;
    }
}

/// Runtime function called on stack underflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_stack_underflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::StackUnderflow;
    }
}

// =============================================================================
// Arithmetic Runtime Helpers
// =============================================================================

/// Compute integer power: base^exp
///
/// Handles negative exponents by returning 0 (integer division truncation).
///
/// # Safety
/// The inputs must be valid NaN-boxed Long values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pow(base: u64, exp: u64) -> u64 {
    // Extract the actual values from NaN-boxed representation
    let base_val = extract_long_signed(base);
    let exp_val = extract_long_signed(exp);

    let result = if exp_val < 0 {
        // Integer power with negative exponent is 0 (except for base=1 or base=-1)
        match base_val {
            1 => 1,
            -1 => {
                if exp_val % 2 == 0 {
                    1
                } else {
                    -1
                }
            }
            _ => 0,
        }
    } else if exp_val == 0 {
        1
    } else {
        // Use binary exponentiation for efficiency
        let mut result: i64 = 1;
        let mut base = base_val;
        let mut exp = exp_val as u64;

        while exp > 0 {
            if exp & 1 == 1 {
                result = result.wrapping_mul(base);
            }
            base = base.wrapping_mul(base);
            exp >>= 1;
        }
        result
    };

    // Box result as Long
    box_long(result)
}

/// Integer absolute value
///
/// # Safety
/// The input must be a valid NaN-boxed Long value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_abs(val: u64) -> u64 {
    let n = extract_long_signed(val);
    box_long(n.abs())
}

/// Integer sign function: returns -1, 0, or 1
///
/// # Safety
/// The input must be a valid NaN-boxed Long value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_signum(val: u64) -> u64 {
    let n = extract_long_signed(val);
    let result = if n < 0 { -1 } else if n > 0 { 1 } else { 0 };
    box_long(result)
}

// =============================================================================
// Type Checking Runtime
// =============================================================================

/// Check if value is a Long
#[no_mangle]
pub extern "C" fn jit_runtime_is_long(val: u64) -> u64 {
    let is_long = (val & super::types::TAG_MASK) == TAG_LONG;
    super::types::TAG_BOOL | (is_long as u64)
}

/// Check if value is a Bool
#[no_mangle]
pub extern "C" fn jit_runtime_is_bool(val: u64) -> u64 {
    let is_bool = (val & super::types::TAG_MASK) == super::types::TAG_BOOL;
    super::types::TAG_BOOL | (is_bool as u64)
}

/// Check if value is nil
#[no_mangle]
pub extern "C" fn jit_runtime_is_nil(val: u64) -> u64 {
    let is_nil = (val & super::types::TAG_MASK) == super::types::TAG_NIL;
    super::types::TAG_BOOL | (is_nil as u64)
}

/// Get the type tag as an integer (for switch statements)
#[no_mangle]
pub extern "C" fn jit_runtime_get_tag(val: u64) -> u64 {
    let tag = (val & super::types::TAG_MASK) >> 48;
    box_long(tag as i64)
}

// =============================================================================
// Type Operations Runtime (Phase 1 JIT)
// =============================================================================

// Static type name strings for efficient atom creation
// These are leaked to get 'static lifetimes that survive JIT code
static TYPE_NAME_NUMBER: &str = "Number";
static TYPE_NAME_BOOL: &str = "Bool";
static TYPE_NAME_NIL: &str = "Nil";
static TYPE_NAME_UNIT: &str = "Unit";
static TYPE_NAME_EXPRESSION: &str = "Expression";
static TYPE_NAME_ERROR: &str = "Error";
static TYPE_NAME_SYMBOL: &str = "Symbol";
static TYPE_NAME_VARIABLE: &str = "Variable";
static TYPE_NAME_STRING: &str = "String";
static TYPE_NAME_TYPE: &str = "Type";
static TYPE_NAME_CONJUNCTION: &str = "Conjunction";
static TYPE_NAME_SPACE: &str = "Space";
static TYPE_NAME_STATE: &str = "State";
static TYPE_NAME_MEMO: &str = "Memo";
static TYPE_NAME_EMPTY: &str = "Empty";
static TYPE_NAME_UNKNOWN: &str = "Unknown";

/// Get the type name of a NaN-boxed value.
///
/// Returns the type name as a NaN-boxed atom (TAG_ATOM with pointer to static string).
/// Type names match MettaValue::type_name():
/// - TAG_LONG → "Number"
/// - TAG_BOOL → "Bool"
/// - TAG_NIL → "Nil"
/// - TAG_UNIT → "Unit"
/// - TAG_HEAP → depends on heap value type
/// - TAG_ERROR → "Error"
/// - TAG_ATOM → "Symbol" (or "Variable" if starts with $)
/// - TAG_VAR → "Variable"
///
/// # Safety
/// For heap pointers, the referenced MettaValue must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_type(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let tag = val & super::types::TAG_MASK;

    let type_name: &'static str = match tag {
        super::types::TAG_LONG => TYPE_NAME_NUMBER,
        super::types::TAG_BOOL => TYPE_NAME_BOOL,
        super::types::TAG_NIL => TYPE_NAME_NIL,
        super::types::TAG_UNIT => TYPE_NAME_UNIT,
        super::types::TAG_ERROR => TYPE_NAME_ERROR,
        super::types::TAG_VAR => TYPE_NAME_VARIABLE,
        super::types::TAG_ATOM => {
            // Check if it's a variable (starts with $)
            let ptr = (val & PAYLOAD_MASK) as *const String;
            if !ptr.is_null() {
                let s = &*ptr;
                if s.starts_with('$') {
                    TYPE_NAME_VARIABLE
                } else {
                    TYPE_NAME_SYMBOL
                }
            } else {
                TYPE_NAME_SYMBOL
            }
        }
        super::types::TAG_HEAP => {
            // Need to inspect the heap value
            let ptr = (val & PAYLOAD_MASK) as *const MettaValue;
            if ptr.is_null() {
                TYPE_NAME_UNKNOWN
            } else {
                match &*ptr {
                    MettaValue::SExpr(_) => TYPE_NAME_EXPRESSION,
                    MettaValue::String(_) => TYPE_NAME_STRING,
                    MettaValue::Type(_) => TYPE_NAME_TYPE,
                    MettaValue::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                    MettaValue::Space(_) => TYPE_NAME_SPACE,
                    MettaValue::State(_) => TYPE_NAME_STATE,
                    MettaValue::Memo(_) => TYPE_NAME_MEMO,
                    MettaValue::Empty => TYPE_NAME_EMPTY,
                    MettaValue::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                    MettaValue::Atom(_) => TYPE_NAME_SYMBOL,
                    MettaValue::Bool(_) => TYPE_NAME_BOOL,
                    MettaValue::Long(_) | MettaValue::Float(_) => TYPE_NAME_NUMBER,
                    MettaValue::Nil => TYPE_NAME_NIL,
                    MettaValue::Error(_, _) => TYPE_NAME_ERROR,
                    MettaValue::Unit => TYPE_NAME_UNIT,
                }
            }
        }
        _ => TYPE_NAME_UNKNOWN,
    };

    // Return as a Symbol (heap-allocated MettaValue::Atom)
    // We create a new MettaValue::Atom and return it as a heap pointer
    let atom = Box::new(MettaValue::Atom(type_name.to_string()));
    let ptr = Box::into_raw(atom);
    super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Check if a value's type matches an expected type.
///
/// Pops a type name (as atom) and compares it with the value's type.
/// Returns a NaN-boxed Bool (true if matches, false otherwise).
///
/// Special case: Type variables (starting with $) match any type.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `val` - The value to check (NaN-boxed)
/// * `type_atom` - The expected type as an atom/symbol (NaN-boxed)
/// * `ip` - Current instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed Bool: true if types match, false otherwise.
///
/// # Safety
/// The context pointer must be valid. Atom pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_check_type(
    ctx: *mut JitContext,
    val: u64,
    type_atom: u64,
    _ip: u64,
) -> u64 {
    // Extract the expected type name from the type_atom
    let expected_type: Option<&str> = {
        let type_tag = type_atom & super::types::TAG_MASK;
        match type_tag {
            super::types::TAG_ATOM => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const String;
                if !ptr.is_null() {
                    Some((&*ptr).as_str())
                } else {
                    None
                }
            }
            super::types::TAG_HEAP => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const MettaValue;
                if !ptr.is_null() {
                    if let MettaValue::Atom(s) = &*ptr {
                        Some(s.as_str())
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    };

    let Some(expected) = expected_type else {
        // Type atom is not a valid symbol - signal error
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(_ip as usize, JitBailoutReason::TypeError);
        }
        return super::types::TAG_BOOL; // false
    };

    // Type variables match anything
    if expected.starts_with('$') {
        return super::types::TAG_BOOL | 1; // true
    }

    // Get the actual type of the value
    let actual_type = jit_runtime_get_type_name(val);

    // Compare types
    let matches = actual_type == expected;
    super::types::TAG_BOOL | (matches as u64)
}

/// Internal helper: Get the type name as a string slice (not exported)
unsafe fn jit_runtime_get_type_name(val: u64) -> &'static str {
    let tag = val & super::types::TAG_MASK;

    match tag {
        super::types::TAG_LONG => TYPE_NAME_NUMBER,
        super::types::TAG_BOOL => TYPE_NAME_BOOL,
        super::types::TAG_NIL => TYPE_NAME_NIL,
        super::types::TAG_UNIT => TYPE_NAME_UNIT,
        super::types::TAG_ERROR => TYPE_NAME_ERROR,
        super::types::TAG_VAR => TYPE_NAME_VARIABLE,
        super::types::TAG_ATOM => {
            let ptr = (val & PAYLOAD_MASK) as *const String;
            if !ptr.is_null() {
                let s = &*ptr;
                if s.starts_with('$') {
                    TYPE_NAME_VARIABLE
                } else {
                    TYPE_NAME_SYMBOL
                }
            } else {
                TYPE_NAME_SYMBOL
            }
        }
        super::types::TAG_HEAP => {
            let ptr = (val & PAYLOAD_MASK) as *const MettaValue;
            if ptr.is_null() {
                return TYPE_NAME_UNKNOWN;
            }
            match &*ptr {
                MettaValue::SExpr(_) => TYPE_NAME_EXPRESSION,
                MettaValue::String(_) => TYPE_NAME_STRING,
                MettaValue::Type(_) => TYPE_NAME_TYPE,
                MettaValue::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                MettaValue::Space(_) => TYPE_NAME_SPACE,
                MettaValue::State(_) => TYPE_NAME_STATE,
                MettaValue::Memo(_) => TYPE_NAME_MEMO,
                MettaValue::Empty => TYPE_NAME_EMPTY,
                MettaValue::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                MettaValue::Atom(_) => TYPE_NAME_SYMBOL,
                MettaValue::Bool(_) => TYPE_NAME_BOOL,
                MettaValue::Long(_) | MettaValue::Float(_) => TYPE_NAME_NUMBER,
                MettaValue::Nil => TYPE_NAME_NIL,
                MettaValue::Error(_, _) => TYPE_NAME_ERROR,
                MettaValue::Unit => TYPE_NAME_UNIT,
            }
        }
        _ => TYPE_NAME_UNKNOWN,
    }
}

/// Assert that a value's type matches the expected type.
///
/// Similar to check_type, but instead of returning a bool, this either:
/// - Returns the original value unchanged if types match
/// - Signals a bailout error if types don't match
///
/// Stack effect: [value, type_atom] -> [value] (if types match)
/// On mismatch: signals bailout with TypeError
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `val` - The value to check (NaN-boxed)
/// * `type_atom` - The expected type as an atom/symbol (NaN-boxed)
/// * `ip` - Current instruction pointer (for error reporting)
///
/// # Returns
/// The original value if types match, or signals bailout on mismatch.
///
/// # Safety
/// The context pointer must be valid. Atom pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_assert_type(
    ctx: *mut JitContext,
    val: u64,
    type_atom: u64,
    ip: u64,
) -> u64 {
    // Extract the expected type name from the type_atom
    let expected_type: Option<&str> = {
        let type_tag = type_atom & super::types::TAG_MASK;
        match type_tag {
            super::types::TAG_ATOM => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const String;
                if !ptr.is_null() {
                    Some((&*ptr).as_str())
                } else {
                    None
                }
            }
            super::types::TAG_HEAP => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const MettaValue;
                if !ptr.is_null() {
                    if let MettaValue::Atom(s) = &*ptr {
                        Some(s.as_str())
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    };

    let Some(expected) = expected_type else {
        // Type atom is not a valid symbol - signal error and return value anyway
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        return val;
    };

    // Type variables match anything
    if expected.starts_with('$') {
        return val;
    }

    // Get the actual type of the value
    let actual_type = jit_runtime_get_type_name(val);

    // Compare types
    if actual_type == expected {
        // Types match - return the original value
        val
    } else {
        // Type mismatch - signal bailout error
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        val // Return value anyway (bailout will handle the error)
    }
}

// =============================================================================
// Stack Operations Runtime
// =============================================================================

/// Push a value onto the JIT context's memory stack
///
/// Used when we need to materialize values to memory (e.g., for calls).
///
/// # Safety
/// The context pointer and stack must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push(ctx: *mut JitContext, val: u64) -> i32 {
    if let Some(ctx) = ctx.as_mut() {
        if ctx.sp >= ctx.stack_cap {
            return -1; // Stack overflow
        }
        *ctx.value_stack.add(ctx.sp) = JitValue::from_raw(val);
        ctx.sp += 1;
        0 // Success
    } else {
        -2 // Null context
    }
}

/// Pop a value from the JIT context's memory stack
///
/// # Safety
/// The context pointer and stack must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pop(ctx: *mut JitContext) -> u64 {
    if let Some(ctx) = ctx.as_mut() {
        if ctx.sp == 0 {
            // Stack underflow - return nil as sentinel
            return super::types::TAG_NIL;
        }
        ctx.sp -= 1;
        (*ctx.value_stack.add(ctx.sp)).to_bits()
    } else {
        super::types::TAG_NIL
    }
}

/// Get stack pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_sp(ctx: *const JitContext) -> u64 {
    if let Some(ctx) = ctx.as_ref() {
        ctx.sp as u64
    } else {
        0
    }
}

/// Set stack pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_set_sp(ctx: *mut JitContext, sp: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.sp = sp as usize;
    }
}

// =============================================================================
// Constant Pool Access
// =============================================================================

/// Load a constant from the constant pool
///
/// Returns the constant as a JitValue (boxing if necessary).
///
/// # Safety
/// The context pointer and constant index must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_constant(ctx: *const JitContext, index: u64) -> u64 {
    if let Some(ctx) = ctx.as_ref() {
        let idx = index as usize;
        if idx >= ctx.constants_len {
            // Invalid constant index - return nil
            return super::types::TAG_NIL;
        }

        let constant = &*ctx.constants.add(idx);

        // Try to NaN-box the constant
        match JitValue::try_from_metta(constant) {
            Some(jv) => jv.to_bits(),
            None => {
                // Can't NaN-box - return as heap pointer
                let ptr = constant as *const MettaValue;
                super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
            }
        }
    } else {
        super::types::TAG_NIL
    }
}

// =============================================================================
// Debugging Runtime
// =============================================================================

/// Print a JitValue for debugging
#[no_mangle]
pub extern "C" fn jit_runtime_debug_print(val: u64) {
    let jv = JitValue::from_raw(val);
    eprintln!("[JIT DEBUG] {:?}", jv);
}

/// Print the current stack for debugging
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_debug_stack(ctx: *const JitContext) {
    if let Some(ctx) = ctx.as_ref() {
        eprintln!("[JIT DEBUG] Stack (sp={}): ", ctx.sp);
        for i in 0..ctx.sp {
            let val = *ctx.value_stack.add(i);
            eprintln!("  [{}] {:?}", i, val);
        }
    }
}

// =============================================================================
// Helper Functions (not exported)
// =============================================================================

/// Extract signed 64-bit integer from NaN-boxed Long
fn extract_long_signed(val: u64) -> i64 {
    let payload = val & PAYLOAD_MASK;
    // Sign extend from 48 bits
    const SIGN_BIT: u64 = 0x0000_8000_0000_0000;
    if payload & SIGN_BIT != 0 {
        (payload | 0xFFFF_0000_0000_0000) as i64
    } else {
        payload as i64
    }
}

/// Box a signed 64-bit integer as NaN-boxed Long
fn box_long(n: i64) -> u64 {
    TAG_LONG | ((n as u64) & PAYLOAD_MASK)
}

// =============================================================================
// Non-Determinism Runtime (Choice Points)
// =============================================================================

/// Push a new choice point onto the choice point stack.
///
/// This is called by JIT code when executing a Fork opcode. The choice point
/// stores the current state so we can restore it during backtracking.
///
/// # Arguments
/// * `ctx` - Pointer to the JitContext
/// * `alt_count` - Number of alternatives in this choice point
/// * `alternatives` - Pointer to array of JitAlternative values
/// * `saved_ip` - Instruction pointer to resume at on backtrack
/// * `saved_chunk` - Pointer to chunk to switch to on backtrack
///
/// # Returns
/// * 0 on success
/// * -1 on choice point stack overflow
/// * -2 on null context
///
/// # Safety
/// The context pointer must be valid and have non-determinism support enabled.
/// The alternatives pointer must point to at least `alt_count` valid JitAlternative values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_choice_point(
    ctx: *mut JitContext,
    alt_count: u64,
    alternatives: *const JitAlternative,
    saved_ip: u64,
    saved_chunk: *const (),
) -> i64 {
    let Some(ctx) = ctx.as_mut() else {
        return -2; // Null context
    };

    if ctx.choice_points.is_null() || ctx.choice_point_count >= ctx.choice_point_cap {
        // No choice point support or stack overflow
        ctx.signal_error(saved_ip as usize, JitBailoutReason::StackOverflow);
        return -1;
    }

    // Optimization 5.2: Check if alternatives fit inline
    if alt_count as usize > super::MAX_ALTERNATIVES_INLINE {
        ctx.signal_error(saved_ip as usize, JitBailoutReason::Fork);
        return -3; // Too many alternatives
    }

    // Create the choice point
    let cp = &mut *ctx.choice_points.add(ctx.choice_point_count);
    cp.saved_sp = ctx.sp as u64;
    cp.alt_count = alt_count;
    cp.current_index = 0;
    cp.saved_ip = saved_ip;
    cp.saved_chunk = saved_chunk;
    cp.saved_stack_pool_idx = -1; // No stack save for this path
    cp.saved_stack_count = 0;

    // Optimization 5.2: Copy alternatives to inline array
    if !alternatives.is_null() {
        for i in 0..alt_count as usize {
            cp.alternatives_inline[i] = *alternatives.add(i);
        }
    }

    ctx.choice_point_count += 1;
    0 // Success
}

/// Backtrack to the next alternative.
///
/// This is called by JIT code when execution fails or when Yield is used.
/// It restores the state from the most recent choice point and returns
/// information about the next alternative to try.
///
/// # Returns
/// * Positive value: The tag of the next alternative (0=Value, 1=Chunk, 2=RuleMatch)
/// * -1: No more alternatives (all choice points exhausted)
/// * -2: Null context
///
/// When a positive value is returned:
/// * For Value (0): The value to push is stored in the current choice point
/// * For Chunk (1): The chunk pointer is in the alternative
/// * For RuleMatch (2): The chunk and bindings pointers are in the alternative
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fail(ctx: *mut JitContext) -> i64 {
    let Some(ctx) = ctx.as_mut() else {
        return -2; // Null context
    };

    // Search for a choice point with remaining alternatives
    while ctx.choice_point_count > 0 {
        let cp = &mut *ctx.choice_points.add(ctx.choice_point_count - 1);

        if cp.current_index < cp.alt_count {
            // Found an alternative - restore state
            ctx.sp = cp.saved_sp as usize;

            // Optimization 5.2: Get from inline alternatives array
            let alt = &cp.alternatives_inline[cp.current_index as usize];
            cp.current_index += 1;

            // Return the alternative tag
            return alt.tag as i64;
        }

        // No more alternatives in this choice point - pop it
        ctx.choice_point_count -= 1;
    }

    // No more alternatives anywhere
    -1
}

/// Get the current alternative from the topmost choice point.
///
/// This should be called after jit_runtime_fail returns a non-negative value
/// to get the actual alternative data.
///
/// # Returns
/// The JitAlternative at the current index of the topmost choice point.
///
/// # Safety
/// The context must have at least one choice point with a valid current alternative.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_current_alternative(
    ctx: *const JitContext,
) -> JitAlternative {
    let ctx = &*ctx;
    debug_assert!(ctx.choice_point_count > 0);

    let cp = &*ctx.choice_points.add(ctx.choice_point_count - 1);
    // current_index was already incremented by fail, so use index - 1
    debug_assert!(cp.current_index > 0);
    // Optimization 5.2: Read from inline alternatives array
    cp.alternatives_inline[(cp.current_index - 1) as usize]
}

// NOTE: jit_runtime_yield moved to Phase 4 section below (with value and ip params)

// NOTE: jit_runtime_collect moved to Phase 4 section below (with chunk_index param)

/// Get the number of results collected.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_results_count(ctx: *const JitContext) -> u64 {
    let Some(ctx) = ctx.as_ref() else {
        return 0;
    };
    ctx.results_count as u64
}

/// Get the number of active choice points.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_choice_point_count(ctx: *const JitContext) -> u64 {
    let Some(ctx) = ctx.as_ref() else {
        return 0;
    };
    ctx.choice_point_count as u64
}

// =============================================================================
// Phase 2a: Value Creation Runtime (MakeSExpr, ConsAtom)
// =============================================================================

/// Create an S-expression from an array of NaN-boxed values.
///
/// This function takes a pointer to an array of NaN-boxed values (u64) and
/// creates a MettaValue::SExpr from them.
///
/// # Arguments
/// * `ctx` - JIT context (for error handling)
/// * `values_ptr` - Pointer to array of NaN-boxed u64 values
/// * `count` - Number of elements in the array
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the new S-expression
///
/// # Safety
/// * The context pointer must be valid
/// * values_ptr must point to a valid array of count u64 values
/// * Each value must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_sexpr(
    _ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    let count = count as usize;

    // Handle empty S-expression
    if count == 0 {
        let sexpr = Box::new(MettaValue::SExpr(Vec::new()));
        let ptr = Box::into_raw(sexpr);
        return super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // Convert each value to MettaValue
    let mut elements = Vec::with_capacity(count);
    for i in 0..count {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);
        elements.push(jit_val.to_metta());
    }

    // Create the S-expression and return as heap pointer
    let sexpr = Box::new(MettaValue::SExpr(elements));
    let ptr = Box::into_raw(sexpr);
    super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Prepend a value to an S-expression (cons operation).
///
/// This function implements the cons-atom operation:
/// - If tail is an S-expression, prepend head to it
/// - If tail is Nil, create a single-element S-expression
/// - Otherwise, signal a type error
///
/// # Arguments
/// * `ctx` - JIT context (for error handling)
/// * `head` - NaN-boxed value to prepend
/// * `tail` - NaN-boxed S-expression or Nil
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the new S-expression
///
/// # Safety
/// * The context pointer must be valid
/// * head and tail must be valid NaN-boxed values
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cons_atom(
    ctx: *mut JitContext,
    head: u64,
    tail: u64,
    ip: u64,
) -> u64 {
    let head_val = JitValue::from_raw(head);
    let head_metta = head_val.to_metta();

    let tail_tag = tail & super::types::TAG_MASK;

    // Handle Nil tail
    if tail_tag == super::types::TAG_NIL {
        let sexpr = Box::new(MettaValue::SExpr(vec![head_metta]));
        let ptr = Box::into_raw(sexpr);
        return super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // Must be a heap pointer (S-expression)
    if tail_tag != super::types::TAG_HEAP {
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        return super::types::TAG_NIL;
    }

    // Get the tail as MettaValue
    let tail_ptr = (tail & PAYLOAD_MASK) as *const MettaValue;
    if tail_ptr.is_null() {
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        return super::types::TAG_NIL;
    }

    // Check if tail is an S-expression
    match &*tail_ptr {
        MettaValue::SExpr(elements) => {
            // Prepend head to the elements
            let mut new_elements = Vec::with_capacity(elements.len() + 1);
            new_elements.push(head_metta);
            new_elements.extend(elements.iter().cloned());

            let sexpr = Box::new(MettaValue::SExpr(new_elements));
            let ptr = Box::into_raw(sexpr);
            super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        MettaValue::Nil => {
            // Treat Nil as empty S-expression
            let sexpr = Box::new(MettaValue::SExpr(vec![head_metta]));
            let ptr = Box::into_raw(sexpr);
            super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        _ => {
            // Type error: tail is not an S-expression or Nil
            if let Some(ctx) = ctx.as_mut() {
                ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
            }
            super::types::TAG_NIL
        }
    }
}

// =============================================================================
// Phase 2b: Value Creation Runtime (PushUri, MakeList, MakeQuote)
// =============================================================================

/// Load a URI from the constant pool (same as PushConstant).
///
/// PushUri uses the same mechanism as PushConstant - it loads a value from
/// the constant pool by index. The value at that index should be a MettaValue
/// representing the URI.
///
/// This function is an alias for jit_runtime_load_constant for clarity.
///
/// # Safety
/// The context pointer and constant index must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_uri(ctx: *const JitContext, index: u64) -> u64 {
    jit_runtime_load_constant(ctx, index)
}

/// Create a proper MeTTa list from an array of NaN-boxed values.
///
/// Builds a linked list using the (Cons elem rest) structure:
/// - Elements are popped in order and reversed to build (Cons elem (Cons ... Nil))
/// - Empty list is just Nil
///
/// For example, with values [1, 2, 3], creates:
/// (Cons 1 (Cons 2 (Cons 3 Nil)))
///
/// # Arguments
/// * `ctx` - JIT context (for error handling)
/// * `values_ptr` - Pointer to array of NaN-boxed u64 values
/// * `count` - Number of elements in the array
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the list (or TAG_NIL for empty list)
///
/// # Safety
/// * The context pointer must be valid
/// * values_ptr must point to a valid array of count u64 values
/// * Each value must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_list(
    _ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    let count = count as usize;

    // Empty list is Nil
    if count == 0 {
        return super::types::TAG_NIL;
    }

    // Build the list from the end (reverse order to get proper Cons structure)
    // Start with Nil, then Cons each element from the end
    let mut list = MettaValue::Nil;

    for i in (0..count).rev() {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);
        let elem = jit_val.to_metta();

        // Build (Cons elem list)
        list = MettaValue::SExpr(vec![
            MettaValue::Atom("Cons".to_string()),
            elem,
            list,
        ]);
    }

    // Return as heap pointer
    let boxed = Box::new(list);
    let ptr = Box::into_raw(boxed);
    super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Wrap a value in a quote expression.
///
/// Creates (quote value) S-expression to prevent evaluation.
///
/// # Arguments
/// * `ctx` - JIT context (unused, for consistency)
/// * `val` - NaN-boxed value to quote
/// * `ip` - Instruction pointer (unused, for consistency)
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the (quote value) S-expression
///
/// # Safety
/// * val must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_quote(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);
    let inner = jit_val.to_metta();

    // Create (quote value)
    let quoted = MettaValue::SExpr(vec![
        MettaValue::Atom("quote".to_string()),
        inner,
    ]);

    let boxed = Box::new(quoted);
    let ptr = Box::into_raw(boxed);
    super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

// =============================================================================
// Phase 3: Call/TailCall Support
// =============================================================================

/// Dispatch a call expression with native rule lookup.
///
/// Stage 2 implementation with native rule dispatch:
/// 1. Builds the call expression from head symbol + arguments
/// 2. If bridge available: dispatches rules natively using MorkBridge
/// 3. For 0 matches: returns expression directly (irreducible) - NO bailout!
/// 4. For 1+ matches: signals bailout for VM to execute rule bodies
///
/// The native dispatch avoids VM overhead for the common case of irreducible
/// expressions (grounded functions, data constructors, etc.).
///
/// # Parameters
/// * `ctx` - JIT context (may be modified to signal bailout)
/// * `head_index` - Index of head symbol in constant pool
/// * `args_ptr` - Pointer to array of NaN-boxed argument values
/// * `arity` - Number of arguments
/// * `ip` - Instruction pointer for bailout
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the call expression
///
// =============================================================================
// Optimization 3.2: Fast Path for Grounded Functions
// =============================================================================

/// Attempt to execute a grounded function directly without MorkBridge lookup.
///
/// This fast path handles common arithmetic and comparison operations inline,
/// bypassing the rule dispatch system for known grounded functions.
///
/// Returns `Some(result)` if the operation was handled, `None` otherwise.
///
/// # Safety
/// args_ptr must point to at least `arity` valid NaN-boxed values
#[inline(always)]
unsafe fn try_grounded_fast_path(head: &str, args_ptr: *const u64, arity: usize) -> Option<u64> {
    // Fast path for binary operations (arity == 2)
    if arity == 2 {
        let arg0_raw = *args_ptr;
        let arg1_raw = *args_ptr.add(1);

        // Check if both arguments are integers (TAG_LONG)
        let arg0_jit = JitValue::from_raw(arg0_raw);
        let arg1_jit = JitValue::from_raw(arg1_raw);

        // Integer fast path
        if arg0_jit.is_long() && arg1_jit.is_long() {
            let a = arg0_jit.as_long();
            let b = arg1_jit.as_long();
            let result = match head {
                "+" => Some(JitValue::from_long(a.wrapping_add(b))),
                "-" => Some(JitValue::from_long(a.wrapping_sub(b))),
                "*" => Some(JitValue::from_long(a.wrapping_mul(b))),
                "/" => {
                    if b != 0 {
                        Some(JitValue::from_long(a / b))
                    } else {
                        None // Division by zero - fall back to regular path
                    }
                }
                "%" => {
                    if b != 0 {
                        Some(JitValue::from_long(a % b))
                    } else {
                        None // Modulo by zero - fall back to regular path
                    }
                }
                "==" => Some(JitValue::from_bool(a == b)),
                "!=" => Some(JitValue::from_bool(a != b)),
                "<" => Some(JitValue::from_bool(a < b)),
                "<=" => Some(JitValue::from_bool(a <= b)),
                ">" => Some(JitValue::from_bool(a > b)),
                ">=" => Some(JitValue::from_bool(a >= b)),
                _ => None,
            };

            if let Some(r) = result {
                return Some(r.to_bits());
            }
        }

        // Boolean fast path for logical operations
        if arg0_jit.is_bool() && arg1_jit.is_bool() {
            let a = arg0_jit.as_bool();
            let b = arg1_jit.as_bool();
            let result = match head {
                "and" => Some(JitValue::from_bool(a && b)),
                "or" => Some(JitValue::from_bool(a || b)),
                "==" => Some(JitValue::from_bool(a == b)),
                "!=" => Some(JitValue::from_bool(a != b)),
                _ => None,
            };

            if let Some(r) = result {
                return Some(r.to_bits());
            }
        }
    }

    // Fast path for unary operations (arity == 1)
    if arity == 1 {
        let arg0_raw = *args_ptr;
        let arg0_jit = JitValue::from_raw(arg0_raw);

        // Boolean unary operations
        if arg0_jit.is_bool() {
            let a = arg0_jit.as_bool();
            let result = match head {
                "not" => Some(JitValue::from_bool(!a)),
                _ => None,
            };

            if let Some(r) = result {
                return Some(r.to_bits());
            }
        }

        // Integer unary operations (if we add any like abs, negate)
        if arg0_jit.is_long() {
            let a = arg0_jit.as_long();
            let result = match head {
                "negate" | "-" => Some(JitValue::from_long(-a)),
                "abs" => Some(JitValue::from_long(a.abs())),
                _ => None,
            };

            if let Some(r) = result {
                return Some(r.to_bits());
            }
        }
    }

    None
}

/// # Safety
/// * ctx must be a valid mutable pointer
/// * head_index must be valid for ctx.constants
/// * args_ptr must point to an array of at least `arity` valid NaN-boxed values
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_call(
    ctx: *mut JitContext,
    head_index: u64,
    args_ptr: *const u64,
    arity: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let arity = arity as usize;
    let head_index = head_index as usize;

    // Get head symbol from constant pool
    if head_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return super::types::TAG_NIL;
    }

    let head_value = &*ctx_ref.constants.add(head_index);
    let head = match head_value {
        MettaValue::Atom(s) => s.clone(),
        _ => {
            // Head must be an atom
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            return super::types::TAG_NIL;
        }
    };

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if !args_ptr.is_null() {
        if let Some(result) = try_grounded_fast_path(&head, args_ptr, arity) {
            return result;
        }
    }

    // Build argument list
    let mut items = Vec::with_capacity(arity + 1);
    items.push(MettaValue::Atom(head.clone()));

    // Add arguments
    for i in 0..arity {
        let arg_raw = *args_ptr.add(i);
        let arg_jit = JitValue::from_raw(arg_raw);
        items.push(arg_jit.to_metta());
    }

    // Create the call expression
    let expr = MettaValue::SExpr(items);

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // No rules match - return expression unchanged (irreducible)
            // This is a major optimization: no bailout needed!
            let boxed = Box::new(expr);
            let ptr = Box::into_raw(boxed);
            return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
        }

        // Phase 2: Native rule execution for single-match rules
        if matches.len() == 1 {
            let rule = &matches[0];

            // Execute the rule body with bindings applied
            // The CompiledRule already has bindings from pattern matching
            let mut vm = BytecodeVM::new(Arc::clone(&rule.body));

            // Apply bindings by pushing them onto the VM's binding stack
            for (name, value) in rule.bindings.iter() {
                // Create binding in VM (this is a simplified approach)
                // The bytecode chunk expects bindings to be accessible
                // Push value onto stack as initial binding
                vm.push_initial_value(value.clone());
            }

            // Execute and return result
            match vm.run() {
                Ok(results) => {
                    let result = results.into_iter().next().unwrap_or(MettaValue::Unit);
                    return metta_to_jit(&result).to_bits();
                }
                Err(_) => {
                    // Execution error - bailout for VM to handle
                    ctx_ref.bailout = true;
                    ctx_ref.bailout_ip = ip as usize;
                    ctx_ref.bailout_reason = JitBailoutReason::Call;
                    let boxed = Box::new(expr);
                    let ptr = Box::into_raw(boxed);
                    return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
                }
            }
        }

        // Multiple rules match - use Fork for nondeterminism
        if matches.len() > 1 && ctx_ref.choice_point_count < ctx_ref.choice_point_cap {
            // Create alternatives from matching rules
            let mut alternatives: Vec<JitAlternative> = Vec::with_capacity(matches.len());
            for rule in &matches {
                // Each alternative is the rule's body chunk
                // Execute each and collect as alternatives
                let mut vm = BytecodeVM::new(Arc::clone(&rule.body));
                for (_, value) in rule.bindings.iter() {
                    vm.push_initial_value(value.clone());
                }
                if let Ok(results) = vm.run() {
                    if let Some(result) = results.into_iter().next() {
                        alternatives.push(JitAlternative::value(metta_to_jit(&result)));
                    }
                }
            }

            if !alternatives.is_empty() {
                // Return first result, save rest as choice point
                let first = alternatives.remove(0);

                if !alternatives.is_empty() {
                    let alt_count = alternatives.len();

                    // Optimization 5.2: Check if alternatives fit inline
                    if alt_count <= super::MAX_ALTERNATIVES_INLINE {
                        let cp = &mut *ctx_ref.choice_points.add(ctx_ref.choice_point_count);
                        cp.saved_sp = ctx_ref.sp as u64;
                        cp.alt_count = alt_count as u64;
                        cp.current_index = 0;
                        cp.saved_ip = ip;
                        cp.saved_chunk = ctx_ref.current_chunk;
                        cp.saved_stack_pool_idx = -1;
                        cp.saved_stack_count = 0;
                        cp.fork_depth = ctx_ref.fork_depth;
                        cp.saved_binding_frames_count = ctx_ref.binding_frames_count;
                        cp.is_collect_boundary = false;

                        // Copy alternatives to inline array
                        for (i, alt) in alternatives.into_iter().enumerate() {
                            cp.alternatives_inline[i] = alt;
                        }

                        ctx_ref.choice_point_count += 1;
                    }
                }

                // Return first result value (payload is already NaN-boxed bits)
                return first.payload;
            }
        }

        // Fallback: bailout for VM to execute rule bodies
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Call;

        let boxed = Box::new(expr);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No bridge - signal bailout for VM to handle
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Call;

    // Return the expression as a heap value
    let boxed = Box::new(expr);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Dispatch a tail call expression with native rule lookup.
///
/// Stage 2 implementation with native rule dispatch and TCO hint:
/// 1. Builds the call expression from head symbol + arguments
/// 2. If bridge available: dispatches rules natively using MorkBridge
/// 3. For 0 matches: returns expression directly (irreducible) - NO bailout!
/// 4. For 1+ matches: signals TailCall bailout for VM to execute with TCO
///
/// The TailCall bailout reason tells the VM to use tail call optimization
/// when executing the rule body.
///
/// # Safety
/// Same requirements as `jit_runtime_call`
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_tail_call(
    ctx: *mut JitContext,
    head_index: u64,
    args_ptr: *const u64,
    arity: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let arity = arity as usize;
    let head_index = head_index as usize;

    // Get head symbol from constant pool
    if head_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return super::types::TAG_NIL;
    }

    let head_value = &*ctx_ref.constants.add(head_index);
    let head = match head_value {
        MettaValue::Atom(s) => s.clone(),
        _ => {
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            return super::types::TAG_NIL;
        }
    };

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if !args_ptr.is_null() {
        if let Some(result) = try_grounded_fast_path(&head, args_ptr, arity) {
            return result;
        }
    }

    // Build argument list
    let mut items = Vec::with_capacity(arity + 1);
    items.push(MettaValue::Atom(head.clone()));

    // Add arguments
    for i in 0..arity {
        let arg_raw = *args_ptr.add(i);
        let arg_jit = JitValue::from_raw(arg_raw);
        items.push(arg_jit.to_metta());
    }

    // Create the call expression
    let expr = MettaValue::SExpr(items);

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // No rules match - return expression unchanged (irreducible)
            // This is a major optimization: no bailout needed!
            let boxed = Box::new(expr);
            let ptr = Box::into_raw(boxed);
            return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
        }

        // Rules matched - bailout for VM to execute rule bodies with TCO
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::TailCall;

        let boxed = Box::new(expr);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No bridge - signal bailout for VM to handle (with TCO hint)
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::TailCall;

    // Return the expression as a heap value
    let boxed = Box::new(expr);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

// =============================================================================
// Phase 1.2: CallN/TailCallN Runtime Functions (stack-based head)
// =============================================================================

/// Runtime function for CallN opcode
///
/// Unlike Call which gets head from constant pool, CallN gets head from the stack.
/// This is used when the head is dynamically computed.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `head_val` - NaN-boxed head value (from stack)
/// * `args_ptr` - Pointer to array of NaN-boxed arguments
/// * `arity` - Number of arguments
/// * `ip` - Instruction pointer for bailout
///
/// # Returns
/// NaN-boxed result of the call (heap-allocated S-expression)
///
/// # Safety
/// The context pointer must be valid. The args_ptr must point to `arity` u64 values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_call_n(
    ctx: *mut JitContext,
    head_val: u64,
    args_ptr: *const u64,
    arity: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let arity = arity as usize;

    // Convert head from NaN-boxed to MettaValue
    let head_jit = JitValue::from_raw(head_val);
    let head_metta = head_jit.to_metta();

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if let MettaValue::Atom(ref head_str) = head_metta {
        if !args_ptr.is_null() {
            if let Some(result) = try_grounded_fast_path(head_str, args_ptr, arity) {
                return result;
            }
        }
    }

    // Build argument list with head as first element
    let mut items = Vec::with_capacity(arity + 1);
    items.push(head_metta);

    // Add arguments
    for i in 0..arity {
        let arg_raw = *args_ptr.add(i);
        let arg_jit = JitValue::from_raw(arg_raw);
        items.push(arg_jit.to_metta());
    }

    // Create the call expression
    let expr = MettaValue::SExpr(items);

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // No rules match - return expression unchanged (irreducible)
            let boxed = Box::new(expr);
            let ptr = Box::into_raw(boxed);
            return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
        }

        // Rules matched - bailout for VM to execute rule bodies
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Call;

        let boxed = Box::new(expr);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No bridge - signal bailout for VM to handle
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Call;

    // Return the expression as a heap value
    let boxed = Box::new(expr);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Runtime function for TailCallN opcode
///
/// Same as CallN but signals TCO (tail call optimization) to the VM.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `head_val` - NaN-boxed head value (from stack)
/// * `args_ptr` - Pointer to array of NaN-boxed arguments
/// * `arity` - Number of arguments
/// * `ip` - Instruction pointer for bailout
///
/// # Returns
/// NaN-boxed result of the call (heap-allocated S-expression)
///
/// # Safety
/// The context pointer must be valid. The args_ptr must point to `arity` u64 values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_tail_call_n(
    ctx: *mut JitContext,
    head_val: u64,
    args_ptr: *const u64,
    arity: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let arity = arity as usize;

    // Convert head from NaN-boxed to MettaValue
    let head_jit = JitValue::from_raw(head_val);
    let head_metta = head_jit.to_metta();

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if let MettaValue::Atom(ref head_str) = head_metta {
        if !args_ptr.is_null() {
            if let Some(result) = try_grounded_fast_path(head_str, args_ptr, arity) {
                return result;
            }
        }
    }

    // Build argument list with head as first element
    let mut items = Vec::with_capacity(arity + 1);
    items.push(head_metta);

    // Add arguments
    for i in 0..arity {
        let arg_raw = *args_ptr.add(i);
        let arg_jit = JitValue::from_raw(arg_raw);
        items.push(arg_jit.to_metta());
    }

    // Create the call expression
    let expr = MettaValue::SExpr(items);

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // No rules match - return expression unchanged (irreducible)
            let boxed = Box::new(expr);
            let ptr = Box::into_raw(boxed);
            return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
        }

        // Rules matched - bailout for VM to execute rule bodies with TCO
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::TailCall;

        let boxed = Box::new(expr);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No bridge - signal bailout for VM to handle (with TCO hint)
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::TailCall;

    // Return the expression as a heap value
    let boxed = Box::new(expr);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

// =============================================================================
// Phase 4: Fork/Yield/Collect Runtime Functions
// =============================================================================

/// Runtime function for Fork opcode
///
/// Fork creates a choice point with multiple alternatives. The JIT signals
/// bailout so the VM can manage backtracking properly.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `count` - Number of alternatives
/// * `indices_ptr` - Pointer to array of constant pool indices (u16 each, but passed as u64)
/// * `ip` - Instruction pointer for resume
///
/// # Returns
/// NaN-boxed value of the first alternative (pushed to stack by JIT)
///
/// # Safety
/// The context and indices pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fork(
    ctx: *mut JitContext,
    count: u64,
    indices_ptr: *const u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let count = count as usize;

    // If no alternatives, this is a fail
    if count == 0 {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Fork;
        return super::types::TAG_NIL;
    }

    // Get the first alternative from constant pool
    let first_index = if !indices_ptr.is_null() {
        *indices_ptr as usize
    } else {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Fork;
        return super::types::TAG_NIL;
    };

    if first_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return super::types::TAG_NIL;
    }

    let first_value = &*ctx_ref.constants.add(first_index);
    // Convert to JitValue, heap-allocating if necessary
    let first_jit = match JitValue::try_from_metta(first_value) {
        Some(jv) => jv,
        None => {
            // Can't NaN-box - allocate on heap
            let boxed = Box::new(first_value.clone());
            JitValue::from_heap_ptr(Box::into_raw(boxed))
        }
    };

    // Always signal bailout for Fork so VM can manage choice points
    // The VM will handle creating choice points for remaining alternatives
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Fork;

    // Return the first alternative value
    first_jit.to_bits()
}

/// Runtime function for Yield opcode
///
/// Yield saves the current result and signals bailout for backtracking.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `value` - Value to yield (already popped from stack by JIT)
/// * `ip` - Instruction pointer
///
/// # Returns
/// Always returns Nil (the VM will handle backtracking)
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_yield(
    ctx: *mut JitContext,
    value: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    // Store the result if there's space
    if ctx_ref.results_count < ctx_ref.results_cap && !ctx_ref.results.is_null() {
        let result_val = JitValue::from_raw(value);
        *ctx_ref.results.add(ctx_ref.results_count) = result_val;
        ctx_ref.results_count += 1;
    }

    // Signal bailout for VM to handle backtracking
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Yield;

    super::types::TAG_NIL
}

/// Runtime function for Collect opcode
///
/// Collect gathers all yielded results into an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `chunk_index` - Sub-chunk index (reserved for future use)
/// * `ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed heap pointer to SExpr containing all collected results
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_collect(
    ctx: *mut JitContext,
    _chunk_index: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    // Signal bailout - VM needs to complete nondeterministic execution
    // and collect all results
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Collect;

    // If we have results in the JIT context, build the SExpr
    if ctx_ref.results_count > 0 && !ctx_ref.results.is_null() {
        let mut items = Vec::with_capacity(ctx_ref.results_count);
        for i in 0..ctx_ref.results_count {
            let jit_val = *ctx_ref.results.add(i);
            let metta_val = jit_val.to_metta();
            // Filter out Nil values (matches VM collapse semantics)
            if !matches!(metta_val, MettaValue::Nil) {
                items.push(metta_val);
            }
        }

        // Clear results
        ctx_ref.results_count = 0;

        // Return as heap-allocated SExpr
        let expr = MettaValue::SExpr(items);
        let boxed = Box::new(expr);
        let ptr = Box::into_raw(boxed);
        return super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No results - return empty SExpr
    let empty = MettaValue::SExpr(Vec::new());
    let boxed = Box::new(empty);
    let ptr = Box::into_raw(boxed);
    super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

// =============================================================================
// S-Expression Operations (Stage 14: Head/Tail/Arity/Element)
// =============================================================================

/// Runtime function for PushEmpty opcode
///
/// Creates and returns an empty S-expression ().
///
/// # Returns
/// NaN-boxed heap pointer to empty SExpr
///
/// # Safety
/// No special safety requirements.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_empty() -> u64 {
    let empty = MettaValue::SExpr(Vec::new());
    let boxed = Box::new(empty);
    let ptr = Box::into_raw(boxed);
    super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Runtime function for GetHead opcode
///
/// Gets the head (first element) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed head element, or TAG_NIL if empty/not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_head(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return super::types::TAG_NIL;
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        return super::types::TAG_NIL;
    }

    let metta_val = &*metta_ptr;
    match metta_val {
        MettaValue::SExpr(items) => {
            if items.is_empty() {
                super::types::TAG_NIL
            } else {
                // Return the head element
                let head = &items[0];
                match JitValue::try_from_metta(head) {
                    Some(jv) => jv.to_bits(),
                    None => {
                        // Need to heap-allocate for non-primitive types
                        let boxed = Box::new(head.clone());
                        let ptr = Box::into_raw(boxed);
                        super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
                    }
                }
            }
        }
        _ => super::types::TAG_NIL,
    }
}

/// Runtime function for GetTail opcode
///
/// Gets the tail (all elements except first) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed heap pointer to tail SExpr, or empty SExpr if empty/not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_tail(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        // Return empty SExpr for non-heap values
        let empty = MettaValue::SExpr(Vec::new());
        let boxed = Box::new(empty);
        let ptr = Box::into_raw(boxed);
        return super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        let empty = MettaValue::SExpr(Vec::new());
        let boxed = Box::new(empty);
        let ptr = Box::into_raw(boxed);
        return super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    let metta_val = &*metta_ptr;
    match metta_val {
        MettaValue::SExpr(items) => {
            // Return tail (skip first element)
            let tail: Vec<MettaValue> = if items.len() > 1 {
                items[1..].to_vec()
            } else {
                Vec::new()
            };
            let expr = MettaValue::SExpr(tail);
            let boxed = Box::new(expr);
            let ptr = Box::into_raw(boxed);
            super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        _ => {
            // Return empty SExpr for non-SExpr values
            let empty = MettaValue::SExpr(Vec::new());
            let boxed = Box::new(empty);
            let ptr = Box::into_raw(boxed);
            super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
    }
}

/// Runtime function for GetArity opcode
///
/// Gets the arity (number of elements) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed Long containing the arity, or 0 if not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_arity(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return JitValue::from_long(0).to_bits();
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        return JitValue::from_long(0).to_bits();
    }

    let metta_val = &*metta_ptr;
    match metta_val {
        MettaValue::SExpr(items) => {
            JitValue::from_long(items.len() as i64).to_bits()
        }
        _ => JitValue::from_long(0).to_bits(),
    }
}

/// Runtime function for GetElement opcode
///
/// Gets an element at a specific index from an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `index` - Element index (0-based)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed element at index, or TAG_NIL if out of bounds/not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_element(
    _ctx: *mut JitContext,
    val: u64,
    index: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return super::types::TAG_NIL;
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        return super::types::TAG_NIL;
    }

    let metta_val = &*metta_ptr;
    let idx = index as usize;

    match metta_val {
        MettaValue::SExpr(items) => {
            if idx >= items.len() {
                super::types::TAG_NIL
            } else {
                let elem = &items[idx];
                match JitValue::try_from_metta(elem) {
                    Some(jv) => jv.to_bits(),
                    None => {
                        // Need to heap-allocate for non-primitive types
                        let boxed = Box::new(elem.clone());
                        let ptr = Box::into_raw(boxed);
                        super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
                    }
                }
            }
        }
        _ => super::types::TAG_NIL,
    }
}

// =============================================================================
// Stage 2: Native Nondeterminism Dispatcher Functions
// =============================================================================
//
// These functions implement the dispatcher pattern for native nondeterminism.
// Instead of bailing out to the VM, they return signal values that the
// dispatcher loop uses to control execution flow.
//
// Signal flow:
// 1. Fork creates choice point, saves stack, returns first alternative
// 2. Yield stores result, returns JIT_SIGNAL_YIELD
// 3. Dispatcher calls fail_native to try next alternative
// 4. When exhausted, collect_native gathers all results

/// Save current stack state for backtracking
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_save_stack(ctx: *mut JitContext) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return JIT_SIGNAL_ERROR,
    };

    // If no saved_stack buffer, we can't save
    if ctx_ref.saved_stack.is_null() || ctx_ref.saved_stack_cap == 0 {
        return JIT_SIGNAL_OK; // Not an error, just no-op
    }

    // Copy current stack to saved_stack
    let to_save = ctx_ref.sp.min(ctx_ref.saved_stack_cap);
    for i in 0..to_save {
        *ctx_ref.saved_stack.add(i) = *ctx_ref.value_stack.add(i);
    }
    ctx_ref.saved_stack_count = to_save;

    JIT_SIGNAL_OK
}

/// Restore stack state from saved buffer
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_restore_stack(ctx: *mut JitContext) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return JIT_SIGNAL_ERROR,
    };

    // If no saved stack, nothing to restore
    if ctx_ref.saved_stack.is_null() || ctx_ref.saved_stack_count == 0 {
        return JIT_SIGNAL_OK;
    }

    // Restore stack from saved_stack
    let to_restore = ctx_ref.saved_stack_count.min(ctx_ref.stack_cap);
    for i in 0..to_restore {
        *ctx_ref.value_stack.add(i) = *ctx_ref.saved_stack.add(i);
    }
    ctx_ref.sp = to_restore;

    JIT_SIGNAL_OK
}

/// Stage 2: Fork with native nondeterminism
///
/// Creates a choice point, saves stack state, and returns the first alternative.
/// Unlike the Stage 1 version, this doesn't set bailout - it creates a proper
/// choice point for the dispatcher to manage.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `count` - Number of alternatives
/// * `indices_ptr` - Pointer to array of constant pool indices
/// * `ip` - Instruction pointer for resume (after Fork)
///
/// # Returns
/// NaN-boxed value of the first alternative
///
/// # Safety
/// The context and indices pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fork_native(
    ctx: *mut JitContext,
    count: u64,
    indices_ptr: *const u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let count = count as usize;

    // If no alternatives, return Nil (fail case)
    if count == 0 {
        return super::types::TAG_NIL;
    }

    // Check if we have nondet support
    if !ctx_ref.has_nondet_support() {
        // Fall back to bailout mode
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Fork;
        return super::types::TAG_NIL;
    }

    // Get the first alternative from constant pool
    let first_index = if !indices_ptr.is_null() {
        *indices_ptr as usize
    } else {
        return super::types::TAG_NIL;
    };

    if first_index >= ctx_ref.constants_len {
        return super::types::TAG_NIL;
    }

    let first_value = &*ctx_ref.constants.add(first_index);
    let first_jit = match JitValue::try_from_metta(first_value) {
        Some(jv) => jv,
        None => {
            let boxed = Box::new(first_value.clone());
            JitValue::from_heap_ptr(Box::into_raw(boxed))
        }
    };

    // If more than one alternative, create choice point
    if count > 1 && ctx_ref.choice_point_count < ctx_ref.choice_point_cap {
        let alt_count = count - 1;

        // Optimization 5.2: Check if we can use inline alternatives
        if alt_count > super::MAX_ALTERNATIVES_INLINE {
            // Too many alternatives for inline storage - fall back to bailout
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::Fork;
            return first_jit.to_bits();
        }

        // Optimization 5.2: Check if stack fits in pool
        let stack_count = ctx_ref.sp;
        if stack_count > super::MAX_STACK_SAVE_VALUES {
            // Stack too large for pool - fall back to bailout
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::Fork;
            return first_jit.to_bits();
        }

        // Create choice point with inline alternatives (no allocation)
        let cp_idx = ctx_ref.choice_point_count;
        let cp = &mut *ctx_ref.choice_points.add(cp_idx);

        // Initialize base fields
        cp.saved_sp = ctx_ref.sp as u64;
        cp.alt_count = alt_count as u64;
        cp.current_index = 0;
        cp.saved_ip = ip;
        cp.saved_chunk = ctx_ref.current_chunk;
        cp.saved_stack_count = stack_count;
        cp.fork_depth = ctx_ref.fork_depth;
        cp.saved_binding_frames_count = ctx_ref.binding_frames_count;
        cp.is_collect_boundary = false;

        // Optimization 5.2: Store alternatives inline (eliminates Box::leak)
        for i in 0..alt_count {
            let idx = *indices_ptr.add(i + 1) as usize;
            if idx < ctx_ref.constants_len {
                let val = &*ctx_ref.constants.add(idx);
                let jv = match JitValue::try_from_metta(val) {
                    Some(j) => j,
                    None => {
                        let boxed = Box::new(val.clone());
                        JitValue::from_heap_ptr(Box::into_raw(boxed))
                    }
                };
                cp.alternatives_inline[i] = JitAlternative::value(jv);
            }
        }

        // Optimization 5.2: Use stack save pool (eliminates Box::leak)
        if stack_count > 0 && !ctx_ref.value_stack.is_null() && ctx_ref.has_stack_save_pool() {
            let pool_idx = ctx_ref.stack_save_pool_alloc(stack_count);
            if pool_idx >= 0 {
                ctx_ref.stack_save_to_pool(pool_idx as usize, stack_count);
                cp.saved_stack_pool_idx = pool_idx;
            } else {
                cp.saved_stack_pool_idx = -1;
            }
        } else {
            cp.saved_stack_pool_idx = -1;
        }

        ctx_ref.choice_point_count += 1;

        // Enter nondet mode
        ctx_ref.enter_nondet_mode();
    }

    // Return the first alternative value
    first_jit.to_bits()
}

/// Stage 2: Yield with native signal return
///
/// Stores the result and returns JIT_SIGNAL_YIELD to signal the dispatcher
/// to backtrack and try more alternatives.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `value` - Value to yield
/// * `ip` - Instruction pointer
///
/// # Returns
/// JIT_SIGNAL_YIELD as i64 (reinterpreted from u64 return)
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_yield_native(
    ctx: *mut JitContext,
    value: u64,
    ip: u64,
) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return JIT_SIGNAL_ERROR,
    };

    // Store the result
    if ctx_ref.results_count < ctx_ref.results_cap && !ctx_ref.results.is_null() {
        let result_val = JitValue::from_raw(value);
        *ctx_ref.results.add(ctx_ref.results_count) = result_val;
        ctx_ref.results_count += 1;
    }

    // Set resume IP for potential re-entry
    ctx_ref.resume_ip = ip as usize;

    // Return yield signal for dispatcher
    JIT_SIGNAL_YIELD
}

/// Stage 2: Fail and try next alternative
///
/// Attempts to backtrack to the next alternative. If successful, restores
/// state and returns the next alternative value. If no alternatives remain,
/// returns JIT_SIGNAL_FAIL.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// Next alternative value on success, or JIT_SIGNAL_FAIL encoded as u64
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fail_native(ctx: *mut JitContext) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return JIT_SIGNAL_FAIL as u64,
    };

    // No choice points = exhausted
    if ctx_ref.choice_point_count == 0 {
        return JIT_SIGNAL_FAIL as u64;
    }

    // Get current choice point
    let cp_idx = ctx_ref.choice_point_count - 1;
    let cp = &mut *ctx_ref.choice_points.add(cp_idx);

    // Try next alternative
    if cp.current_index < cp.alt_count {
        // Optimization 5.2: Read from inline alternatives array
        let alt = &cp.alternatives_inline[cp.current_index as usize];
        cp.current_index += 1;

        // Optimization 5.2: Restore stack from pool instead of leaked pointer
        if cp.saved_stack_pool_idx >= 0 && cp.saved_stack_count > 0 {
            ctx_ref.stack_restore_from_pool(
                cp.saved_stack_pool_idx as usize,
                cp.saved_stack_count,
            );
        }

        // Restore stack pointer
        ctx_ref.sp = cp.saved_sp as usize;

        // Phase 1.4: Restore binding frames count for nested scope restoration
        ctx_ref.binding_frames_count = cp.saved_binding_frames_count;

        // Return the alternative value
        match alt.tag {
            JitAlternativeTag::Value => alt.payload,
            JitAlternativeTag::Chunk => {
                // For chunk alternatives, set up for chunk execution
                ctx_ref.current_chunk = alt.payload as *const ();
                ctx_ref.resume_ip = 0;
                alt.payload // Return chunk pointer as signal to caller
            }
            JitAlternativeTag::RuleMatch => {
                // For rule matches, similar handling
                ctx_ref.current_chunk = alt.payload as *const ();
                alt.payload
            }
            JitAlternativeTag::SpaceMatch => {
                // Space match alternatives contain pre-computed results:
                // - payload: NaN-boxed result value (template already instantiated)
                // - payload2: unused
                // - payload3: saved binding frames pointer (for restoration)
                //
                // The handler:
                // 1. Restores binding frames from payload3 (consumes the snapshot)
                // 2. Returns the pre-computed result from payload
                jit_runtime_resume_space_match(ctx, alt)
            }
        }
    } else {
        // This choice point exhausted - pop it
        ctx_ref.choice_point_count -= 1;
        ctx_ref.exit_nondet_mode();

        // Recursively try parent choice point
        if ctx_ref.choice_point_count > 0 {
            jit_runtime_fail_native(ctx)
        } else {
            JIT_SIGNAL_FAIL as u64
        }
    }
}

/// Stage 2: Collect all results into an S-expression
///
/// Gathers all yielded results into an S-expression. This should be called
/// after all alternatives have been exhausted.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// NaN-boxed heap pointer to SExpr containing all collected results
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_collect_native(ctx: *mut JitContext) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    // Build SExpr from collected results
    if ctx_ref.results_count > 0 && !ctx_ref.results.is_null() {
        let mut items = Vec::with_capacity(ctx_ref.results_count);
        for i in 0..ctx_ref.results_count {
            let jit_val = *ctx_ref.results.add(i);
            let metta_val = jit_val.to_metta();
            // Filter out Nil values (matches VM collapse semantics)
            if !matches!(metta_val, MettaValue::Nil) {
                items.push(metta_val);
            }
        }

        // Clear results
        ctx_ref.results_count = 0;

        // Exit nondet mode
        ctx_ref.in_nondet_mode = false;
        ctx_ref.fork_depth = 0;

        // Return as heap-allocated SExpr
        let expr = MettaValue::SExpr(items);
        let boxed = Box::new(expr);
        let ptr = Box::into_raw(boxed);
        return super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No results - return empty SExpr
    let empty = MettaValue::SExpr(Vec::new());
    let boxed = Box::new(empty);
    let ptr = Box::into_raw(boxed);
    super::types::TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Stage 2: Check if there are more alternatives to try
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_has_alternatives(ctx: *const JitContext) -> i64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return 0,
    };

    if ctx_ref.choice_point_count == 0 {
        return 0;
    }

    let cp = &*ctx_ref.choice_points.add(ctx_ref.choice_point_count - 1);
    if cp.current_index < cp.alt_count {
        1
    } else {
        0
    }
}

/// Stage 2: Get the current resume IP
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_resume_ip(ctx: *const JitContext) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return 0,
    };
    ctx_ref.resume_ip as u64
}

// =============================================================================
// Stage 2 JIT: Dispatcher Loop
// =============================================================================

/// Type alias for JIT native function pointer
pub type JitNativeFn = unsafe extern "C" fn(*mut JitContext) -> i64;

/// Execute JIT code with nondeterminism support using dispatcher loop
///
/// This function implements the dispatcher pattern for Stage 2 JIT:
/// 1. Calls the JIT function
/// 2. Handles signals (JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL)
/// 3. Tries next alternatives via backtracking
/// 4. Returns collected results when all alternatives exhausted
///
/// # Arguments
/// * `ctx` - Mutable pointer to JitContext
/// * `jit_fn` - Pointer to JIT-compiled native function
///
/// # Returns
/// Vector of MettaValue results from all successful branches
///
/// # Safety
/// The context pointer must be valid and the JIT function must be compiled.
pub unsafe fn execute_with_dispatcher(
    ctx: *mut JitContext,
    jit_fn: JitNativeFn,
) -> Vec<MettaValue> {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return Vec::new(),
    };

    // Enter nondeterminism mode
    ctx_ref.enter_nondet_mode();

    // Reset results
    ctx_ref.results_count = 0;

    loop {
        // Execute JIT function
        let signal = jit_fn(ctx);

        match signal {
            s if s == JIT_SIGNAL_OK => {
                // Normal completion
                // If there are more choice points, try next alternative
                if ctx_ref.choice_point_count > 0 {
                    // Try to get next alternative
                    let fail_result = jit_runtime_fail_native(ctx);
                    if fail_result == JIT_SIGNAL_FAIL as u64 {
                        // No more alternatives - we're done
                        break;
                    }
                    // Got alternative, restore stack and continue
                    jit_runtime_restore_stack(ctx);
                    continue;
                }
                // No choice points, we're done
                break;
            }
            s if s == JIT_SIGNAL_YIELD => {
                // Result was stored by yield_native, try next alternative
                if ctx_ref.choice_point_count > 0 {
                    let fail_result = jit_runtime_fail_native(ctx);
                    if fail_result == JIT_SIGNAL_FAIL as u64 {
                        // No more alternatives
                        break;
                    }
                    // Got alternative, restore stack and continue
                    jit_runtime_restore_stack(ctx);
                    continue;
                }
                // No choice points, we're done
                break;
            }
            s if s == JIT_SIGNAL_FAIL => {
                // Explicit failure - try next alternative
                if ctx_ref.choice_point_count > 0 {
                    let fail_result = jit_runtime_fail_native(ctx);
                    if fail_result == JIT_SIGNAL_FAIL as u64 {
                        // No more alternatives
                        break;
                    }
                    // Got alternative, restore stack and continue
                    jit_runtime_restore_stack(ctx);
                    continue;
                }
                // No choice points, we're done
                break;
            }
            s if s == JIT_SIGNAL_ERROR => {
                // Error occurred - stop execution
                break;
            }
            _ => {
                // Unknown signal - treat as error
                break;
            }
        }
    }

    // Exit nondeterminism mode
    ctx_ref.exit_nondet_mode();

    // Collect results
    collect_results(ctx)
}

/// Collect results from JitContext into Vec<MettaValue>
///
/// # Safety
/// The context pointer must be valid.
pub unsafe fn collect_results(ctx: *mut JitContext) -> Vec<MettaValue> {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut results = Vec::with_capacity(ctx_ref.results_count);

    for i in 0..ctx_ref.results_count {
        let jv = *ctx_ref.results.add(i);
        let mv = jv.to_metta();
        results.push(mv);
    }

    results
}

/// Execute JIT code once (no nondeterminism support)
///
/// This is a simpler execution mode for deterministic code.
/// Returns the value from the top of the stack after execution.
///
/// # Safety
/// The context pointer must be valid.
pub unsafe fn execute_once(
    ctx: *mut JitContext,
    jit_fn: JitNativeFn,
) -> Option<MettaValue> {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return None,
    };

    let signal = jit_fn(ctx);

    if signal == JIT_SIGNAL_OK {
        // Get result from top of stack
        if ctx_ref.sp > 0 {
            let jv = *ctx_ref.value_stack.add(ctx_ref.sp - 1);
            Some(jv.to_metta())
        } else {
            None
        }
    } else if signal == JIT_SIGNAL_ERROR || ctx_ref.bailout {
        // Error occurred
        None
    } else {
        // For YIELD/FAIL signals in non-dispatcher mode, just return top of stack
        if ctx_ref.sp > 0 {
            let jv = *ctx_ref.value_stack.add(ctx_ref.sp - 1);
            Some(jv.to_metta())
        } else {
            None
        }
    }
}

// =============================================================================
// Phase A: Binding/Environment Runtime Functions
// =============================================================================

use super::types::{JitBindingEntry, JitBindingFrame};

/// Load a binding from the current binding environment.
///
/// Searches binding frames from innermost to outermost looking for a binding
/// with the given name index. The name is looked up in the constant pool.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `name_idx` - Index of the variable name in the constant pool
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// The bound value (NaN-boxed), or signals bailout if binding not found.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_binding(
    ctx: *mut JitContext,
    name_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let name_idx_u32 = name_idx as u32;

    // Search binding frames from innermost to outermost
    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        // Start from the innermost frame (highest index)
        for frame_idx in (0..ctx_ref.binding_frames_count).rev() {
            let frame = &*ctx_ref.binding_frames.add(frame_idx);

            // Search entries in this frame
            if frame.entries_count > 0 && !frame.entries.is_null() {
                for entry_idx in 0..frame.entries_count {
                    let entry = &*frame.entries.add(entry_idx);
                    if entry.name_idx == name_idx_u32 {
                        return entry.value.to_bits();
                    }
                }
            }
        }
    }

    // Binding not found - signal bailout
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::InvalidBinding;

    super::types::TAG_NIL
}

/// Store a binding in the current (innermost) binding frame.
///
/// Creates or updates a binding in the current scope.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `name_idx` - Index of the variable name in the constant pool
/// * `value` - The value to bind (NaN-boxed)
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// 0 on success, -1 on error (no binding frames).
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_store_binding(
    ctx: *mut JitContext,
    name_idx: u64,
    value: u64,
    _ip: u64,
) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -2,
    };

    if ctx_ref.binding_frames_count == 0 || ctx_ref.binding_frames.is_null() {
        return -1; // No binding frames
    }

    let name_idx_u32 = name_idx as u32;
    let jit_value = JitValue::from_raw(value);

    // Get the current (innermost) frame
    let frame = &mut *ctx_ref.binding_frames.add(ctx_ref.binding_frames_count - 1);

    // Check if binding already exists in this frame
    if frame.entries_count > 0 && !frame.entries.is_null() {
        for entry_idx in 0..frame.entries_count {
            let entry = &mut *frame.entries.add(entry_idx);
            if entry.name_idx == name_idx_u32 {
                // Update existing binding
                entry.value = jit_value;
                return 0;
            }
        }
    }

    // Add new binding to this frame
    // Ensure capacity
    if frame.entries.is_null() || frame.entries_count >= frame.entries_cap {
        // Need to grow or allocate
        let new_cap = if frame.entries_cap == 0 { 8 } else { frame.entries_cap * 2 };
        let layout = std::alloc::Layout::array::<JitBindingEntry>(new_cap)
            .expect("Layout calculation failed");
        let new_entries = std::alloc::alloc(layout) as *mut JitBindingEntry;

        if new_entries.is_null() {
            return -3; // Allocation failed
        }

        // Copy existing entries if any
        if !frame.entries.is_null() && frame.entries_count > 0 {
            std::ptr::copy_nonoverlapping(frame.entries, new_entries, frame.entries_count);
            // Free old allocation
            let old_layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                .expect("Layout calculation failed");
            std::alloc::dealloc(frame.entries as *mut u8, old_layout);
        }

        frame.entries = new_entries;
        frame.entries_cap = new_cap;
    }

    // Add the new entry
    let entry = &mut *frame.entries.add(frame.entries_count);
    entry.name_idx = name_idx_u32;
    entry.value = jit_value;
    frame.entries_count += 1;

    0 // Success
}

/// Check if a binding exists in any binding frame.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `name_idx` - Index of the variable name in the constant pool
///
/// # Returns
/// NaN-boxed Bool: true if binding exists, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_has_binding(
    ctx: *const JitContext,
    name_idx: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return super::types::TAG_BOOL, // false
    };

    let name_idx_u32 = name_idx as u32;

    // Search binding frames from innermost to outermost
    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        for frame_idx in (0..ctx_ref.binding_frames_count).rev() {
            let frame = &*ctx_ref.binding_frames.add(frame_idx);

            if frame.entries_count > 0 && !frame.entries.is_null() {
                for entry_idx in 0..frame.entries_count {
                    let entry = &*frame.entries.add(entry_idx);
                    if entry.name_idx == name_idx_u32 {
                        return super::types::TAG_BOOL | 1; // true
                    }
                }
            }
        }
    }

    super::types::TAG_BOOL // false
}

/// Clear all bindings in the current (innermost) binding frame.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_clear_bindings(ctx: *mut JitContext) {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return,
    };

    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        let frame = &mut *ctx_ref.binding_frames.add(ctx_ref.binding_frames_count - 1);
        frame.entries_count = 0;
        // Note: we don't deallocate entries array, just clear count for reuse
    }
}

/// Push a new binding frame onto the binding frame stack.
///
/// Creates a new scope level for bindings.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// 0 on success, -1 on stack overflow.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_binding_frame(ctx: *mut JitContext) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -2,
    };

    if ctx_ref.binding_frames.is_null() || ctx_ref.binding_frames_count >= ctx_ref.binding_frames_cap {
        return -1; // No capacity or overflow
    }

    // Create new frame with incremented scope depth
    let scope_depth = ctx_ref.binding_frames_count as u32;
    let new_frame = JitBindingFrame::new(scope_depth);
    *ctx_ref.binding_frames.add(ctx_ref.binding_frames_count) = new_frame;
    ctx_ref.binding_frames_count += 1;

    0 // Success
}

/// Pop the current binding frame from the binding frame stack.
///
/// Returns to the previous scope level.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// 0 on success, -1 if trying to pop the root frame.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pop_binding_frame(ctx: *mut JitContext) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -2,
    };

    if ctx_ref.binding_frames_count <= 1 {
        return -1; // Cannot pop root frame
    }

    // Get the frame being popped
    let frame_idx = ctx_ref.binding_frames_count - 1;
    let frame = &mut *ctx_ref.binding_frames.add(frame_idx);

    // Free entries array if allocated
    if !frame.entries.is_null() && frame.entries_cap > 0 {
        let layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
            .expect("Layout calculation failed");
        std::alloc::dealloc(frame.entries as *mut u8, layout);
        frame.entries = std::ptr::null_mut();
        frame.entries_count = 0;
        frame.entries_cap = 0;
    }

    ctx_ref.binding_frames_count -= 1;
    0 // Success
}

// =============================================================================
// Space Ops Phase 4: Binding Frame Forking for Nondeterminism
// =============================================================================

/// Saved binding frames snapshot for nondeterministic restoration.
///
/// This structure holds a deep copy of binding frames that can be restored
/// during backtracking. It is allocated on the heap and the pointer is stored
/// in JitAlternative::payload3 for SpaceMatch alternatives.
#[repr(C)]
pub struct JitSavedBindings {
    /// Array of saved binding frames
    pub frames: *mut JitBindingFrame,
    /// Number of frames in the array
    pub frames_count: usize,
    /// Total allocated capacity
    pub frames_cap: usize,
}

impl JitSavedBindings {
    /// Create a new empty saved bindings structure
    pub fn new() -> Self {
        Self {
            frames: std::ptr::null_mut(),
            frames_count: 0,
            frames_cap: 0,
        }
    }

    /// Check if this snapshot contains any frames
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.frames_count == 0 || self.frames.is_null()
    }
}

impl Default for JitSavedBindings {
    fn default() -> Self {
        Self::new()
    }
}

/// Fork (deep copy) all current binding frames for branch isolation.
///
/// Creates a complete snapshot of the current binding state that can be
/// restored during backtracking. This is essential for nondeterministic
/// operations like space matching where each alternative needs its own
/// isolated binding environment.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// Pointer to heap-allocated JitSavedBindings on success, null on failure.
/// The caller is responsible for freeing this via `jit_runtime_free_saved_bindings`.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fork_bindings(ctx: *const JitContext) -> *mut JitSavedBindings {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return std::ptr::null_mut(),
    };

    // If no binding frames, return empty snapshot
    if ctx_ref.binding_frames.is_null() || ctx_ref.binding_frames_count == 0 {
        let saved = Box::new(JitSavedBindings::new());
        return Box::into_raw(saved);
    }

    let frame_count = ctx_ref.binding_frames_count;

    // Allocate frame array
    let frames_layout = std::alloc::Layout::array::<JitBindingFrame>(frame_count)
        .expect("Layout calculation failed");
    let frames = std::alloc::alloc(frames_layout) as *mut JitBindingFrame;
    if frames.is_null() {
        return std::ptr::null_mut();
    }

    // Deep copy each frame
    for i in 0..frame_count {
        let src_frame = &*ctx_ref.binding_frames.add(i);
        let dst_frame = &mut *frames.add(i);

        dst_frame.scope_depth = src_frame.scope_depth;

        if src_frame.entries.is_null() || src_frame.entries_count == 0 {
            // Empty frame
            dst_frame.entries = std::ptr::null_mut();
            dst_frame.entries_count = 0;
            dst_frame.entries_cap = 0;
        } else {
            // Allocate and copy entries
            let entries_layout = std::alloc::Layout::array::<JitBindingEntry>(src_frame.entries_count)
                .expect("Layout calculation failed");
            let entries = std::alloc::alloc(entries_layout) as *mut JitBindingEntry;
            if entries.is_null() {
                // Cleanup already allocated frames on failure
                for j in 0..i {
                    let cleanup_frame = &*frames.add(j);
                    if !cleanup_frame.entries.is_null() && cleanup_frame.entries_cap > 0 {
                        let cleanup_layout = std::alloc::Layout::array::<JitBindingEntry>(cleanup_frame.entries_cap)
                            .expect("Layout calculation failed");
                        std::alloc::dealloc(cleanup_frame.entries as *mut u8, cleanup_layout);
                    }
                }
                std::alloc::dealloc(frames as *mut u8, frames_layout);
                return std::ptr::null_mut();
            }

            // Copy entry data
            std::ptr::copy_nonoverlapping(src_frame.entries, entries, src_frame.entries_count);
            dst_frame.entries = entries;
            dst_frame.entries_count = src_frame.entries_count;
            dst_frame.entries_cap = src_frame.entries_count; // Exact fit for snapshot
        }
    }

    // Create and return the saved bindings structure
    let saved = Box::new(JitSavedBindings {
        frames,
        frames_count: frame_count,
        frames_cap: frame_count,
    });
    Box::into_raw(saved)
}

/// Restore binding frames from a saved snapshot.
///
/// Replaces the current binding state with the saved snapshot. This is used
/// during backtracking to restore the binding environment to a previous state.
///
/// # Arguments
/// * `ctx` - JIT context pointer (mutable)
/// * `saved` - Pointer to saved bindings from `jit_runtime_fork_bindings`
/// * `consume` - If true, frees the saved bindings after restoration
///
/// # Returns
/// 0 on success, -1 on error.
///
/// # Safety
/// Both pointers must be valid. The saved pointer must come from
/// `jit_runtime_fork_bindings`.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_restore_bindings(
    ctx: *mut JitContext,
    saved: *mut JitSavedBindings,
    consume: bool,
) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -1,
    };

    let saved_ref = match saved.as_ref() {
        Some(s) => s,
        None => return -1,
    };

    // First, clear current binding frames (free entries)
    if !ctx_ref.binding_frames.is_null() {
        for i in 0..ctx_ref.binding_frames_count {
            let frame = &mut *ctx_ref.binding_frames.add(i);
            if !frame.entries.is_null() && frame.entries_cap > 0 {
                let layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                    .expect("Layout calculation failed");
                std::alloc::dealloc(frame.entries as *mut u8, layout);
                frame.entries = std::ptr::null_mut();
                frame.entries_count = 0;
                frame.entries_cap = 0;
            }
        }
    }

    // If saved is empty, just clear the count
    if saved_ref.is_empty() {
        ctx_ref.binding_frames_count = 0;
        if consume {
            let _ = Box::from_raw(saved);
        }
        return 0;
    }

    // Check capacity
    if saved_ref.frames_count > ctx_ref.binding_frames_cap {
        // Not enough capacity - this shouldn't happen in normal use
        // but we handle it gracefully
        if consume {
            jit_runtime_free_saved_bindings(saved);
        }
        return -2;
    }

    // Copy frames from saved to context
    for i in 0..saved_ref.frames_count {
        let src_frame = &*saved_ref.frames.add(i);
        let dst_frame = &mut *ctx_ref.binding_frames.add(i);

        dst_frame.scope_depth = src_frame.scope_depth;

        if src_frame.entries.is_null() || src_frame.entries_count == 0 {
            dst_frame.entries = std::ptr::null_mut();
            dst_frame.entries_count = 0;
            dst_frame.entries_cap = 0;
        } else {
            // Allocate and copy entries
            let entries_layout = std::alloc::Layout::array::<JitBindingEntry>(src_frame.entries_count)
                .expect("Layout calculation failed");
            let entries = std::alloc::alloc(entries_layout) as *mut JitBindingEntry;
            if entries.is_null() {
                ctx_ref.binding_frames_count = i;
                if consume {
                    jit_runtime_free_saved_bindings(saved);
                }
                return -3;
            }
            std::ptr::copy_nonoverlapping(src_frame.entries, entries, src_frame.entries_count);
            dst_frame.entries = entries;
            dst_frame.entries_count = src_frame.entries_count;
            dst_frame.entries_cap = src_frame.entries_count;
        }
    }

    ctx_ref.binding_frames_count = saved_ref.frames_count;

    // Consume the saved bindings if requested
    if consume {
        jit_runtime_free_saved_bindings(saved);
    }

    0 // Success
}

/// Free saved bindings without restoring them.
///
/// Used when discarding a choice point or when the alternative was never taken.
///
/// # Arguments
/// * `saved` - Pointer to saved bindings from `jit_runtime_fork_bindings`
///
/// # Safety
/// The pointer must come from `jit_runtime_fork_bindings`.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_free_saved_bindings(saved: *mut JitSavedBindings) {
    if saved.is_null() {
        return;
    }

    let saved_box = Box::from_raw(saved);

    // Free all entry arrays in frames
    if !saved_box.frames.is_null() {
        for i in 0..saved_box.frames_count {
            let frame = &*saved_box.frames.add(i);
            if !frame.entries.is_null() && frame.entries_cap > 0 {
                let layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                    .expect("Layout calculation failed");
                std::alloc::dealloc(frame.entries as *mut u8, layout);
            }
        }

        // Free frames array
        if saved_box.frames_cap > 0 {
            let frames_layout = std::alloc::Layout::array::<JitBindingFrame>(saved_box.frames_cap)
                .expect("Layout calculation failed");
            std::alloc::dealloc(saved_box.frames as *mut u8, frames_layout);
        }
    }

    // Box is dropped here, freeing the JitSavedBindings struct
}

/// Get the size of saved bindings (for debugging/metrics).
///
/// # Returns
/// Total number of binding entries across all frames, or 0 if invalid.
///
/// # Safety
/// The pointer must come from `jit_runtime_fork_bindings` or be null.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_saved_bindings_size(saved: *const JitSavedBindings) -> usize {
    let saved_ref = match saved.as_ref() {
        Some(s) => s,
        None => return 0,
    };

    if saved_ref.is_empty() {
        return 0;
    }

    let mut total = 0;
    for i in 0..saved_ref.frames_count {
        let frame = &*saved_ref.frames.add(i);
        total += frame.entries_count;
    }
    total
}

// =============================================================================
// Phase B: Pattern Matching Runtime Functions
// =============================================================================

// =============================================================================
// Optimization 3.1: Fast Path for Simple Pattern Matching
// =============================================================================

/// Attempt to match pattern against value using fast path (no heap allocation).
///
/// Returns:
/// - `Some(true)` if pattern definitely matches
/// - `Some(false)` if pattern definitely does not match
/// - `None` if fast path cannot determine (fall back to full implementation)
///
/// Fast path handles:
/// 1. Variable patterns ($x) - always match
/// 2. Wildcard patterns (_) - always match (via atom check)
/// 3. Ground value patterns (Long, Bool, Nil, Unit) - direct comparison
/// 4. Atom patterns - pointer comparison for interned strings
#[inline(always)]
fn try_pattern_match_fast_path(pattern: u64, value: u64) -> Option<bool> {
    let pattern_jit = JitValue::from_raw(pattern);
    let value_jit = JitValue::from_raw(value);

    // Fast path 1: Variable pattern matches anything
    if pattern_jit.is_var() {
        return Some(true);
    }

    // Fast path 2: Wildcard atom "_" matches anything
    if pattern_jit.is_atom() {
        // Check if this is the wildcard
        let pattern_ptr = (pattern & super::types::PAYLOAD_MASK) as *const String;
        if !pattern_ptr.is_null() {
            // SAFETY: We checked is_atom() and the pointer is from our NaN-boxing
            let pattern_str = unsafe { &*pattern_ptr };
            if pattern_str == "_" {
                return Some(true);
            }
            // Also check for variable prefix in atom form
            if pattern_str.starts_with('$') {
                return Some(true);
            }
        }
    }

    // Fast path 3: Same-type ground values - direct comparison
    let pattern_tag = pattern_jit.tag();
    let value_tag = value_jit.tag();

    // Long comparison
    if pattern_tag == super::types::TAG_LONG && value_tag == super::types::TAG_LONG {
        // Compare raw payloads directly (both are 48-bit signed integers)
        return Some((pattern & super::types::PAYLOAD_MASK) == (value & super::types::PAYLOAD_MASK));
    }

    // Bool comparison
    if pattern_tag == super::types::TAG_BOOL && value_tag == super::types::TAG_BOOL {
        return Some((pattern & 1) == (value & 1));
    }

    // Nil comparison
    if pattern_tag == super::types::TAG_NIL && value_tag == super::types::TAG_NIL {
        return Some(true);
    }

    // Unit comparison
    if pattern_tag == super::types::TAG_UNIT && value_tag == super::types::TAG_UNIT {
        return Some(true);
    }

    // Atom comparison (interned string pointer equality)
    if pattern_tag == super::types::TAG_ATOM && value_tag == super::types::TAG_ATOM {
        // Same pointer means same interned string
        let pattern_ptr = pattern & super::types::PAYLOAD_MASK;
        let value_ptr = value & super::types::PAYLOAD_MASK;
        if pattern_ptr == value_ptr {
            return Some(true);
        }
        // Different pointers - need to compare string contents
        // Fall back to full implementation for safety
        // (interning should make this rare)
    }

    // Fast path 4: Type mismatch for ground values = no match
    // If pattern is a ground type and value is a different ground type, no match
    let pattern_is_ground = matches!(
        pattern_tag,
        super::types::TAG_LONG | super::types::TAG_BOOL | super::types::TAG_NIL | super::types::TAG_UNIT
    );
    let value_is_ground = matches!(
        value_tag,
        super::types::TAG_LONG | super::types::TAG_BOOL | super::types::TAG_NIL | super::types::TAG_UNIT
    );

    if pattern_is_ground && value_is_ground && pattern_tag != value_tag {
        return Some(false);
    }

    // Cannot determine with fast path
    None
}

/// Pattern match value against pattern (without binding).
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `pattern` - NaN-boxed pattern value
/// * `value` - NaN-boxed value to match against
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed Bool: true if pattern matches value, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pattern_match(
    ctx: *const JitContext,
    pattern: u64,
    value: u64,
    _ip: u64,
) -> u64 {
    let _ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return super::types::TAG_BOOL, // false
    };

    // Optimization 3.1: Try fast path first
    if let Some(result) = try_pattern_match_fast_path(pattern, value) {
        return if result {
            super::types::TAG_BOOL | 1 // true
        } else {
            super::types::TAG_BOOL // false
        };
    }

    // Fall back to full implementation for complex patterns
    let pattern_val = JitValue::from_raw(pattern).to_metta();
    let value_val = JitValue::from_raw(value).to_metta();

    let matches = pattern_matches_impl(&pattern_val, &value_val);

    if matches {
        super::types::TAG_BOOL | 1 // true
    } else {
        super::types::TAG_BOOL // false
    }
}

// =============================================================================
// Variable Index Cache (Optimization 5.3)
// =============================================================================

/// Compute a simple hash of a variable name for cache lookup.
/// Uses FNV-1a hash for speed and good distribution.
#[inline(always)]
fn hash_var_name(name: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Look up variable name index with caching (Optimization 5.3).
///
/// Uses a direct-mapped cache in JitContext to avoid O(n) constant array scans
/// for repeated variable bindings.
///
/// # Arguments
/// * `ctx` - JIT context pointer (mutable for cache updates)
/// * `name` - Variable name to look up
/// * `constants` - Constant array slice
///
/// # Returns
/// Some(index) if found, None if not found in constants.
///
/// # Safety
/// The context pointer must be valid.
#[inline(always)]
unsafe fn lookup_var_index_cached(
    ctx: *mut JitContext,
    name: &str,
    constants: &[MettaValue],
) -> Option<usize> {
    let name_hash = hash_var_name(name);
    let cache_slot = (name_hash as usize) % super::types::VAR_INDEX_CACHE_SIZE;

    // Check cache first
    if let Some(ctx_ref) = ctx.as_ref() {
        let (cached_hash, cached_idx) = ctx_ref.var_index_cache[cache_slot];
        if cached_hash == name_hash && cached_idx != u32::MAX {
            // Verify the cached index is valid and matches the name
            let idx = cached_idx as usize;
            if idx < constants.len() {
                if let MettaValue::Atom(s) = &constants[idx] {
                    if s == name {
                        return Some(idx);
                    }
                }
            }
            // Cache hit but stale - fall through to linear search
        }
    }

    // Cache miss - linear search
    let name_idx = constants.iter().position(|c| {
        matches!(c, MettaValue::Atom(s) if s == name)
    });

    // Update cache on successful lookup
    if let Some(idx) = name_idx {
        if let Some(ctx_ref) = ctx.as_mut() {
            ctx_ref.var_index_cache[cache_slot] = (name_hash, idx as u32);
        }
    }

    name_idx
}

/// Fast path for pattern match with binding.
/// Returns:
/// - `Some(true)` if pattern matches and binding was handled
/// - `Some(false)` if pattern definitely does not match
/// - `None` if fast path cannot handle (fall back to full implementation)
#[inline(always)]
unsafe fn try_pattern_match_bind_fast_path(
    ctx: *mut JitContext,
    pattern: u64,
    value: u64,
) -> Option<bool> {
    let pattern_jit = JitValue::from_raw(pattern);
    let pattern_tag = pattern_jit.tag();
    let value_jit = JitValue::from_raw(value);
    let value_tag = value_jit.tag();

    // Fast path 1: Variable pattern - bind value and return true
    if pattern_jit.is_var() {
        // Get the variable name from the pattern
        let var_ptr = (pattern & super::types::PAYLOAD_MASK) as *const String;
        if !var_ptr.is_null() {
            let var_name = &*var_ptr;

            // Find the variable name in constants to get its index (Optimization 5.3: cached lookup)
            let ctx_ref = ctx.as_ref()?;
            if ctx_ref.constants_len > 0 && !ctx_ref.constants.is_null() {
                let constants = std::slice::from_raw_parts(ctx_ref.constants, ctx_ref.constants_len);
                let name_idx = lookup_var_index_cached(ctx, var_name, constants);

                if let Some(idx) = name_idx {
                    let store_result = jit_runtime_store_binding(ctx, idx as u64, value, 0);
                    return Some(store_result == 0);
                }
            }
        }
        // Could not store binding, fall back
        return None;
    }

    // Fast path 2: Atom pattern - check for wildcard or variable in atom form
    if pattern_jit.is_atom() {
        let pattern_ptr = (pattern & super::types::PAYLOAD_MASK) as *const String;
        if !pattern_ptr.is_null() {
            let pattern_str = &*pattern_ptr;

            // Wildcard matches without binding
            if pattern_str == "_" {
                return Some(true);
            }

            // Variable in atom form - bind and return (Optimization 5.3: cached lookup)
            if pattern_str.starts_with('$') {
                let ctx_ref = ctx.as_ref()?;
                if ctx_ref.constants_len > 0 && !ctx_ref.constants.is_null() {
                    let constants = std::slice::from_raw_parts(ctx_ref.constants, ctx_ref.constants_len);
                    let name_idx = lookup_var_index_cached(ctx, pattern_str, constants);

                    if let Some(idx) = name_idx {
                        let store_result = jit_runtime_store_binding(ctx, idx as u64, value, 0);
                        return Some(store_result == 0);
                    }
                }
                return None; // Fall back
            }

            // Regular atom - compare with value
            if value_tag == super::types::TAG_ATOM {
                let value_ptr = (value & super::types::PAYLOAD_MASK) as *const String;
                if !value_ptr.is_null() {
                    let value_str = &*value_ptr;
                    return Some(pattern_str == value_str);
                }
            }
        }
    }

    // Fast path 3: Ground value comparison (no binding needed)
    // Long comparison
    if pattern_tag == super::types::TAG_LONG && value_tag == super::types::TAG_LONG {
        return Some((pattern & super::types::PAYLOAD_MASK) == (value & super::types::PAYLOAD_MASK));
    }

    // Bool comparison
    if pattern_tag == super::types::TAG_BOOL && value_tag == super::types::TAG_BOOL {
        return Some((pattern & 1) == (value & 1));
    }

    // Nil comparison
    if pattern_tag == super::types::TAG_NIL && value_tag == super::types::TAG_NIL {
        return Some(true);
    }

    // Unit comparison
    if pattern_tag == super::types::TAG_UNIT && value_tag == super::types::TAG_UNIT {
        return Some(true);
    }

    // Fast path 4: Type mismatch for ground values = no match
    let pattern_is_ground = matches!(
        pattern_tag,
        super::types::TAG_LONG | super::types::TAG_BOOL | super::types::TAG_NIL | super::types::TAG_UNIT
    );
    let value_is_ground = matches!(
        value_tag,
        super::types::TAG_LONG | super::types::TAG_BOOL | super::types::TAG_NIL | super::types::TAG_UNIT
    );

    if pattern_is_ground && value_is_ground && pattern_tag != value_tag {
        return Some(false);
    }

    // Cannot determine with fast path
    None
}

/// Pattern match value against pattern with variable binding.
///
/// # Arguments
/// * `ctx` - JIT context pointer (mutable for bindings)
/// * `pattern` - NaN-boxed pattern value
/// * `value` - NaN-boxed value to match against
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed Bool: true if pattern matches and bindings were added, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pattern_match_bind(
    ctx: *mut JitContext,
    pattern: u64,
    value: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_BOOL, // false
    };

    // Optimization 3.1: Try fast path first
    if let Some(result) = try_pattern_match_bind_fast_path(ctx, pattern, value) {
        return if result {
            super::types::TAG_BOOL | 1 // true
        } else {
            super::types::TAG_BOOL // false
        };
    }

    // Fall back to full implementation for complex patterns
    let pattern_val = JitValue::from_raw(pattern).to_metta();
    let value_val = JitValue::from_raw(value).to_metta();

    let mut bindings = Vec::new();
    if !pattern_match_bind_impl(&pattern_val, &value_val, &mut bindings) {
        return super::types::TAG_BOOL; // false
    }

    // Add bindings to the current binding frame
    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        // Get constants for name lookup
        let constants = if !ctx_ref.constants.is_null() && ctx_ref.constants_len > 0 {
            std::slice::from_raw_parts(ctx_ref.constants, ctx_ref.constants_len)
        } else {
            &[]
        };

        for (name, val) in bindings {
            // Find or allocate name_idx for this variable name (Optimization 5.3: cached lookup)
            let name_idx = lookup_var_index_cached(ctx, &name, constants);

            if let Some(idx) = name_idx {
                let jit_val = metta_to_jit(&val);
                let store_result = jit_runtime_store_binding(ctx, idx as u64, jit_val.to_bits(), 0);
                if store_result != 0 {
                    return super::types::TAG_BOOL; // false - binding failed
                }
            }
            // If name not in constants, we skip binding (this is a limitation)
        }
    }

    super::types::TAG_BOOL | 1 // true
}

/// Check if S-expression has expected arity.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `value` - NaN-boxed value (should be S-expression)
/// * `expected_arity` - Expected number of elements
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed Bool: true if value is S-expression with expected arity, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_match_arity(
    _ctx: *const JitContext,
    value: u64,
    expected_arity: u64,
    _ip: u64,
) -> u64 {
    let val = JitValue::from_raw(value).to_metta();

    let matches = match val {
        crate::backend::models::MettaValue::SExpr(items) => items.len() == expected_arity as usize,
        _ => false,
    };

    if matches {
        super::types::TAG_BOOL | 1 // true
    } else {
        super::types::TAG_BOOL // false
    }
}

/// Check if S-expression head matches expected symbol.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `value` - NaN-boxed value (should be S-expression)
/// * `expected_head_idx` - Index of expected head symbol in constant pool
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed Bool: true if value is S-expression with matching head, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_match_head(
    ctx: *const JitContext,
    value: u64,
    expected_head_idx: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return super::types::TAG_BOOL, // false
    };

    let val = JitValue::from_raw(value).to_metta();

    // Get the expected head from the constant pool
    let expected_head = if !ctx_ref.constants.is_null()
        && expected_head_idx < ctx_ref.constants_len as u64
    {
        &*ctx_ref.constants.add(expected_head_idx as usize)
    } else {
        return super::types::TAG_BOOL; // false - invalid index
    };

    let matches = match val {
        crate::backend::models::MettaValue::SExpr(items) if !items.is_empty() => {
            &items[0] == expected_head
        }
        _ => false,
    };

    if matches {
        super::types::TAG_BOOL | 1 // true
    } else {
        super::types::TAG_BOOL // false
    }
}

/// Bidirectional unification of two values.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `a` - First NaN-boxed value
/// * `b` - Second NaN-boxed value
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed Bool: true if values unify, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_unify(
    _ctx: *const JitContext,
    a: u64,
    b: u64,
    _ip: u64,
) -> u64 {
    let a_val = JitValue::from_raw(a).to_metta();
    let b_val = JitValue::from_raw(b).to_metta();

    let mut bindings = Vec::new();
    let unifies = unify_impl(&a_val, &b_val, &mut bindings);

    if unifies {
        super::types::TAG_BOOL | 1 // true
    } else {
        super::types::TAG_BOOL // false
    }
}

/// Bidirectional unification with variable binding.
///
/// # Arguments
/// * `ctx` - JIT context pointer (mutable for bindings)
/// * `a` - First NaN-boxed value
/// * `b` - Second NaN-boxed value
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed Bool: true if values unify and bindings were added, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_unify_bind(
    ctx: *mut JitContext,
    a: u64,
    b: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_BOOL, // false
    };

    let a_val = JitValue::from_raw(a).to_metta();
    let b_val = JitValue::from_raw(b).to_metta();

    let mut bindings = Vec::new();
    if !unify_impl(&a_val, &b_val, &mut bindings) {
        return super::types::TAG_BOOL; // false
    }

    // Add bindings to the current binding frame
    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        let constants = if !ctx_ref.constants.is_null() && ctx_ref.constants_len > 0 {
            std::slice::from_raw_parts(ctx_ref.constants, ctx_ref.constants_len)
        } else {
            &[]
        };

        for (name, val) in bindings {
            // Optimization 5.3: cached lookup
            let name_idx = lookup_var_index_cached(ctx, &name, constants);

            if let Some(idx) = name_idx {
                let jit_val = metta_to_jit(&val);
                let store_result = jit_runtime_store_binding(ctx, idx as u64, jit_val.to_bits(), 0);
                if store_result != 0 {
                    return super::types::TAG_BOOL; // false - binding failed
                }
            }
        }
    }

    super::types::TAG_BOOL | 1 // true
}

// =============================================================================
// Pattern Matching Helper Functions
// =============================================================================

/// Convert a MettaValue to a JitValue
///
/// For simple types (Long, Bool, Nil, Unit), creates a NaN-boxed value directly.
/// For complex types (SExpr, Atom, String, etc.), boxes the value and returns a heap pointer.
fn metta_to_jit(val: &crate::backend::models::MettaValue) -> JitValue {
    use crate::backend::models::MettaValue;

    match val {
        MettaValue::Long(n) => JitValue::from_long(*n),
        MettaValue::Bool(b) => JitValue::from_bool(*b),
        MettaValue::Nil => JitValue::nil(),
        MettaValue::Unit => JitValue::unit(),
        // For complex types, box and return heap pointer
        other => {
            let boxed = Box::new(other.clone());
            JitValue::from_heap_ptr(Box::into_raw(boxed))
        }
    }
}

/// Convert a MettaValue to a JitValue with heap tracking.
///
/// For simple types (Long, Bool, Nil, Unit), creates a NaN-boxed value directly.
/// For complex types (SExpr, Atom, String, etc.), boxes the value, tracks the
/// allocation in the context, and returns a heap pointer.
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext (or null to disable tracking)
unsafe fn metta_to_jit_tracked(
    val: &crate::backend::models::MettaValue,
    ctx: *mut JitContext,
) -> JitValue {
    use crate::backend::models::MettaValue;

    match val {
        MettaValue::Long(n) => JitValue::from_long(*n),
        MettaValue::Bool(b) => JitValue::from_bool(*b),
        MettaValue::Nil => JitValue::nil(),
        MettaValue::Unit => JitValue::unit(),
        // For complex types, box, track, and return heap pointer
        other => {
            let boxed = Box::new(other.clone());
            let ptr = Box::into_raw(boxed);
            // Track the allocation if context has heap tracking enabled
            if let Some(ctx_ref) = ctx.as_mut() {
                ctx_ref.track_heap_allocation(ptr);
            }
            JitValue::from_heap_ptr(ptr)
        }
    }
}

/// Pattern match implementation (without binding)
fn pattern_matches_impl(
    pattern: &crate::backend::models::MettaValue,
    value: &crate::backend::models::MettaValue,
) -> bool {
    use crate::backend::models::MettaValue;

    match (pattern, value) {
        // Variable matches anything (Atom starting with $)
        (MettaValue::Atom(s), _) if s.starts_with('$') => true,
        // Wildcard matches anything
        (MettaValue::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        // Exact match for literals
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        // S-expression matching
        (MettaValue::SExpr(ps), MettaValue::SExpr(vs)) => {
            ps.len() == vs.len()
                && ps
                    .iter()
                    .zip(vs.iter())
                    .all(|(p, v)| pattern_matches_impl(p, v))
        }
        _ => false,
    }
}

/// Pattern match with binding implementation
fn pattern_match_bind_impl(
    pattern: &crate::backend::models::MettaValue,
    value: &crate::backend::models::MettaValue,
    bindings: &mut Vec<(String, crate::backend::models::MettaValue)>,
) -> bool {
    use crate::backend::models::MettaValue;

    match (pattern, value) {
        // Variable binds to value (Atom starting with $)
        (MettaValue::Atom(name), val) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        // Wildcard matches without binding
        (MettaValue::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        // Exact match for literals
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        // S-expression matching
        (MettaValue::SExpr(ps), MettaValue::SExpr(vs)) => {
            ps.len() == vs.len()
                && ps
                    .iter()
                    .zip(vs.iter())
                    .all(|(p, v)| pattern_match_bind_impl(p, v, bindings))
        }
        _ => false,
    }
}

/// Unification implementation (bidirectional)
fn unify_impl(
    a: &crate::backend::models::MettaValue,
    b: &crate::backend::models::MettaValue,
    bindings: &mut Vec<(String, crate::backend::models::MettaValue)>,
) -> bool {
    use crate::backend::models::MettaValue;

    match (a, b) {
        // Variables unify with anything (Atom starting with $)
        (MettaValue::Atom(name), val) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        (val, MettaValue::Atom(name)) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        // Wildcard matches without binding (both directions)
        (MettaValue::Atom(s), _) if s == "_" => true,
        (_, MettaValue::Atom(s)) if s == "_" => true,
        // Same structure
        (MettaValue::Atom(x), MettaValue::Atom(y)) => x == y,
        (MettaValue::Long(x), MettaValue::Long(y)) => x == y,
        (MettaValue::Bool(x), MettaValue::Bool(y)) => x == y,
        (MettaValue::String(x), MettaValue::String(y)) => x == y,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        (MettaValue::SExpr(xs), MettaValue::SExpr(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| unify_impl(x, y, bindings))
        }
        _ => false,
    }
}

// =============================================================================
// Phase D: Space Operations
// =============================================================================

/// Add an atom to a space
///
/// Stack: [space, atom] -> [Unit]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `space` - NaN-boxed space handle (TAG_HEAP pointing to MettaValue::Space)
/// * `atom` - NaN-boxed atom to add
/// * `_ip` - Instruction pointer (for debugging)
///
/// # Returns
/// NaN-boxed Unit value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_add(
    _ctx: *mut JitContext,
    space: u64,
    atom: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;

    let space_val = JitValue::from_raw(space);
    let atom_val = JitValue::from_raw(atom);

    let space_metta = space_val.to_metta();
    let atom_metta = atom_val.to_metta();

    match space_metta {
        MettaValue::Space(handle) => {
            handle.add_atom(atom_metta);
            JitValue::unit().to_bits()
        }
        _ => {
            // Type error - not a space
            JitValue::unit().to_bits()
        }
    }
}

/// Remove an atom from a space
///
/// Stack: [space, atom] -> [Bool]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `space` - NaN-boxed space handle
/// * `atom` - NaN-boxed atom to remove
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Bool - true if atom was found and removed, false otherwise
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_remove(
    _ctx: *mut JitContext,
    space: u64,
    atom: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;

    let space_val = JitValue::from_raw(space);
    let atom_val = JitValue::from_raw(atom);

    let space_metta = space_val.to_metta();
    let atom_metta = atom_val.to_metta();

    match space_metta {
        MettaValue::Space(handle) => {
            let removed = handle.remove_atom(&atom_metta);
            JitValue::from_bool(removed).to_bits()
        }
        _ => {
            // Type error - not a space
            JitValue::from_bool(false).to_bits()
        }
    }
}

/// Get all atoms from a space
///
/// Stack: [space] -> [SExpr]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `space` - NaN-boxed space handle
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed SExpr containing all atoms in the space
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_get_atoms(
    _ctx: *mut JitContext,
    space: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;

    let space_val = JitValue::from_raw(space);
    let space_metta = space_val.to_metta();

    match space_metta {
        MettaValue::Space(handle) => {
            let atoms = handle.collapse();
            metta_to_jit(&MettaValue::SExpr(atoms)).to_bits()
        }
        _ => {
            // Type error - return empty S-expression
            metta_to_jit(&MettaValue::SExpr(vec![])).to_bits()
        }
    }
}

/// Match a pattern against all atoms in a space
///
/// Stack: [space, pattern, template] -> [SExpr]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `space` - NaN-boxed space handle
/// * `pattern` - NaN-boxed pattern to match
/// * `_template` - NaN-boxed template (currently ignored, simplified impl)
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed SExpr containing matching atoms
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_match(
    _ctx: *mut JitContext,
    space: u64,
    pattern: u64,
    _template: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;

    let space_val = JitValue::from_raw(space);
    let pattern_val = JitValue::from_raw(pattern);

    let space_metta = space_val.to_metta();
    let pattern_metta = pattern_val.to_metta();

    match space_metta {
        MettaValue::Space(handle) => {
            let atoms = handle.collapse();
            let mut results = Vec::new();

            // Simple pattern matching against atoms
            for atom in &atoms {
                if pattern_matches_impl(&pattern_metta, atom) {
                    results.push(atom.clone());
                }
            }

            metta_to_jit(&MettaValue::SExpr(results)).to_bits()
        }
        _ => {
            // Type error - return empty S-expression
            metta_to_jit(&MettaValue::SExpr(vec![])).to_bits()
        }
    }
}

// =============================================================================
// Space Ops Phase 5: Nondeterministic Space Match
// =============================================================================

/// Nondeterministic space match with choice point creation.
///
/// This function performs pattern matching against all atoms in a space,
/// creating choice points for alternatives when multiple matches exist.
/// It implements the nondeterministic semantics required for MeTTa's `match` form.
///
/// # Arguments
/// * `ctx` - JIT context pointer (mutable for choice point creation)
/// * `space` - NaN-boxed Space value
/// * `pattern` - NaN-boxed pattern expression
/// * `template` - NaN-boxed template expression for result instantiation
/// * `ip` - Instruction pointer for resumption
///
/// # Returns
/// - On single match: NaN-boxed result (template instantiated with bindings)
/// - On multiple matches: First result, with choice points created for rest
/// - On no match: TAG_NIL
/// - On error: TAG_NIL with bailout flag set
///
/// # Semantics
/// ```text
/// 0 matches: return nil (empty)
/// 1 match:   return instantiate(template, bindings[0])
/// N matches: return instantiate(template, bindings[0])
///            + create N-1 choice points for alternatives
///            + signal YIELD if in nondet mode
/// ```
///
/// # Safety
/// The context pointer must be valid. The space value must be a valid SpaceHandle.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_match_nondet(
    ctx: *mut JitContext,
    space: u64,
    pattern: u64,
    template: u64,
    ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;
    use super::types::{JitChoicePoint, JitAlternative, JitAlternativeTag};

    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return super::types::TAG_NIL,
    };

    let space_val = JitValue::from_raw(space);
    let pattern_val = JitValue::from_raw(pattern);
    let template_val = JitValue::from_raw(template);

    let space_metta = space_val.to_metta();
    let pattern_metta = pattern_val.to_metta();
    let template_metta = template_val.to_metta();

    // Validate we have a space
    let handle = match &space_metta {
        MettaValue::Space(h) => h,
        _ => {
            // Type error - not a space
            ctx_ref.bailout = true;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            ctx_ref.bailout_ip = ip as usize;
            return super::types::TAG_NIL;
        }
    };

    // Collapse space to get all atoms
    let atoms = handle.collapse();
    if atoms.is_empty() {
        // No atoms in space - return nil (empty result)
        return super::types::TAG_NIL;
    }

    // Collect matching atoms with their bindings
    let mut matches: Vec<(MettaValue, Vec<(String, MettaValue)>)> = Vec::new();

    for atom in &atoms {
        let mut bindings = Vec::new();
        if pattern_matches_with_bindings_impl(&pattern_metta, atom, &mut bindings) {
            matches.push((atom.clone(), bindings));
        }
    }

    let match_count = matches.len();

    if match_count == 0 {
        // No matches - return nil
        return super::types::TAG_NIL;
    }

    // Take first match
    let (_first_atom, first_bindings) = matches.remove(0);

    // Instantiate template with first bindings
    let first_result = instantiate_template_impl(&template_metta, &first_bindings);
    let first_jit = metta_to_jit(&first_result);

    if match_count == 1 {
        // Single match - just return the result
        return first_jit.to_bits();
    }

    // Multiple matches - create choice point for alternatives 2..N
    // First, check if we have capacity for a choice point
    if ctx_ref.choice_points.is_null() || ctx_ref.choice_point_count >= ctx_ref.choice_point_cap {
        // No choice point capacity - fall back to bailout
        ctx_ref.bailout = true;
        ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
        ctx_ref.bailout_ip = ip as usize;
        return first_jit.to_bits();
    }

    // Optimization 5.2: Check if alternatives fit inline
    let alt_count = matches.len();
    if alt_count > super::MAX_ALTERNATIVES_INLINE {
        // Too many alternatives - bailout to VM
        ctx_ref.bailout = true;
        ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
        ctx_ref.bailout_ip = ip as usize;
        return first_jit.to_bits();
    }

    // Get choice point and initialize
    let cp = &mut *ctx_ref.choice_points.add(ctx_ref.choice_point_count);
    cp.saved_sp = ctx_ref.sp as u64;
    cp.alt_count = alt_count as u64;
    cp.current_index = 0;
    cp.saved_ip = ip;
    cp.saved_chunk = ctx_ref.current_chunk;
    cp.saved_stack_pool_idx = -1;
    cp.saved_stack_count = 0;
    cp.fork_depth = ctx_ref.fork_depth;
    cp.saved_binding_frames_count = ctx_ref.binding_frames_count;
    cp.is_collect_boundary = false;

    // For each remaining match, create a SpaceMatch alternative
    for (idx, (_atom, bindings)) in matches.into_iter().enumerate() {
        // Fork current bindings for this alternative
        let saved_bindings = jit_runtime_fork_bindings(ctx as *const JitContext);
        if saved_bindings.is_null() {
            // Fork failed - cleanup previously created alternatives and bailout
            for cleanup_idx in 0..idx {
                let cleanup_alt = &cp.alternatives_inline[cleanup_idx];
                if cleanup_alt.tag == JitAlternativeTag::SpaceMatch && cleanup_alt.payload3 != 0 {
                    jit_runtime_free_saved_bindings(cleanup_alt.payload3 as *mut JitSavedBindings);
                }
            }
            ctx_ref.bailout = true;
            ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
            ctx_ref.bailout_ip = ip as usize;
            return first_jit.to_bits();
        }

        // Apply this alternative's bindings to the forked snapshot
        apply_bindings_to_saved(saved_bindings, &bindings);

        // Pre-instantiate the result for this alternative
        let alt_result = instantiate_template_impl(&template_metta, &bindings);
        let alt_jit = metta_to_jit(&alt_result);

        // Optimization 5.2: Store alternative inline
        cp.alternatives_inline[idx] = JitAlternative {
            tag: JitAlternativeTag::SpaceMatch,
            payload: alt_jit.to_bits(),    // Pre-computed result
            payload2: 0,                    // Unused (bindings already applied)
            payload3: saved_bindings as u64,
        };
    }

    ctx_ref.choice_point_count += 1;

    first_jit.to_bits()
}

/// Helper: Apply bindings to a saved bindings snapshot.
///
/// This stores the bindings from a match operation into the saved frames
/// so they're available when the alternative is taken during backtracking.
unsafe fn apply_bindings_to_saved(
    saved: *mut JitSavedBindings,
    bindings: &[(String, MettaValue)],
) {
    use crate::backend::models::MettaValue;

    let saved_ref = match saved.as_mut() {
        Some(s) => s,
        None => return,
    };

    if saved_ref.is_empty() || bindings.is_empty() {
        return;
    }

    // Get the current (last) frame to apply bindings to
    let frame_idx = saved_ref.frames_count.saturating_sub(1);
    let frame = &mut *saved_ref.frames.add(frame_idx);

    // Ensure capacity for new bindings
    let new_count = frame.entries_count + bindings.len();
    if frame.entries.is_null() || new_count > frame.entries_cap {
        let new_cap = new_count.max(8);
        let layout = std::alloc::Layout::array::<JitBindingEntry>(new_cap)
            .expect("Layout calculation failed");
        let new_entries = std::alloc::alloc(layout) as *mut JitBindingEntry;
        if new_entries.is_null() {
            return; // Allocation failed - bindings won't be stored
        }

        // Copy existing entries
        if !frame.entries.is_null() && frame.entries_count > 0 {
            std::ptr::copy_nonoverlapping(frame.entries, new_entries, frame.entries_count);
            if frame.entries_cap > 0 {
                let old_layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                    .expect("Layout calculation failed");
                std::alloc::dealloc(frame.entries as *mut u8, old_layout);
            }
        }
        frame.entries = new_entries;
        frame.entries_cap = new_cap;
    }

    // Add bindings as entries
    for (name, value) in bindings {
        // Hash the name to get a name_idx (simple hash for now)
        let name_hash = {
            let mut h: u32 = 0;
            for b in name.bytes() {
                h = h.wrapping_mul(31).wrapping_add(b as u32);
            }
            h
        };

        let entry_ptr = frame.entries.add(frame.entries_count);
        *entry_ptr = JitBindingEntry {
            name_idx: name_hash,
            value: metta_to_jit(value),
        };
        frame.entries_count += 1;
    }
}

/// Pattern matching with binding extraction.
///
/// Like `pattern_matches_impl` but also collects variable bindings.
/// Variables are atoms starting with '$'.
fn pattern_matches_with_bindings_impl(
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Vec<(String, MettaValue)>,
) -> bool {
    use crate::backend::models::MettaValue;

    match (pattern, value) {
        // Variable pattern (atom starting with $) - always matches and binds
        (MettaValue::Atom(var), _) if var.starts_with('$') => {
            bindings.push((var.clone(), value.clone()));
            true
        }

        // Wildcard - always matches
        (MettaValue::Atom(s), _) if s == "_" => true,

        // Same type matching
        (MettaValue::Atom(p), MettaValue::Atom(v)) => p == v,
        (MettaValue::Long(p), MettaValue::Long(v)) => p == v,
        (MettaValue::Bool(p), MettaValue::Bool(v)) => p == v,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        (MettaValue::String(p), MettaValue::String(v)) => p == v,

        // S-expression matching - recursive with same length
        (MettaValue::SExpr(pats), MettaValue::SExpr(vals)) => {
            if pats.len() != vals.len() {
                return false;
            }
            for (p, v) in pats.iter().zip(vals.iter()) {
                if !pattern_matches_with_bindings_impl(p, v, bindings) {
                    return false;
                }
            }
            true
        }

        _ => false,
    }
}

/// Instantiate a template expression with bindings.
///
/// Replaces variables in the template with their bound values.
/// Variables are atoms starting with '$'.
fn instantiate_template_impl(
    template: &MettaValue,
    bindings: &[(String, MettaValue)],
) -> MettaValue {
    use crate::backend::models::MettaValue;

    match template {
        // Variable substitution (atoms starting with $)
        MettaValue::Atom(var) if var.starts_with('$') => {
            for (name, value) in bindings {
                if name == var {
                    return value.clone();
                }
            }
            // Unbound variable - keep as-is
            template.clone()
        }

        // S-expression - recurse
        MettaValue::SExpr(items) => {
            MettaValue::SExpr(
                items.iter()
                    .map(|item| instantiate_template_impl(item, bindings))
                    .collect()
            )
        }

        // Conjunction - recurse
        MettaValue::Conjunction(items) => {
            MettaValue::Conjunction(
                items.iter()
                    .map(|item| instantiate_template_impl(item, bindings))
                    .collect()
            )
        }

        // All other values pass through unchanged
        _ => template.clone(),
    }
}

/// Resume space match from a SpaceMatch alternative during backtracking.
///
/// This function is called by the backtracking handler when a SpaceMatch
/// choice point is taken. It restores bindings and returns the pre-computed result.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `alt` - Pointer to the JitAlternative being taken
///
/// # Returns
/// The pre-computed result from the alternative's payload.
///
/// # Safety
/// The context and alternative pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_resume_space_match(
    ctx: *mut JitContext,
    alt: *const JitAlternative,
) -> u64 {
    use super::types::JitAlternativeTag;

    let alt_ref = match alt.as_ref() {
        Some(a) => a,
        None => return super::types::TAG_NIL,
    };

    debug_assert_eq!(alt_ref.tag, JitAlternativeTag::SpaceMatch);

    // Restore bindings if we have a saved snapshot
    if alt_ref.payload3 != 0 {
        let saved = alt_ref.payload3 as *mut JitSavedBindings;
        // Restore and consume the saved bindings
        jit_runtime_restore_bindings(ctx, saved, true);
    }

    // Return the pre-computed result
    alt_ref.payload
}

/// Free saved bindings from SpaceMatch alternatives in a choice point.
///
/// Called when a choice point is exhausted to clean up saved bindings.
/// With Optimization 5.2, alternatives are inline so we don't free the array.
///
/// # Safety
/// The choice point pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_free_space_match_alternatives(
    cp: *mut JitChoicePoint,
) {
    use super::types::JitAlternativeTag;

    let cp_ref = match cp.as_ref() {
        Some(c) => c,
        None => return,
    };

    if cp_ref.alt_count == 0 {
        return;
    }

    // Free any remaining saved bindings in alternatives
    // Optimization 5.2: Alternatives are now inline, so we access alternatives_inline
    for i in cp_ref.current_index..cp_ref.alt_count {
        let alt = &cp_ref.alternatives_inline[i as usize];
        if alt.tag == JitAlternativeTag::SpaceMatch && alt.payload3 != 0 {
            jit_runtime_free_saved_bindings(alt.payload3 as *mut JitSavedBindings);
        }
    }

    // Optimization 5.2: Alternatives are inline, no need to free the array
}

// =============================================================================
// Phase C: Rule Dispatch Operations
// =============================================================================

/// Dispatch rules for an expression, returning the count of matching rules
///
/// Stack: [expr] -> [count]
///
/// # Arguments
/// * `ctx` - JIT context (stores matching rules internally)
/// * `expr` - NaN-boxed expression to match against rules
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Long - number of matching rules (0 if no match)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_dispatch_rules(
    ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return box_long(0),
    };

    // Check if bridge is available
    if ctx_ref.bridge_ptr.is_null() {
        return box_long(0);
    }

    // Convert expression to MettaValue
    let expr_val = JitValue::from_raw(expr);
    let expr_metta = expr_val.to_metta();

    // Get the MorkBridge and call dispatch_rules
    let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
    let rules = bridge.dispatch_rules(&expr_metta);
    let count = rules.len() as i64;

    // Free any previous rules
    if !ctx_ref.current_rules.is_null() {
        let _ = Box::from_raw(ctx_ref.current_rules as *mut Vec<CompiledRule>);
    }

    // Store rules in context for subsequent TryRule calls
    if count > 0 {
        let rules_box = Box::new(rules);
        ctx_ref.current_rules = Box::into_raw(rules_box) as *mut ();
    } else {
        ctx_ref.current_rules = std::ptr::null_mut();
    }
    ctx_ref.current_rule_idx = 0;

    box_long(count)
}

/// Try a single rule, pushing result or signaling failure
///
/// Stack: [expr] -> [result] or signal FAIL
///
/// # Arguments
/// * `ctx` - JIT context
/// * `rule_idx` - Index of rule in the match list (from previous DispatchRules)
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result value, or nil if rule doesn't match/doesn't exist
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_try_rule(
    ctx: *mut JitContext,
    rule_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        // No context - return nil as a valid "no match" result
        None => return JitValue::nil().to_bits(),
    };

    // Check if we have rules available
    if ctx_ref.current_rules.is_null() {
        // No rules dispatched yet - return nil
        return JitValue::nil().to_bits();
    }

    let rules = &*(ctx_ref.current_rules as *const Vec<CompiledRule>);
    let idx = rule_idx as usize;

    // Check bounds
    if idx >= rules.len() {
        // Rule index out of bounds - return nil
        return JitValue::nil().to_bits();
    }

    let rule = &rules[idx];

    // Install bindings from the pattern match into the JIT context
    // We need to push a new binding frame and populate it with the rule's bindings
    if ctx_ref.binding_frames_count < ctx_ref.binding_frames_cap && !ctx_ref.binding_frames.is_null() {
        // Push a new binding frame for this rule
        let frame_ptr = ctx_ref.binding_frames.add(ctx_ref.binding_frames_count);
        let frame = &mut *frame_ptr;

        // Count bindings
        let binding_count = rule.bindings.iter().count();
        if binding_count > 0 {
            // Allocate entries for this frame
            let layout = std::alloc::Layout::array::<super::types::JitBindingEntry>(binding_count)
                .expect("Layout calculation failed");
            frame.entries = std::alloc::alloc(layout) as *mut super::types::JitBindingEntry;
            frame.entries_cap = binding_count;
            frame.entries_count = 0;
            frame.scope_depth = ctx_ref.binding_frames_count as u32;

            // Install each binding
            for (name, value) in rule.bindings.iter() {
                // We need to store the variable name index - for now store as hash
                let name_idx = hash_string(name) as u32;
                let jit_value = metta_to_jit(value);

                let entry_ptr = frame.entries.add(frame.entries_count);
                *entry_ptr = super::types::JitBindingEntry::new(name_idx, jit_value);
                frame.entries_count += 1;
            }
        } else {
            frame.entries = std::ptr::null_mut();
            frame.entries_cap = 0;
            frame.entries_count = 0;
            frame.scope_depth = ctx_ref.binding_frames_count as u32;
        }

        ctx_ref.binding_frames_count += 1;
    }

    // Update current rule index
    ctx_ref.current_rule_idx = idx;

    // Signal bailout for the VM to execute the rule body
    // The rule body is in rule.body, which needs to be executed by the VM
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Call;

    // Return unit for now - actual result comes from rule body execution
    JitValue::unit().to_bits()
}

/// Simple hash function for binding names
#[inline]
fn hash_string(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Advance to next matching rule in choice point
///
/// Stack: [] -> [] (modifies internal state)
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 0 if advanced successfully, -1 if no more rules
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_next_rule(_ctx: *mut JitContext, _ip: u64) -> i64 {
    // Currently returns -1 (no more rules)
    // In full implementation, this would advance the rule index in the choice point
    -1
}

/// Commit to current rule (cut), removing alternative rules
///
/// Stack: [] -> []
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 0 on success
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_commit_rule(_ctx: *mut JitContext, _ip: u64) -> i64 {
    // Currently a no-op
    // In full implementation, this would remove alternative rules from choice points
    0
}

/// Signal explicit rule failure
///
/// Stack: [] -> [] (signals backtracking)
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// JIT_SIGNAL_FAIL to trigger backtracking
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fail_rule(_ctx: *mut JitContext, _ip: u64) -> i64 {
    JIT_SIGNAL_FAIL
}

/// Look up rules by head symbol
///
/// Stack: [] -> [count]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `head_idx` - Index of head symbol in constant pool
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Long - number of matching rules
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_lookup_rules(
    ctx: *mut JitContext,
    head_idx: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return box_long(0),
    };

    // Check if bridge is available
    if ctx_ref.bridge_ptr.is_null() {
        return box_long(0);
    }

    // Get head symbol from constants
    let head_metta = if head_idx < ctx_ref.constants_len as u64 {
        let constant_ptr = ctx_ref.constants.add(head_idx as usize);
        (*constant_ptr).clone()
    } else {
        return box_long(0);
    };

    // Dispatch rules using the head as expression
    let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
    let rules = bridge.dispatch_rules(&head_metta);
    let count = rules.len() as i64;

    // Free any previous rules
    if !ctx_ref.current_rules.is_null() {
        let _ = Box::from_raw(ctx_ref.current_rules as *mut Vec<CompiledRule>);
    }

    // Store rules in context
    if count > 0 {
        let rules_box = Box::new(rules);
        ctx_ref.current_rules = Box::into_raw(rules_box) as *mut ();
    } else {
        ctx_ref.current_rules = std::ptr::null_mut();
    }
    ctx_ref.current_rule_idx = 0;

    box_long(count)
}

/// Apply substitution to an expression
///
/// Stack: [expr, bindings] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed expression to substitute into
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result with variables substituted
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_apply_subst(
    ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    let expr_val = JitValue::from_raw(expr);
    let expr_metta = expr_val.to_metta();

    // Get bindings from context
    let bindings = collect_bindings_from_ctx(ctx);

    // Apply bindings to the expression
    let result = apply_bindings(&expr_metta, &bindings);

    // Convert result back to JitValue
    metta_to_jit(&result.into_owned()).to_bits()
}

/// Collect all bindings from the JIT context's binding frames
///
/// This iterates through all binding frames and collects their entries
/// into a Bindings struct for use with apply_bindings.
///
/// # Safety
/// The ctx pointer must be valid and point to a properly initialized JitContext.
unsafe fn collect_bindings_from_ctx(ctx: *mut JitContext) -> Bindings {
    let mut bindings = Bindings::new();

    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return bindings,
    };

    if ctx_ref.binding_frames.is_null() || ctx_ref.binding_frames_count == 0 {
        return bindings;
    }

    // Iterate through all binding frames (from innermost to outermost)
    // We collect from all frames to handle nested scopes
    for frame_idx in (0..ctx_ref.binding_frames_count).rev() {
        let frame = &*ctx_ref.binding_frames.add(frame_idx);

        if frame.entries.is_null() || frame.entries_count == 0 {
            continue;
        }

        // Collect entries from this frame
        for entry_idx in 0..frame.entries_count {
            let entry = &*frame.entries.add(entry_idx);

            // Convert JitValue back to MettaValue
            let value = entry.value.to_metta();

            // We need to recover the variable name from the hash
            // For now, we'll use the current_rules to find the original names
            // This is a workaround - ideally we'd store the actual names
            if let Some(name) = find_binding_name_by_hash(ctx, entry.name_idx as u64) {
                // Only insert if not already present (inner scope shadows outer)
                if bindings.get(&name).is_none() {
                    bindings.insert(name, value);
                }
            }
        }
    }

    bindings
}

/// Find binding name by hash from the current rules' bindings
///
/// This is a helper to recover variable names from their hashes.
/// It searches through the current rules' bindings to find matching names.
unsafe fn find_binding_name_by_hash(ctx: *mut JitContext, name_hash: u64) -> Option<String> {
    let ctx_ref = ctx.as_ref()?;

    if ctx_ref.current_rules.is_null() {
        return None;
    }

    let rules = &*(ctx_ref.current_rules as *const Vec<CompiledRule>);

    // Search through all rules' bindings for a matching name hash
    for rule in rules.iter() {
        // Use SmartBindings::iter() to get an iterator
        for (name, _value) in rule.bindings.iter() {
            if hash_string(name) as u32 == name_hash as u32 {
                return Some(name.clone());
            }
        }
    }

    None
}

/// Define a new rule in the environment
///
/// Stack: [pattern, body] -> [Unit]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `pattern_idx` - Index of pattern in constant pool
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit on success
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_define_rule(
    _ctx: *mut JitContext,
    _pattern_idx: u64,
    _ip: u64,
) -> u64 {
    // Currently a no-op returning Unit
    // In full implementation, this would add the rule to the environment
    JitValue::unit().to_bits()
}

// =============================================================================
// Phase E: Special Forms
// =============================================================================

/// Evaluate an if expression
///
/// Stack: [condition, then_branch, else_branch] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `condition` - NaN-boxed condition value
/// * `then_val` - NaN-boxed then branch value
/// * `else_val` - NaN-boxed else branch value
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result (then_val if condition is true, else_val otherwise)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_if(
    _ctx: *mut JitContext,
    condition: u64,
    then_val: u64,
    else_val: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::bytecode::jit::types::{TAG_BOOL, TAG_NIL};

    // True is TAG_BOOL | 1, False is TAG_BOOL | 0
    let tag_bool_true = TAG_BOOL | 1;
    let tag_bool_false = TAG_BOOL;

    // Check condition - True returns then_val, False/Nil returns else_val
    if condition == tag_bool_true {
        then_val
    } else if condition == tag_bool_false || condition == TAG_NIL {
        else_val
    } else {
        // Non-boolean truthy - return then branch
        then_val
    }
}

/// Evaluate a let expression (single binding)
///
/// Stack: [name_idx, value, body_chunk_ptr] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `name_idx` - Index of variable name in constant pool
/// * `value` - NaN-boxed value to bind
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit (binding stored in context)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_let(
    ctx: *mut JitContext,
    name_idx: u64,
    value: u64,
    _ip: u64,
) -> u64 {
    // Store binding in context
    jit_runtime_store_binding(ctx, name_idx, value, _ip);
    JitValue::unit().to_bits()
}

/// Evaluate a let* expression (sequential bindings)
///
/// Stack: [] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit (bindings are handled sequentially by the compiler)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_let_star(
    _ctx: *mut JitContext,
    _ip: u64,
) -> u64 {
    // Let* bindings are handled sequentially by the bytecode compiler
    // This runtime function is mainly a marker/placeholder
    JitValue::unit().to_bits()
}

/// Evaluate a match expression
///
/// Stack: [value, pattern] -> [bool]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `value` - NaN-boxed value to match
/// * `pattern` - NaN-boxed pattern
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Bool indicating match success
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_match(
    ctx: *mut JitContext,
    value: u64,
    pattern: u64,
    _ip: u64,
) -> u64 {
    // Delegate to pattern match runtime
    jit_runtime_pattern_match(ctx, pattern, value, _ip)
}

/// Evaluate a case expression (pattern-based switch)
///
/// Stack: [value] -> [case_index]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `value` - NaN-boxed value to switch on
/// * `case_count` - Number of cases
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Long - index of matching case (0 = first case, -1 = no match)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_case(
    ctx: *mut JitContext,
    value: u64,
    case_count: u64,
    ip: u64,
) -> u64 {
    // Case dispatch using pattern matching
    // This is called when the bytecode uses EvalCase opcode
    // Patterns are expected to be at consecutive indices starting at ip-based offset
    //
    // For each case i (0..case_count):
    //   - pattern is at constant[base_idx + i*2]
    //   - body is at constant[base_idx + i*2 + 1]
    //
    // Returns the index of the matching case, or -1 if no match

    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return box_long(-1),
    };

    let value_jit = JitValue::from_raw(value);
    let value_metta = value_jit.to_metta();

    // Calculate base index for patterns (this is a simplified approach)
    // In practice, the pattern indices would be encoded in the bytecode
    let base_idx = ip as usize;

    // Try each pattern
    for i in 0..(case_count as usize) {
        let pattern_idx = base_idx + i * 2;

        if pattern_idx < ctx_ref.constants_len {
            let pattern = &*ctx_ref.constants.add(pattern_idx);

            if let Some(bindings) = pattern_match(pattern, &value_metta) {
                // Match found - install bindings if we have binding frames
                if !ctx_ref.binding_frames.is_null() && ctx_ref.binding_frames_count > 0 {
                    // Get current frame
                    let frame_idx = ctx_ref.binding_frames_count - 1;
                    let frame = &mut *ctx_ref.binding_frames.add(frame_idx);

                    // Install bindings
                    let binding_count = bindings.iter().count();
                    if binding_count > 0 && frame.entries.is_null() {
                        let layout = std::alloc::Layout::array::<super::types::JitBindingEntry>(binding_count)
                            .expect("Layout calculation failed");
                        frame.entries = std::alloc::alloc(layout) as *mut super::types::JitBindingEntry;
                        frame.entries_cap = binding_count;
                        frame.entries_count = 0;
                    }

                    for (name, val) in bindings.iter() {
                        if frame.entries_count < frame.entries_cap {
                            let name_idx = hash_string(name) as u32;
                            let jit_value = metta_to_jit(val);
                            let entry_ptr = frame.entries.add(frame.entries_count);
                            *entry_ptr = super::types::JitBindingEntry::new(name_idx, jit_value);
                            frame.entries_count += 1;
                        }
                    }
                }

                return box_long(i as i64);
            }
        }
    }

    // No match found
    box_long(-1)
}

/// Evaluate a chain expression (sequential evaluation)
///
/// Stack: [expr1, expr2] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `first` - First expression result (ignored except for side effects)
/// * `second` - Second expression (returned)
/// * `_ip` - Instruction pointer
///
/// # Returns
/// The second expression result
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_chain(
    _ctx: *mut JitContext,
    _first: u64,
    second: u64,
    _ip: u64,
) -> u64 {
    // Chain evaluates first for side effects, returns second
    second
}

/// Evaluate a quote expression (prevent evaluation)
///
/// Stack: [expr] -> [quoted_expr]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed expression to quote
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed quoted expression
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_quote(
    ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    // Wrap expression in a quote - delegates to make_quote
    jit_runtime_make_quote(ctx, expr, _ip)
}

/// Evaluate an unquote expression (force evaluation of quoted)
///
/// Stack: [quoted_expr] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed quoted expression
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result of evaluating the unquoted expression
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_unquote(
    _ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;

    let expr_val = JitValue::from_raw(expr);
    let metta = expr_val.to_metta();

    // If it's a quote, unwrap it; otherwise return as-is
    match metta {
        MettaValue::SExpr(elems) if !elems.is_empty() => {
            if let MettaValue::Atom(ref s) = elems[0] {
                if s == "quote" && elems.len() == 2 {
                    // Return the quoted content
                    return metta_to_jit(&elems[1]).to_bits();
                }
            }
        }
        _ => {}
    }

    // Not a quote, return as-is
    expr
}

/// Evaluate an eval expression (force evaluation)
///
/// Stack: [expr] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed expression to evaluate
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result of evaluation
/// Note: In full implementation, this would trigger rule dispatch
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_eval(
    _ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    // In a full implementation, this would call the evaluator
    // For now, return expression unchanged (evaluation happens at bytecode level)
    expr
}

/// Evaluate a bind expression (create binding in space)
///
/// Stack: [name, value] -> [Unit]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `name_idx` - Name index in constant pool
/// * `value` - NaN-boxed value to bind
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_bind(
    ctx: *mut JitContext,
    name_idx: u64,
    value: u64,
    _ip: u64,
) -> u64 {
    // Store binding (same as eval_let)
    jit_runtime_store_binding(ctx, name_idx, value, _ip);
    JitValue::unit().to_bits()
}

/// Evaluate a new expression (create new space)
///
/// Stack: [] -> [space]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed space handle
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_new(
    ctx: *mut JitContext,
    _ip: u64,
) -> u64 {
    use crate::backend::bytecode::space_registry::SpaceRegistry;
    use crate::backend::models::{MettaValue, SpaceHandle};
    use std::sync::atomic::{AtomicU64, Ordering};

    // Global counter for unique anonymous space IDs
    static ANON_SPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    // Generate unique ID and name
    let space_id = ANON_SPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let space_name = format!("anon-space-{}", space_id);
    let space = SpaceHandle::new(space_id, space_name.clone());

    // Optionally register in space registry if available
    if let Some(ctx_ref) = ctx.as_ref() {
        if !ctx_ref.space_registry.is_null() {
            let registry = &*(ctx_ref.space_registry as *const SpaceRegistry);
            registry.register(&space_name, space.clone());
        }
    }

    metta_to_jit(&MettaValue::Space(space)).to_bits()
}

/// Evaluate a collapse expression (determinize nondeterministic results)
///
/// Stack: [expr] -> [list]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed expression with nondeterministic results
/// * `ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed list of all results
///
/// Note: Full nondeterministic collapse requires the dispatcher loop.
/// This implementation collects results from the context's result buffer
/// when in nondeterminism mode, or wraps a single value in a list.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_collapse(
    ctx: *mut JitContext,
    expr: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => {
            // No context - wrap single value
            let expr_val = JitValue::from_raw(expr);
            let metta = expr_val.to_metta();
            return metta_to_jit(&MettaValue::SExpr(vec![metta])).to_bits();
        }
    };

    // Check if we have collected results from nondeterminism
    if ctx_ref.results_count > 0 && !ctx_ref.results.is_null() {
        // Collect all results into a list
        let mut results = Vec::with_capacity(ctx_ref.results_count);

        for i in 0..ctx_ref.results_count {
            let result_val = &*ctx_ref.results.add(i);
            results.push(result_val.to_metta());
        }

        // Clear results buffer
        ctx_ref.results_count = 0;

        return metta_to_jit(&MettaValue::SExpr(results)).to_bits();
    }

    // No nondeterminism results available
    // If there are active choice points, we need to trigger full exploration
    if ctx_ref.choice_point_count > 0 {
        // Signal bailout for full nondeterminism exploration
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;

        // Return expr unchanged - VM will handle collapse
        return expr;
    }

    // No nondeterminism - wrap single value in list
    let expr_val = JitValue::from_raw(expr);
    let metta = expr_val.to_metta();

    // If already a list, return as-is (could be result of superpose)
    match metta {
        MettaValue::SExpr(_) => expr,
        _ => metta_to_jit(&MettaValue::SExpr(vec![metta])).to_bits(),
    }
}

/// Evaluate a superpose expression (create nondeterministic choice)
///
/// Stack: [list] -> [choice]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `list` - NaN-boxed list of alternatives
/// * `ip` - Instruction pointer (used for resume point)
///
/// # Returns
/// NaN-boxed first alternative (creates choice point for remaining alternatives)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_superpose(
    ctx: *mut JitContext,
    list: u64,
    ip: u64,
) -> u64 {
    let list_val = JitValue::from_raw(list);
    let metta = list_val.to_metta();

    match metta {
        MettaValue::SExpr(elems) if !elems.is_empty() => {
            let ctx_ref = match ctx.as_mut() {
                Some(c) => c,
                None => return metta_to_jit(&elems[0]).to_bits(),
            };

            // If more than one element, create choice point for alternatives
            if elems.len() > 1 && ctx_ref.choice_point_cap > 0 {
                let alt_count = elems.len() - 1;

                // Optimization 5.2: Check if alternatives fit inline
                if alt_count > super::MAX_ALTERNATIVES_INLINE {
                    // Too many alternatives - fall back to returning just the first
                    return metta_to_jit(&elems[0]).to_bits();
                }

                // Check if we have capacity
                if ctx_ref.choice_point_count < ctx_ref.choice_point_cap {
                    let cp_ptr = ctx_ref.choice_points.add(ctx_ref.choice_point_count);
                    let cp = &mut *cp_ptr;

                    // Save current stack pointer
                    cp.saved_sp = ctx_ref.sp as u64;
                    cp.saved_ip = ip;
                    cp.saved_chunk = ctx_ref.current_chunk;
                    cp.saved_stack_pool_idx = -1; // No stack save for superpose
                    cp.saved_stack_count = 0;
                    cp.alt_count = alt_count as u64;
                    cp.current_index = 0;

                    // Optimization 5.2: Store alternatives inline
                    for (i, elem) in elems.iter().skip(1).enumerate() {
                        cp.alternatives_inline[i] = JitAlternative::value(metta_to_jit(elem));
                    }

                    ctx_ref.choice_point_count += 1;
                    ctx_ref.in_nondet_mode = true;
                }
            }

            // Return first element
            metta_to_jit(&elems[0]).to_bits()
        }
        MettaValue::SExpr(elems) if elems.is_empty() => {
            // Empty superpose - signal failure
            JIT_SIGNAL_FAIL as u64
        }
        _ => {
            // Not a list - return as-is (single value)
            list
        }
    }
}

/// Evaluate a memo expression (memoized evaluation)
///
/// Stack: [expr] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed expression to memoize
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result (cached if previously evaluated)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_memo(
    _ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    // In full implementation, this would check a memo cache
    // For now, return expression unchanged (no caching)
    expr
}

/// Evaluate a memo-first expression (memoize only first result)
///
/// Stack: [expr] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed expression
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed first result (cached)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_memo_first(
    _ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    // In full implementation, this would memoize only the first result
    // For now, return expression unchanged
    expr
}

/// Evaluate a pragma expression (compiler directive)
///
/// Stack: [directive] -> [Unit]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_directive` - NaN-boxed pragma directive
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_pragma(
    _ctx: *mut JitContext,
    _directive: u64,
    _ip: u64,
) -> u64 {
    // Pragmas are compile-time directives, no runtime effect
    JitValue::unit().to_bits()
}

/// Evaluate a function definition
///
/// Stack: [name, params, body] -> [Unit]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `name_idx` - Function name index
/// * `param_count` - Number of parameters
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_function(
    _ctx: *mut JitContext,
    _name_idx: u64,
    _param_count: u64,
    _ip: u64,
) -> u64 {
    // Function definitions are stored in the environment
    // This is a placeholder - actual definition happens via DefineRule
    JitValue::unit().to_bits()
}

/// Evaluate a lambda expression (create closure)
///
/// Creates a closure that captures the current binding environment.
/// The closure can later be applied to arguments via `jit_runtime_eval_apply`.
///
/// Stack: [params, body] -> [closure]
///
/// # Arguments
/// * `ctx` - JIT context (provides current binding frames to capture)
/// * `param_count` - Number of parameters the lambda expects
/// * `ip` - Instruction pointer (for debuggin/error context)
///
/// # Returns
/// NaN-boxed closure represented as a heap-allocated MettaValue::SExpr.
/// The closure is encoded as: `(lambda param_count (captured_env...))`
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_lambda(
    ctx: *mut JitContext,
    param_count: u64,
    ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;

    if ctx.is_null() {
        // No context - create minimal closure representation
        let closure = MettaValue::SExpr(vec![
            MettaValue::Atom("lambda".to_string()),
            MettaValue::Long(param_count as i64),
            MettaValue::SExpr(vec![]), // Empty captured env
        ]);
        return metta_to_jit(&closure).to_bits();
    }

    let ctx_ref = &*ctx;

    // Capture the current binding environment
    // We create a copy of all current bindings to be restored when closure is applied
    let captured_bindings = collect_bindings_from_ctx(ctx);

    // Create captured environment representation
    // Variables are represented as Atom with $ prefix
    let mut captured_env: Vec<MettaValue> = Vec::with_capacity(captured_bindings.len());
    for (name, value) in captured_bindings.iter() {
        // Variables use $prefix in MeTTa
        let var_name = if name.starts_with('$') {
            name.to_string()
        } else {
            format!("${}", name)
        };
        captured_env.push(MettaValue::SExpr(vec![
            MettaValue::Atom(var_name),
            value.clone(),
        ]));
    }

    // Create closure representation as S-expression:
    // (lambda param_count (captured_bindings...) ip)
    let closure = MettaValue::SExpr(vec![
        MettaValue::Atom("lambda".to_string()),
        MettaValue::Long(param_count as i64),
        MettaValue::SExpr(captured_env),
        MettaValue::Long(ip as i64), // Store IP for body reference
    ]);

    metta_to_jit(&closure).to_bits()
}

/// Evaluate an apply expression (apply closure to arguments)
///
/// Applies a closure to arguments by:
/// 1. Extracting the closure's captured environment
/// 2. Installing the captured bindings into the JIT context
/// 3. Binding arguments to parameters
/// 4. Triggering bailout for the bytecode VM to execute the closure body
///
/// Stack: [closure, args...] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `closure` - NaN-boxed closure (MettaValue::SExpr)
/// * `arg_count` - Number of arguments being passed
/// * `ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result of application. For full closure evaluation,
/// triggers bailout so the bytecode VM can execute the body.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_apply(
    ctx: *mut JitContext,
    closure: u64,
    arg_count: u64,
    ip: u64,
) -> u64 {
    use crate::backend::models::MettaValue;

    if ctx.is_null() {
        return closure;
    }

    let ctx_ref = &mut *ctx;
    let closure_val = JitValue::from_raw(closure).to_metta();

    // Extract closure components: (lambda param_count (captured_env...) body_ip)
    if let MettaValue::SExpr(ref items) = closure_val {
        if items.len() >= 3 {
            let is_lambda = matches!(&items[0], MettaValue::Atom(s) if s == "lambda");
            if !is_lambda {
                // Not a lambda - return unchanged
                return closure;
            }

            // Get parameter count from closure
            let param_count = match &items[1] {
                MettaValue::Long(n) => *n as u64,
                _ => 0,
            };

            // Check arity
            if arg_count != param_count {
                // Arity mismatch - return error or closure unchanged
                let error = MettaValue::Error(
                    format!(
                        "Lambda arity mismatch: expected {} arguments, got {}",
                        param_count, arg_count
                    ),
                    std::sync::Arc::new(closure_val),
                );
                return metta_to_jit(&error).to_bits();
            }

            // Install captured environment bindings
            if let MettaValue::SExpr(ref captured_env) = items[2] {
                // Push a new binding frame for the closure scope
                jit_runtime_push_binding_frame(ctx);

                // Install each captured binding
                // Variables in MeTTa are Atoms that start with $
                for captured in captured_env {
                    if let MettaValue::SExpr(ref binding) = captured {
                        if binding.len() >= 2 {
                            if let MettaValue::Atom(ref name) = binding[0] {
                                // Variables start with $ - strip it for binding name
                                let binding_name = if name.starts_with('$') {
                                    &name[1..]
                                } else {
                                    name.as_str()
                                };
                                let name_hash = hash_string(binding_name);
                                let value_bits = metta_to_jit(&binding[1]).to_bits();
                                jit_runtime_store_binding(ctx, name_hash, value_bits, ip);
                            }
                        }
                    }
                }
            }

            // Trigger bailout for the bytecode VM to execute the closure body
            // The VM will handle argument binding and body evaluation
            ctx_ref.bailout = true;
            ctx_ref.bailout_reason = JitBailoutReason::Call;
            return JitValue::unit().to_bits();
        }
    }

    // Fallback: return closure unchanged
    closure
}

// =============================================================================
// Phase F: Advanced Calls
// =============================================================================

/// Call a native Rust function by ID
///
/// Stack: [args...] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `func_id` - Native function ID (from NativeRegistry)
/// * `arg_count` - Number of arguments
/// * `ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result from the native function
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_call_native(
    ctx: *mut JitContext,
    func_id: u64,
    arg_count: u64,
    ip: u64,
) -> u64 {
    use crate::backend::bytecode::native_registry::{NativeContext, NativeRegistry};
    use crate::backend::models::MettaValue;
    use crate::backend::Environment;

    if ctx.is_null() {
        return JitValue::nil().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Get arguments from stack
    let mut args = Vec::with_capacity(arg_count as usize);
    for _i in 0..arg_count as usize {
        if ctx_ref.sp < 1 {
            eprintln!("JIT runtime: Stack underflow in call_native at IP {}", ip);
            return JitValue::nil().to_bits();
        }
        ctx_ref.sp -= 1;
        let val = *ctx_ref.value_stack.add(ctx_ref.sp);
        args.push(JitValue::from_raw(val.to_bits()).to_metta());
    }
    args.reverse(); // Restore argument order

    // Create native context
    let native_ctx = NativeContext::new(Environment::new());

    // In a full implementation, we would get the registry from the context
    // For now, use a default registry with stdlib functions
    let registry = NativeRegistry::with_stdlib();

    // Call the native function
    match registry.call(func_id as u16, &args, &native_ctx) {
        Ok(results) => {
            if results.len() == 1 {
                metta_to_jit(&results[0]).to_bits()
            } else if results.is_empty() {
                JitValue::unit().to_bits()
            } else {
                metta_to_jit(&MettaValue::SExpr(results)).to_bits()
            }
        }
        Err(e) => {
            eprintln!("JIT runtime: Native call error at IP {}: {}", ip, e);
            JitValue::nil().to_bits()
        }
    }
}

/// Call an external function by name index
///
/// Stack: [args...] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `name_idx` - Index into constant pool for function name
/// * `arg_count` - Number of arguments
/// * `ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result from the external function
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_call_external(
    ctx: *mut JitContext,
    name_idx: u64,
    arg_count: u64,
    ip: u64,
) -> u64 {
    if ctx.is_null() {
        return JitValue::nil().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Check if external registry is available
    if ctx_ref.external_registry.is_null() {
        // No external registry - pop args and return Unit
        for _ in 0..arg_count {
            if ctx_ref.sp > 0 {
                ctx_ref.sp -= 1;
            }
        }
        return JitValue::unit().to_bits();
    }

    // Get function name from constant pool
    let name_index = name_idx as usize;
    if name_index >= ctx_ref.constants_len {
        // Invalid constant index - pop args and return error
        for _ in 0..arg_count {
            if ctx_ref.sp > 0 {
                ctx_ref.sp -= 1;
            }
        }
        return JitValue::nil().to_bits();
    }

    let name_constant = &*ctx_ref.constants.add(name_index);
    let func_name = match name_constant {
        MettaValue::Atom(s) => s.as_str(),
        MettaValue::String(s) => s.as_str(),
        _ => {
            // Name must be an atom or string
            for _ in 0..arg_count {
                if ctx_ref.sp > 0 {
                    ctx_ref.sp -= 1;
                }
            }
            return JitValue::nil().to_bits();
        }
    };

    // Collect arguments from stack (in reverse order since they're pushed first-to-last)
    let arg_count_usize = arg_count as usize;
    let mut args: Vec<MettaValue> = Vec::with_capacity(arg_count_usize);

    // Pop arguments from stack in reverse order
    if ctx_ref.sp < arg_count_usize {
        eprintln!("JIT runtime: Stack underflow in call_external at IP {}", ip);
        return JitValue::nil().to_bits();
    }

    // Read arguments in correct order (oldest first)
    let stack_base = ctx_ref.sp - arg_count_usize;
    for i in 0..arg_count_usize {
        let jit_val = *ctx_ref.value_stack.add(stack_base + i);
        args.push(jit_val.to_metta());
    }
    ctx_ref.sp = stack_base; // Pop all args at once

    // Get the external registry
    let registry = &*(ctx_ref.external_registry as *const ExternalRegistry);

    // Create external context with default environment
    let ext_ctx = ExternalContext::default();

    // Call the external function
    match registry.call(func_name, &args, &ext_ctx) {
        Ok(results) => {
            // Return first result (or Unit if empty)
            if results.is_empty() {
                JitValue::unit().to_bits()
            } else {
                match JitValue::try_from_metta(&results[0]) {
                    Some(jv) => jv.to_bits(),
                    None => {
                        // Can't NaN-box the result - allocate on heap
                        let boxed = Box::new(results[0].clone());
                        JitValue::from_heap_ptr(Box::into_raw(boxed)).to_bits()
                    }
                }
            }
        }
        Err(e) => {
            // External call failed - return error
            eprintln!("JIT runtime: External call '{}' failed: {}", func_name, e);
            let error = MettaValue::Error(
                format!("external-call-failed: {}", e),
                Arc::new(MettaValue::Atom(func_name.to_string())),
            );
            let boxed = Box::new(error);
            JitValue::from_heap_ptr(Box::into_raw(boxed)).to_bits()
        }
    }
}

/// Call a function with memoization (cached results)
///
/// Stack: [args...] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `head_idx` - Index into constant pool for function head
/// * `arg_count` - Number of arguments
/// * `ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result (cached if available)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_call_cached(
    ctx: *mut JitContext,
    head_idx: u64,
    arg_count: u64,
    ip: u64,
) -> u64 {
    use crate::backend::bytecode::memo_cache::MemoCache;

    if ctx.is_null() {
        return JitValue::nil().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Get function head from constant pool
    let head_index = head_idx as usize;
    if head_index >= ctx_ref.constants_len {
        // Invalid constant index - pop args and return nil
        for _ in 0..arg_count {
            if ctx_ref.sp > 0 {
                ctx_ref.sp -= 1;
            }
        }
        return JitValue::nil().to_bits();
    }

    let head_constant = &*ctx_ref.constants.add(head_index);
    let func_head = match head_constant {
        MettaValue::Atom(s) => s.clone(),
        _ => {
            // Head must be an atom
            for _ in 0..arg_count {
                if ctx_ref.sp > 0 {
                    ctx_ref.sp -= 1;
                }
            }
            return JitValue::nil().to_bits();
        }
    };

    // Collect arguments from stack (in correct order)
    let arg_count_usize = arg_count as usize;
    if ctx_ref.sp < arg_count_usize {
        eprintln!("JIT runtime: Stack underflow in call_cached at IP {}", ip);
        return JitValue::nil().to_bits();
    }

    let stack_base = ctx_ref.sp - arg_count_usize;
    let mut args: Vec<MettaValue> = Vec::with_capacity(arg_count_usize);
    for i in 0..arg_count_usize {
        let jit_val = *ctx_ref.value_stack.add(stack_base + i);
        args.push(jit_val.to_metta());
    }
    ctx_ref.sp = stack_base; // Pop all args at once

    // Check if memo cache is available
    if !ctx_ref.memo_cache.is_null() {
        let cache = &*(ctx_ref.memo_cache as *const MemoCache);

        // Check cache for existing result
        if let Some(cached_result) = cache.get(&func_head, &args) {
            // Cache hit - return cached result
            match JitValue::try_from_metta(&cached_result) {
                Some(jv) => return jv.to_bits(),
                None => {
                    let boxed = Box::new(cached_result);
                    return JitValue::from_heap_ptr(Box::into_raw(boxed)).to_bits();
                }
            }
        }
    }

    // Cache miss - need to compute the result
    // Try to dispatch via MorkBridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);

        // Build the call expression
        let mut call_expr_parts = vec![MettaValue::Atom(func_head.clone())];
        call_expr_parts.extend(args.clone());
        let call_expr = MettaValue::SExpr(call_expr_parts.clone());

        // Dispatch rules
        let matches = bridge.dispatch_rules(&call_expr);

        if matches.is_empty() {
            // No matching rules - return the call expression as irreducible
            let expr = MettaValue::SExpr(call_expr_parts);
            let boxed = Box::new(expr);
            return JitValue::from_heap_ptr(Box::into_raw(boxed)).to_bits();
        }

        // Execute the first matching rule
        // For cached calls, we only use the first result (no nondeterminism)
        let rule = &matches[0];
        let mut vm = BytecodeVM::new(Arc::clone(&rule.body));

        // Apply bindings by pushing initial values
        for (_name, value) in rule.bindings.iter() {
            vm.push_initial_value(value.clone());
        }

        // Execute and get result
        match vm.run() {
            Ok(results) => {
                let result = results.into_iter().next().unwrap_or(MettaValue::Unit);

                // Cache the result if memo cache is available
                if !ctx_ref.memo_cache.is_null() {
                    let cache = &*(ctx_ref.memo_cache as *const MemoCache);
                    cache.insert(&func_head, &args, result.clone());
                }

                // Return the result
                match JitValue::try_from_metta(&result) {
                    Some(jv) => return jv.to_bits(),
                    None => {
                        let boxed = Box::new(result);
                        return JitValue::from_heap_ptr(Box::into_raw(boxed)).to_bits();
                    }
                }
            }
            Err(_) => {
                // VM execution failed - return expression as irreducible
                let expr = MettaValue::SExpr(call_expr_parts);
                let boxed = Box::new(expr);
                return JitValue::from_heap_ptr(Box::into_raw(boxed)).to_bits();
            }
        }
    }

    // No bridge - return the call expression as irreducible
    let mut expr_parts = vec![MettaValue::Atom(func_head)];
    expr_parts.extend(args);
    let expr = MettaValue::SExpr(expr_parts);
    let boxed = Box::new(expr);
    JitValue::from_heap_ptr(Box::into_raw(boxed)).to_bits()
}

// =============================================================================
// Phase G: Advanced Nondeterminism
// =============================================================================

/// Cut operation - prune search space by removing choice points back to the current cut scope
///
/// Prolog-style cut: removes choice points created since the current scope was entered,
/// but preserves choice points from outer scopes.
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cut(ctx: *mut JitContext, _ip: u64) -> u64 {
    if ctx.is_null() {
        return JitValue::unit().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Check if we have cut markers available
    if ctx_ref.cut_marker_count > 0 && !ctx_ref.cut_markers.is_null() {
        // Get the most recent cut marker (choice point count at scope entry)
        let marker = *ctx_ref.cut_markers.add(ctx_ref.cut_marker_count - 1);

        // Remove choice points back to the marker, but not beyond it
        // This preserves choice points from outer scopes
        if ctx_ref.choice_point_count > marker {
            ctx_ref.choice_point_count = marker;
        }
    } else {
        // No cut markers - fall back to clearing all choice points
        // This happens when cut is called outside of a proper cut scope
        ctx_ref.choice_point_count = 0;
    }

    JitValue::unit().to_bits()
}

/// Enter a cut scope - push a cut marker
///
/// Call this when entering a scope that may use cut (e.g., rule body, once/1, etc.)
/// The marker records the current choice point count so cut knows where to stop.
///
/// # Arguments
/// * `ctx` - JIT context
///
/// # Returns
/// 1 on success, 0 on failure (no room for marker)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_enter_cut_scope(ctx: *mut JitContext) -> i64 {
    if ctx.is_null() {
        return 0;
    }
    let ctx_ref = &mut *ctx;

    // Check if we have room for another marker
    if ctx_ref.cut_markers.is_null() || ctx_ref.cut_marker_count >= ctx_ref.cut_marker_cap {
        return 0; // No room
    }

    // Push the current choice point count as a marker
    *ctx_ref.cut_markers.add(ctx_ref.cut_marker_count) = ctx_ref.choice_point_count;
    ctx_ref.cut_marker_count += 1;

    1 // Success
}

/// Exit a cut scope - pop a cut marker
///
/// Call this when leaving a scope that may have used cut.
///
/// # Arguments
/// * `ctx` - JIT context
///
/// # Returns
/// 1 on success, 0 on failure (no markers to pop)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_exit_cut_scope(ctx: *mut JitContext) -> i64 {
    if ctx.is_null() {
        return 0;
    }
    let ctx_ref = &mut *ctx;

    // Check if we have markers to pop
    if ctx_ref.cut_marker_count == 0 {
        return 0; // No markers
    }

    ctx_ref.cut_marker_count -= 1;
    1 // Success
}

/// Guard condition - check if condition allows continuation
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `condition` - NaN-boxed boolean condition
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 1 if guard passes (proceed), 0 if guard fails (backtrack)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_guard(
    _ctx: *mut JitContext,
    condition: u64,
    _ip: u64,
) -> i64 {
    use crate::backend::bytecode::jit::types::TAG_BOOL;

    // True is TAG_BOOL | 1
    let tag_bool_true = TAG_BOOL | 1;

    if condition == tag_bool_true {
        1 // Guard passes, continue
    } else {
        0 // Guard fails, backtrack
    }
}

/// Amb operation - inline nondeterministic choice
///
/// Stack: [alt1, alt2, ..., altN] -> [selected]
/// Creates choice point for alternatives 2..N, returns first alternative
///
/// # Arguments
/// * `ctx` - JIT context
/// * `alt_count` - Number of alternatives on stack
/// * `ip` - Instruction pointer for backtracking
///
/// # Returns
/// NaN-boxed first alternative value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_amb(
    ctx: *mut JitContext,
    alt_count: u64,
    ip: u64,
) -> u64 {
    if ctx.is_null() {
        return JitValue::nil().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Empty amb - fail immediately
    if alt_count == 0 {
        return JitValue::nil().to_bits();
    }

    // Pop all alternatives from stack
    let mut alts = Vec::with_capacity(alt_count as usize);
    for _ in 0..alt_count {
        if ctx_ref.sp < 1 {
            eprintln!("JIT runtime: Stack underflow in amb at IP {}", ip);
            return JitValue::nil().to_bits();
        }
        ctx_ref.sp -= 1;
        let val = *ctx_ref.value_stack.add(ctx_ref.sp);
        alts.push(val);
    }
    alts.reverse(); // Restore order: alt1, alt2, ..., altN

    // Single alternative - no choice point needed
    if alt_count == 1 {
        return alts[0].to_bits();
    }

    // Create choice point for remaining alternatives (2..N)
    if ctx_ref.choice_point_count < ctx_ref.choice_point_cap {
        let cp = &mut *ctx_ref.choice_points.add(ctx_ref.choice_point_count);
        cp.saved_sp = ctx_ref.sp as u64;
        cp.alt_count = (alt_count - 1) as u64;
        cp.current_index = 0;
        cp.saved_ip = ip;

        // Store remaining alternatives for backtracking
        // Note: In a full implementation, we'd store all alts[1..] in the choice point
        // For now, we just store the count

        ctx_ref.choice_point_count += 1;
    }

    // Return first alternative
    alts[0].to_bits()
}

/// Commit operation - remove N choice points (soft cut)
///
/// # Arguments
/// * `ctx` - JIT context
/// * `count` - Number of choice points to remove (0 = all)
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_commit(
    ctx: *mut JitContext,
    count: u64,
    _ip: u64,
) -> u64 {
    if ctx.is_null() {
        return JitValue::unit().to_bits();
    }
    let ctx_ref = &mut *ctx;

    if count == 0 {
        // Remove all choice points (full cut)
        ctx_ref.choice_point_count = 0;
    } else {
        // Remove N most recent choice points
        let to_remove = (count as usize).min(ctx_ref.choice_point_count);
        ctx_ref.choice_point_count -= to_remove;
    }

    JitValue::unit().to_bits()
}

/// Backtrack operation - force immediate backtracking
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// JIT signal for backtracking (-3 = FAIL signal)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_backtrack(
    _ctx: *mut JitContext,
    _ip: u64,
) -> i64 {
    // Return FAIL signal to trigger backtracking in the JIT dispatcher
    super::JIT_SIGNAL_FAIL
}

// =============================================================================
// Phase 1.1: Core Nondeterminism Markers
// =============================================================================

/// Begin nondeterministic section - increment fork depth counter
///
/// This marks the start of a code section that may contain nondeterministic
/// operations (Fork, Amb, etc.). The fork_depth counter helps track nesting
/// and can be used for optimization decisions.
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_begin_nondet(ctx: *mut JitContext, _ip: u64) {
    let ctx_ref = &mut *ctx;
    ctx_ref.fork_depth += 1;
}

/// End nondeterministic section - decrement fork depth counter
///
/// This marks the end of a code section that may contain nondeterministic
/// operations. Decrements the fork_depth counter (will not go below 0).
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_end_nondet(ctx: *mut JitContext, _ip: u64) {
    let ctx_ref = &mut *ctx;
    if ctx_ref.fork_depth > 0 {
        ctx_ref.fork_depth -= 1;
    }
}

// =============================================================================
// Phase H: MORK Bridge
// =============================================================================

/// Look up a value in MORK PathMap
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `path` - NaN-boxed path expression
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed value from MORK, or Nil if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_lookup(
    ctx: *mut JitContext,
    path: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return JitValue::nil().to_bits(),
    };

    // Convert path to MettaValue
    let path_jit = JitValue::from_raw(path);
    let path_metta = path_jit.to_metta();

    // Try to look up through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.read();
        if let Ok(env_read) = env_guard {
            // Check if this is an atom and it exists in the space
            if let MettaValue::Atom(name) = &path_metta {
                if env_read.has_fact(name) {
                    return metta_to_jit(&path_metta).to_bits();
                }
            }

            // Check if this is an S-expression that exists in space
            if env_read.has_sexpr_fact(&path_metta) {
                return metta_to_jit(&path_metta).to_bits();
            }
        }
    }

    // Not found
    JitValue::nil().to_bits()
}

/// Match a pattern against MORK space
///
/// # Arguments
/// * `ctx` - JIT context
/// * `pattern` - NaN-boxed pattern to match
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result (may create choice points for multiple matches)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_match(
    ctx: *mut JitContext,
    pattern: u64,
    _ip: u64,
) -> u64 {
    // Delegate to space match (MORK uses similar semantics)
    jit_runtime_space_match(ctx, 0, pattern, 0, _ip)
}

/// Insert a value into MORK PathMap / space
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_path` - NaN-boxed path expression (unused for simple add)
/// * `value` - NaN-boxed value to insert
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 0 on success, -1 on error
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_insert(
    ctx: *mut JitContext,
    _path: u64,
    value: u64,
    _ip: u64,
) -> i64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return -1,
    };

    // Convert value to MettaValue
    let value_jit = JitValue::from_raw(value);
    let value_metta = value_jit.to_metta();

    // Try to insert through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.write();
        if let Ok(mut env_write) = env_guard {
            env_write.add_to_space(&value_metta);
            return 0; // Success
        }
    }

    -1 // Error (no bridge available)
}

/// Delete a value from MORK PathMap / space
///
/// # Arguments
/// * `ctx` - JIT context
/// * `path` - NaN-boxed path/value expression to delete
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 1 if deleted, 0 if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_delete(
    ctx: *mut JitContext,
    path: u64,
    _ip: u64,
) -> i64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return 0,
    };

    // Convert path to MettaValue
    let path_jit = JitValue::from_raw(path);
    let path_metta = path_jit.to_metta();

    // Try to delete through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.write();
        if let Ok(mut env_write) = env_guard {
            env_write.remove_from_space(&path_metta);
            return 1; // Deleted (or at least attempted)
        }
    }

    0 // Not found (no bridge available)
}

// =============================================================================
// Phase I: Debug/Meta
// =============================================================================

/// Emit a trace event for debugging
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `msg_idx` - Index of message in constant pool
/// * `value` - NaN-boxed value to trace
/// * `ip` - Instruction pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_trace(
    _ctx: *mut JitContext,
    msg_idx: u64,
    value: u64,
    ip: u64,
) {
    // Convert value to string for tracing
    let jit_val = JitValue::from_raw(value);
    let metta_val = jit_val.to_metta();

    // Simple trace output to stderr
    eprintln!("[JIT TRACE] ip={} msg_idx={} value={:?}", ip, msg_idx, metta_val);
}

/// Breakpoint for debugging
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `bp_id` - Breakpoint identifier
/// * `ip` - Instruction pointer
///
/// # Returns
/// -1 to pause execution, 0 to continue
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_breakpoint(
    _ctx: *mut JitContext,
    bp_id: u64,
    ip: u64,
) -> i64 {
    // Log breakpoint hit
    eprintln!("[JIT BREAKPOINT] id={} ip={}", bp_id, ip);

    // In a full implementation, this would check a debugger flag
    // and potentially pause execution. For now, always continue.
    0 // Continue
}

// =============================================================================
// Phase 1.3: Multi-Value Return - ReturnMulti, CollectN
// =============================================================================

/// Phase 1.3: Return multiple values for nondeterminism
///
/// Signals that the current computation has multiple results available.
/// The count parameter indicates how many values are on the stack to return.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - Stack must contain at least `count` values
///
/// # Returns
/// JIT_SIGNAL_YIELD if results were stored, JIT_SIGNAL_FAIL if no results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_return_multi(
    ctx: *mut JitContext,
    count: u64,
    _ip: u64,
) -> u64 {
    let ctx = ctx.as_mut().expect("return_multi: null context");

    let count = count as usize;
    if count == 0 {
        return JIT_SIGNAL_FAIL as u64;
    }

    // Store results from stack
    for i in 0..count {
        if ctx.results_count >= ctx.results_cap {
            break;
        }
        let stack_idx = ctx.sp.saturating_sub(count - i);
        let val = *ctx.value_stack.add(stack_idx);
        *ctx.results.add(ctx.results_count) = val;
        ctx.results_count += 1;
    }

    // Pop values from stack
    ctx.sp = ctx.sp.saturating_sub(count);

    JIT_SIGNAL_YIELD as u64
}

/// Phase 1.3: Collect up to N nondeterministic results
///
/// Collects at most `max_count` results from the results buffer into
/// an S-expression. If fewer results are available, returns those.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed S-expression containing collected results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_collect_n(
    ctx: *mut JitContext,
    max_count: u64,
    _ip: u64,
) -> u64 {
    let ctx = ctx.as_mut().expect("collect_n: null context");

    let count = (max_count as usize).min(ctx.results_count);

    if count == 0 {
        // Return empty S-expression
        let empty_sexpr = MettaValue::SExpr(vec![]);
        let boxed = Box::new(empty_sexpr);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | (ptr as u64 & PAYLOAD_MASK);
    }

    // Collect results into MettaValue vec
    let mut results = Vec::with_capacity(count);
    for i in 0..count {
        let jit_val = *ctx.results.add(i);
        let metta_val = jit_val.to_metta();
        results.push(metta_val);
    }

    // Clear collected results
    ctx.results_count = ctx.results_count.saturating_sub(count);

    // Return as S-expression
    let sexpr = MettaValue::SExpr(results);
    let boxed = Box::new(sexpr);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | (ptr as u64 & PAYLOAD_MASK)
}

// =============================================================================
// Phase 1.5: Global/Space Access - LoadGlobal, StoreGlobal, LoadSpace
// =============================================================================

/// Phase 1.5: Load global variable by symbol index
///
/// Looks up a global variable by its symbol index. First checks type annotations,
/// then falls back to checking if the atom exists in the space.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed value of the global, or Nil if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_global(
    ctx: *const JitContext,
    symbol_idx: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return JitValue::nil().to_bits(),
    };

    // Get symbol name from constants
    let symbol_name = if symbol_idx < ctx_ref.constants_len as u64 {
        let constant_ptr = ctx_ref.constants.add(symbol_idx as usize);
        let constant = &*constant_ptr;
        match constant {
            MettaValue::Atom(name) => Some(name.clone()),
            _ => None,
        }
    } else {
        None
    };

    let name = match symbol_name {
        Some(n) => n,
        None => return JitValue::nil().to_bits(),
    };

    // Try to look up through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.read();

        // First check if there's a type annotation for this symbol
        if let Ok(env_read) = env_guard {
            if let Some(type_val) = env_read.get_type(&name) {
                return metta_to_jit(&type_val).to_bits();
            }

            // Check if the atom itself exists in the space
            if env_read.has_fact(&name) {
                // Return the atom itself as the value
                return metta_to_jit(&MettaValue::Atom(name)).to_bits();
            }
        }
    }

    // Not found - return Nil
    JitValue::nil().to_bits()
}

/// Phase 1.5: Store global variable by symbol index
///
/// Stores a value in the global variable registry by its symbol index.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// Unit value on success
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_store_global(
    _ctx: *mut JitContext,
    symbol_idx: u64,
    _value: u64,
    _ip: u64,
) -> u64 {
    // Placeholder implementation - full implementation needs
    // mutable access to global registry
    let _ = symbol_idx;
    JitValue::unit().to_bits()
}

/// Phase 1.5: Load space handle by name index
///
/// Retrieves a space handle by its name. Spaces are named containers for atoms.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed space handle (currently returns Nil as placeholder)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_space(
    ctx: *const JitContext,
    name_idx: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::bytecode::space_registry::SpaceRegistry;
    use crate::backend::models::SpaceHandle;

    let ctx_ref = ctx.as_ref().expect("load_space: null context");

    // Get space name from constants
    let space_name = if name_idx < ctx_ref.constants_len as u64 {
        let constant_ptr = ctx_ref.constants.add(name_idx as usize);
        let constant = &*constant_ptr;
        match constant {
            MettaValue::Atom(name) => name.clone(),
            MettaValue::String(name) => name.clone(),
            _ => {
                // Not a valid space name
                return JitValue::nil().to_bits();
            }
        }
    } else {
        return JitValue::nil().to_bits();
    };

    // Space Ops Phase 2: Check for grounded references first
    // If this is a grounded reference and we have pre-resolved spaces, use them
    let grounded_idx = grounded_space_index(&space_name);
    if grounded_idx != u64::MAX && ctx_ref.has_grounded_spaces() {
        return jit_runtime_load_grounded_space(ctx, grounded_idx);
    }

    // If we have a space registry, use it to get or create the named space
    if !ctx_ref.space_registry.is_null() {
        let registry = &*(ctx_ref.space_registry as *const SpaceRegistry);
        let space = registry.get_or_create(&space_name);
        return metta_to_jit(&MettaValue::Space(space)).to_bits();
    }

    // Fallback: create a standalone space with the name (not shared)
    // This maintains backwards compatibility when no registry is available
    let space_id = {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        space_name.hash(&mut hasher);
        hasher.finish()
    };
    let space = SpaceHandle::new(space_id, space_name);
    metta_to_jit(&MettaValue::Space(space)).to_bits()
}

/// Space Ops Phase 1: Load pre-resolved grounded space by index
///
/// Retrieves a grounded space (&self, &kb, &stack) by its index.
/// These spaces are pre-resolved at JIT entry for O(1) access.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `index` - Grounded space index: 0 = &self, 1 = &kb, 2 = &stack
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - grounded_spaces must have been set via set_grounded_spaces()
///
/// # Returns
/// NaN-boxed space handle, or Nil if index out of bounds or not available
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_grounded_space(
    ctx: *const JitContext,
    index: u64,
) -> u64 {
    use crate::backend::models::SpaceHandle;

    let ctx = ctx.as_ref().expect("load_grounded_space: null context");

    // Check bounds
    let index = index as usize;
    if index >= ctx.grounded_spaces_count {
        return JitValue::nil().to_bits();
    }

    // Check if grounded spaces are set
    if ctx.grounded_spaces.is_null() {
        return JitValue::nil().to_bits();
    }

    // Load the pre-resolved space handle pointer
    let space_ptr = *ctx.grounded_spaces.add(index);
    if space_ptr.is_null() {
        return JitValue::nil().to_bits();
    }

    // The space_ptr points to a SpaceHandle
    let space_handle = &*(space_ptr as *const SpaceHandle);

    // Return as NaN-boxed heap pointer to a Space value
    // We need to create a MettaValue::Space and return its heap pointer
    metta_to_jit(&MettaValue::Space(space_handle.clone())).to_bits()
}

/// Space Ops Phase 1: Get grounded space index from name
///
/// Maps grounded reference names to indices:
/// - "self" or "&self" -> 0
/// - "kb" or "&kb" -> 1
/// - "stack" or "&stack" -> 2
///
/// # Returns
/// Index (0-2) or u64::MAX if not a grounded reference
#[inline]
pub fn grounded_space_index(name: &str) -> u64 {
    match name {
        "self" | "&self" => 0,
        "kb" | "&kb" => 1,
        "stack" | "&stack" => 2,
        _ => u64::MAX,
    }
}

/// Space Ops Phase 1: Check if a name is a grounded reference
#[inline]
pub fn is_grounded_ref(name: &str) -> bool {
    grounded_space_index(name) != u64::MAX
}

// =============================================================================
// Phase 1.6: Closure Support - LoadUpvalue
// =============================================================================

/// Phase 1.6: Load variable from enclosing scope (closures)
///
/// Traverses upvalue frames to load a captured variable.
/// - `depth`: How many frames up to traverse
/// - `index`: Index of variable within that frame
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed value from the closure environment, or Nil if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_upvalue(
    ctx: *const JitContext,
    depth: u64,
    index: u64,
    _ip: u64,
) -> u64 {
    let ctx = ctx.as_ref().expect("load_upvalue: null context");

    // Traverse binding frames to find the value
    // Each frame represents a scope level
    let depth = depth as usize;
    let index = index as usize;

    // Calculate target frame (binding frames are in reverse order)
    if ctx.binding_frames_count <= depth {
        return JitValue::nil().to_bits();
    }

    let frame_idx = ctx.binding_frames_count - 1 - depth;
    let frame = ctx.binding_frames.add(frame_idx);
    let frame_ref = &*frame;

    // Get value from that frame at the given index
    if index < frame_ref.entries_count {
        let entry = frame_ref.entries.add(index);
        let entry_ref = &*entry;
        return entry_ref.value.to_bits();
    }

    JitValue::nil().to_bits()
}

// =============================================================================
// Phase 1.7: S-Expression Operations - DeconAtom, Repr
// =============================================================================

/// Phase 1.7: Deconstruct an S-expression into (head, tail) pair
///
/// Given an S-expression, returns a new S-expression containing
/// the head (first element) and tail (remaining elements).
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed S-expression `(head tail)`, or Nil for non-S-expressions
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_decon_atom(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);
    let metta_val = jit_val.to_metta();

    match metta_val {
        MettaValue::SExpr(elems) if !elems.is_empty() => {
            let head = elems[0].clone();
            let tail = MettaValue::SExpr(elems[1..].to_vec());
            let result = MettaValue::SExpr(vec![head, tail]);
            metta_to_jit(&result).to_bits()
        }
        MettaValue::SExpr(_) => {
            // Empty S-expression - return (Nil, ())
            let result = MettaValue::SExpr(vec![
                MettaValue::Nil,
                MettaValue::SExpr(vec![]),
            ]);
            metta_to_jit(&result).to_bits()
        }
        _ => {
            // Non-S-expression - return Nil
            JitValue::nil().to_bits()
        }
    }
}

/// Phase 1.7: Convert value to string representation
///
/// Creates a string representation of any MeTTa value.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed String containing the representation
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_repr(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);
    let metta_val = jit_val.to_metta();

    // Format the value as a string (using Debug since Display not impl'd)
    let repr = format!("{:?}", metta_val);
    let result = MettaValue::String(repr);
    metta_to_jit(&result).to_bits()
}

// =============================================================================
// Phase 1.8: Higher-Order Operations - MapAtom, FilterAtom, FoldlAtom
// =============================================================================

/// Helper: Execute a template chunk with a single bound value (for map/filter)
///
/// Creates a mini-VM, pushes the binding value, and executes the template.
fn execute_template_single(chunk: &Arc<BytecodeChunk>, binding: MettaValue) -> MettaValue {
    let mut vm = BytecodeVM::new(Arc::clone(chunk));
    // Push binding as local slot 0
    vm.push_initial_value(binding);
    // Execute and return first result
    match vm.run() {
        Ok(results) => results.into_iter().next().unwrap_or(MettaValue::Unit),
        Err(_) => MettaValue::Unit,
    }
}

/// Helper: Execute a foldl template chunk with accumulator and item
///
/// Creates a mini-VM, pushes (acc, item) as local slots, and executes.
fn execute_foldl_template(chunk: &Arc<BytecodeChunk>, acc: MettaValue, item: MettaValue) -> MettaValue {
    let mut vm = BytecodeVM::new(Arc::clone(chunk));
    // Push acc as local slot 0, item as local slot 1
    vm.push_initial_value(acc);
    vm.push_initial_value(item);
    // Execute and return first result (the new accumulator)
    match vm.run() {
        Ok(results) => results.into_iter().next().unwrap_or(MettaValue::Unit),
        Err(_) => MettaValue::Unit,
    }
}

/// Phase 1.8: Map a function over S-expression elements
///
/// Applies a bytecode chunk to each element of an S-expression,
/// collecting results. Now executes natively using mini-VM for templates.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - ctx.current_chunk must point to a valid BytecodeChunk
///
/// # Returns
/// NaN-boxed S-expression with mapped results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_map_atom(
    ctx: *mut JitContext,
    list: u64,
    chunk_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return list, // Null context, return original
    };

    // Get the current chunk to access sub-chunks
    if ctx_ref.current_chunk.is_null() {
        // No chunk available, bail out
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
        return list;
    }

    // Get the list items
    let jit_list = JitValue::from_raw(list);
    let metta_list = jit_list.to_metta();
    let items = match metta_list {
        MettaValue::SExpr(items) => items,
        _ => {
            // Not a list, return original
            return list;
        }
    };

    // Get the template sub-chunk from the parent chunk
    let chunk = &*(ctx_ref.current_chunk as *const BytecodeChunk);
    let template_chunk = match chunk.get_chunk_constant(chunk_idx as u16) {
        Some(c) => c,
        None => {
            // Invalid chunk index, bail out
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
            return list;
        }
    };

    // Map over each element
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let result = execute_template_single(&template_chunk, item);
        results.push(result);
    }

    // Return mapped results as S-expression
    let result = MettaValue::SExpr(results);
    metta_to_jit(&result).to_bits()
}

/// Phase 1.8: Filter S-expression elements by predicate
///
/// Applies a predicate bytecode chunk to each element, keeping only
/// elements where the predicate returns true. Now executes natively.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - ctx.current_chunk must point to a valid BytecodeChunk
///
/// # Returns
/// NaN-boxed S-expression with filtered results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_filter_atom(
    ctx: *mut JitContext,
    list: u64,
    chunk_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return list,
    };

    // Get the current chunk to access sub-chunks
    if ctx_ref.current_chunk.is_null() {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
        return list;
    }

    // Get the list items
    let jit_list = JitValue::from_raw(list);
    let metta_list = jit_list.to_metta();
    let items = match metta_list {
        MettaValue::SExpr(items) => items,
        _ => return list,
    };

    // Get the predicate sub-chunk from the parent chunk
    let chunk = &*(ctx_ref.current_chunk as *const BytecodeChunk);
    let predicate_chunk = match chunk.get_chunk_constant(chunk_idx as u16) {
        Some(c) => c,
        None => {
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
            return list;
        }
    };

    // Filter elements where predicate returns true
    let mut results = Vec::new();
    for item in items {
        let result = execute_template_single(&predicate_chunk, item.clone());
        // Check if predicate returned true
        if matches!(result, MettaValue::Bool(true)) {
            results.push(item);
        }
    }

    // Return filtered results as S-expression
    let result = MettaValue::SExpr(results);
    metta_to_jit(&result).to_bits()
}

/// Phase 1.8: Left fold over S-expression elements
///
/// Applies a binary function to accumulator and each element,
/// threading the result through. Now executes natively.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - ctx.current_chunk must point to a valid BytecodeChunk
///
/// # Returns
/// Accumulated result
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_foldl_atom(
    ctx: *mut JitContext,
    list: u64,
    init: u64,
    chunk_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return init,
    };

    // Get the current chunk to access sub-chunks
    if ctx_ref.current_chunk.is_null() {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
        return init;
    }

    // Get the list items
    let jit_list = JitValue::from_raw(list);
    let metta_list = jit_list.to_metta();
    let items = match metta_list {
        MettaValue::SExpr(items) => items,
        _ => return init, // Not a list, return init
    };

    // Get the operation sub-chunk from the parent chunk
    let chunk = &*(ctx_ref.current_chunk as *const BytecodeChunk);
    let op_chunk = match chunk.get_chunk_constant(chunk_idx as u16) {
        Some(c) => c,
        None => {
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
            return init;
        }
    };

    // Get initial accumulator value
    let jit_init = JitValue::from_raw(init);
    let mut acc = jit_init.to_metta();

    // Fold over elements
    for item in items {
        acc = execute_foldl_template(&op_chunk, acc, item);
    }

    // Return accumulated result
    metta_to_jit(&acc).to_bits()
}

// =============================================================================
// Phase 1.9: Type Operations - GetMetaType
// =============================================================================

/// Phase 1.9: Get meta-level type of a value
///
/// Returns the meta-type of a value, which is one of:
/// - "Expression" for S-expressions
/// - "Symbol" for atoms/symbols
/// - "Variable" for variables
/// - "Grounded" for ground types (numbers, bools, strings)
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed Atom string representing the meta-type
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_metatype(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Determine meta-type from tag
    let tag = jit_val.tag();
    let metatype = match tag {
        t if t == TAG_HEAP => {
            // Could be SExpr or other heap type
            let metta = jit_val.to_metta();
            match metta {
                MettaValue::SExpr(_) => "Expression",
                MettaValue::String(_) => "Grounded",
                _ => "Expression",
            }
        }
        t if t == TAG_ATOM => "Symbol",
        t if t == TAG_VAR => "Variable",
        t if t == TAG_LONG => "Grounded",
        t if t == TAG_BOOL => "Grounded",
        t if t == TAG_NIL => "Grounded",
        t if t == TAG_UNIT => "Grounded",
        _ => "Unknown",
    };

    let result = MettaValue::Atom(metatype.to_string());
    metta_to_jit(&result).to_bits()
}

// =============================================================================
// Phase 1.10: MORK/Debug Operations - BloomCheck, Halt
// =============================================================================

/// Phase 1.10: Fast bloom filter check before MORK lookup
///
/// Performs a probabilistic check to determine if a key might exist
/// in the MORK trie. Returns:
/// - false: Key definitely does not exist (skip lookup)
/// - true: Key possibly exists (proceed with full lookup)
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed bool (always true as conservative fallback)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_bloom_check(
    _ctx: *const JitContext,
    _key: u64,
    _ip: u64,
) -> u64 {
    // Conservative implementation: always say "maybe present"
    // This means we never skip lookups, but we're always correct.
    // A full implementation would check the actual bloom filter.
    JitValue::from_bool(true).to_bits()
}

// Note: Halt (0xFF) is handled directly in JIT codegen by returning
// JIT_SIGNAL_HALT signal, so no runtime function is needed.

// =============================================================================
// Heap Tracking Runtime Functions
// =============================================================================

/// Track a heap allocation in the context for later cleanup.
///
/// This function should be called whenever a new heap value (Box<MettaValue>)
/// is created during JIT execution. The allocation will be freed when
/// `jit_runtime_cleanup_heap` is called.
///
/// # Arguments
/// * `ctx` - JIT context pointer (must have heap tracking enabled)
/// * `ptr` - Raw pointer to the Box<MettaValue> allocation
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext
/// - `ptr` must be from a valid `Box::into_raw(Box::new(MettaValue))` call
/// - Heap tracking should be enabled via `enable_heap_tracking`
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_track_heap(ctx: *mut JitContext, ptr: *mut MettaValue) {
    if let Some(ctx_ref) = ctx.as_mut() {
        ctx_ref.track_heap_allocation(ptr);
    }
}

/// Cleanup all tracked heap allocations.
///
/// This function frees all heap allocations that were tracked during JIT
/// execution. It should be called when JIT execution is complete.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext
/// - This should only be called once per JIT execution
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cleanup_heap(ctx: *mut JitContext) {
    if let Some(ctx_ref) = ctx.as_mut() {
        ctx_ref.cleanup_heap_allocations();
    }
}

/// Get the number of tracked heap allocations.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// Number of tracked allocations, or 0 if tracking is disabled
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_heap_count(ctx: *const JitContext) -> u64 {
    if let Some(ctx_ref) = ctx.as_ref() {
        // Safety: We're reading heap_tracker which is read-only here
        if ctx_ref.heap_tracker.is_null() {
            0
        } else {
            (*ctx_ref.heap_tracker).len() as u64
        }
    } else {
        0
    }
}

// =============================================================================
// State Operations Runtime (Phase D.1)
// =============================================================================

/// Create a new mutable state cell with an initial value.
///
/// Used by `new-state` operation. Creates a state cell in the environment
/// and returns a State(id) value.
///
/// # Arguments
/// - `ctx`: JIT context pointer (must have env_ptr set)
/// - `initial_value`: NaN-boxed initial value for the state
/// - `_ip`: Instruction pointer (for bailout tracking)
///
/// # Returns
/// NaN-boxed State(id) value, or error if environment is not available
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext with env_ptr set
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_new_state(
    ctx: *mut JitContext,
    initial_value: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return make_jit_error("new-state: null context"),
    };

    if ctx_ref.env_ptr.is_null() {
        return make_jit_error("new-state: environment not available");
    }

    // Cast env_ptr to Environment
    use crate::backend::Environment;
    let env = &mut *(ctx_ref.env_ptr as *mut Environment);

    // Convert JIT value to MettaValue
    let jit_val = JitValue::from_raw(initial_value);
    let metta_val = jit_val.to_metta();

    // Create state in environment
    let state_id = env.create_state(metta_val);

    // Return State(id) as heap-allocated MettaValue
    let state_val = MettaValue::State(state_id);
    metta_to_jit_tracked(&state_val, ctx).to_bits()
}

/// Get the current value from a state cell.
///
/// Used by `get-state` operation. Retrieves the current value from a state cell.
///
/// # Arguments
/// - `ctx`: JIT context pointer (must have env_ptr set)
/// - `state_handle`: NaN-boxed State(id) value
/// - `_ip`: Instruction pointer (for bailout tracking)
///
/// # Returns
/// NaN-boxed current value, or error if state not found
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext with env_ptr set
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_state(
    ctx: *mut JitContext,
    state_handle: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return make_jit_error("get-state: null context"),
    };

    if ctx_ref.env_ptr.is_null() {
        return make_jit_error("get-state: environment not available");
    }

    // Extract state ID from the handle
    let state_id = {
        let jit_val = JitValue::from_raw(state_handle);
        match jit_val.to_metta() {
            MettaValue::State(id) => id,
            other => {
                return make_jit_error_with_details(
                    "get-state: expected State",
                    &format!("got {:?}", other),
                );
            }
        }
    };

    // Optimization 5.1: Check state cache first
    if let Some(cached_value) = ctx_ref.state_cache_get(state_id) {
        return cached_value.to_bits();
    }

    // Cache miss: fetch from Environment
    use crate::backend::Environment;
    let env = &*(ctx_ref.env_ptr as *const Environment);

    // Get state value
    match env.get_state(state_id) {
        Some(value) => {
            // Convert to JIT value with tracking
            let jit_value = metta_to_jit_tracked(&value, ctx);

            // Update cache with the fetched value
            ctx_ref.state_cache_put(state_id, jit_value);

            jit_value.to_bits()
        }
        None => make_jit_error_with_details(
            "get-state: state not found",
            &format!("state_id={}", state_id),
        ),
    }
}

/// Change the value of a state cell.
///
/// Used by `change-state!` operation. Updates the value in a state cell
/// and returns the state handle.
///
/// # Arguments
/// - `ctx`: JIT context pointer (must have env_ptr set)
/// - `state_handle`: NaN-boxed State(id) value
/// - `new_value`: NaN-boxed new value for the state
/// - `_ip`: Instruction pointer (for bailout tracking)
///
/// # Returns
/// NaN-boxed State(id) value (same as input), or error if state not found
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext with env_ptr set
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_change_state(
    ctx: *mut JitContext,
    state_handle: u64,
    new_value: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return make_jit_error("change-state!: null context"),
    };

    if ctx_ref.env_ptr.is_null() {
        return make_jit_error("change-state!: environment not available");
    }

    // Extract state ID from the handle
    let state_id = {
        let jit_val = JitValue::from_raw(state_handle);
        match jit_val.to_metta() {
            MettaValue::State(id) => id,
            other => {
                return make_jit_error_with_details(
                    "change-state!: expected State",
                    &format!("got {:?}", other),
                );
            }
        }
    };

    // Convert new value to MettaValue
    let jit_new_val = JitValue::from_raw(new_value);
    let metta_new_val = jit_new_val.to_metta();

    // Cast env_ptr to Environment
    use crate::backend::Environment;
    let env = &mut *(ctx_ref.env_ptr as *mut Environment);

    // Change state value
    if env.change_state(state_id, metta_new_val) {
        // Optimization 5.1: Update cache with the new value
        // (more efficient than invalidating since next read will be a hit)
        ctx_ref.state_cache_put(state_id, jit_new_val);

        // Return the state handle unchanged
        state_handle
    } else {
        make_jit_error_with_details(
            "change-state!: state not found",
            &format!("state_id={}", state_id),
        )
    }
}

/// Helper to create an error JitValue with a message (for state operations)
fn make_jit_error(msg: &str) -> u64 {
    use std::sync::Arc;
    let error_val = MettaValue::Error(
        msg.to_string(),
        Arc::new(MettaValue::Nil),
    );
    let boxed = Box::new(error_val);
    let ptr = Box::into_raw(boxed);
    ((TAG_HEAP as u64) << 48) | (ptr as u64 & PAYLOAD_MASK)
}

/// Helper to create an error JitValue with message and details (for state operations)
fn make_jit_error_with_details(msg: &str, details: &str) -> u64 {
    use std::sync::Arc;
    let error_val = MettaValue::Error(
        msg.to_string(),
        Arc::new(MettaValue::Atom(details.to_string())),
    );
    let boxed = Box::new(error_val);
    let ptr = Box::into_raw(boxed);
    ((TAG_HEAP as u64) << 48) | (ptr as u64 & PAYLOAD_MASK)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pow_positive() {
        let base = box_long(2);
        let exp = box_long(10);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1024);
    }

    #[test]
    fn test_pow_zero_exp() {
        let base = box_long(5);
        let exp = box_long(0);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1);
    }

    #[test]
    fn test_pow_negative_exp() {
        let base = box_long(2);
        let exp = box_long(-1);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 0); // Integer division truncates
    }

    #[test]
    fn test_pow_one_negative_exp() {
        let base = box_long(1);
        let exp = box_long(-5);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1); // 1^anything = 1
    }

    #[test]
    fn test_abs() {
        let neg = box_long(-42);
        let result = unsafe { jit_runtime_abs(neg) };
        assert_eq!(extract_long_signed(result), 42);

        let pos = box_long(42);
        let result = unsafe { jit_runtime_abs(pos) };
        assert_eq!(extract_long_signed(result), 42);
    }

    #[test]
    fn test_signum() {
        assert_eq!(
            extract_long_signed(unsafe { jit_runtime_signum(box_long(-42)) }),
            -1
        );
        assert_eq!(
            extract_long_signed(unsafe { jit_runtime_signum(box_long(0)) }),
            0
        );
        assert_eq!(
            extract_long_signed(unsafe { jit_runtime_signum(box_long(42)) }),
            1
        );
    }

    #[test]
    fn test_is_long() {
        let long = box_long(42);
        let result = jit_runtime_is_long(long);
        assert_eq!(result & 1, 1); // true

        let bool_val = super::super::types::TAG_BOOL | 1;
        let result = jit_runtime_is_long(bool_val);
        assert_eq!(result & 1, 0); // false
    }

    #[test]
    fn test_extract_long_signed() {
        // Positive value
        let pos = box_long(12345);
        assert_eq!(extract_long_signed(pos), 12345);

        // Negative value
        let neg = box_long(-12345);
        assert_eq!(extract_long_signed(neg), -12345);

        // Zero
        let zero = box_long(0);
        assert_eq!(extract_long_signed(zero), 0);

        // Max 48-bit positive
        let max = box_long((1i64 << 47) - 1);
        assert_eq!(extract_long_signed(max), (1i64 << 47) - 1);

        // Min 48-bit negative
        let min = box_long(-(1i64 << 47));
        assert_eq!(extract_long_signed(min), -(1i64 << 47));
    }

    // =========================================================================
    // Choice Point Tests
    // =========================================================================

    #[test]
    fn test_push_choice_point_success() {
        // Create context with choice point support
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Create some alternatives
        let alts = [
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
            JitAlternative::value(JitValue::from_long(3)),
        ];

        // Push a choice point
        let result = unsafe {
            jit_runtime_push_choice_point(
                &mut ctx,
                3,
                alts.as_ptr(),
                100,
                std::ptr::null(),
            )
        };

        assert_eq!(result, 0); // Success
        assert_eq!(ctx.choice_point_count, 1);
    }

    #[test]
    fn test_push_choice_point_overflow() {
        // Create context with only 1 choice point slot
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 1];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                1, // Only 1 slot
                results.as_mut_ptr(),
                results.len(),
            )
        };

        let alts = [JitAlternative::value(JitValue::from_long(1))];

        // First push succeeds
        let result = unsafe {
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(result, 0);

        // Second push should fail (overflow)
        let result = unsafe {
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(result, -1); // Overflow
        assert!(ctx.bailout);
    }

    #[test]
    fn test_fail_with_alternatives() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        let alts = [
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
        ];

        // Push choice point
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 2, alts.as_ptr(), 0, std::ptr::null());
        }

        // First fail should return first alternative
        let tag = unsafe { jit_runtime_fail(&mut ctx) };
        assert_eq!(tag, JitAlternativeTag::Value as i64);

        // Get the alternative
        let alt = unsafe { jit_runtime_get_current_alternative(&ctx) };
        assert_eq!(alt.tag, JitAlternativeTag::Value);
        let val = JitValue::from_raw(alt.payload);
        assert_eq!(val.as_long(), 1);

        // Second fail should return second alternative
        let tag = unsafe { jit_runtime_fail(&mut ctx) };
        assert_eq!(tag, JitAlternativeTag::Value as i64);

        let alt = unsafe { jit_runtime_get_current_alternative(&ctx) };
        let val = JitValue::from_raw(alt.payload);
        assert_eq!(val.as_long(), 2);

        // Third fail should return -1 (no more alternatives)
        let tag = unsafe { jit_runtime_fail(&mut ctx) };
        assert_eq!(tag, -1);
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_yield_stores_result_and_signals_bailout() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Create the value to yield (as NaN-boxed u64)
        let yield_value = JitValue::from_long(42);

        // Yield the value (Phase 4: value is passed as argument, not popped from stack)
        let _result = unsafe { jit_runtime_yield(&mut ctx, yield_value.to_bits(), 0) };

        // Should have stored the result
        assert_eq!(ctx.results_count, 1);
        let stored = unsafe { *ctx.results.add(0) };
        assert_eq!(stored.as_long(), 42);

        // Should have signaled bailout with Yield reason
        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::Yield);
    }

    #[test]
    fn test_cut_clears_choice_points() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        let alts = [JitAlternative::value(JitValue::from_long(1))];

        // Push multiple choice points
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null());
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null());
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null());
        }
        assert_eq!(ctx.choice_point_count, 3);

        // Cut should clear all
        unsafe { jit_runtime_cut(&mut ctx, 0) };
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_context_has_nondet_support() {
        // Context without nondet support
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
            )
        };
        assert!(!ctx.has_nondet_support());

        // Context with nondet support
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];
        let ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        assert!(ctx.has_nondet_support());
    }

    // =========================================================================
    // Stage 2: Native Nondeterminism Tests
    // =========================================================================

    #[test]
    fn test_yield_native_stores_result() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Yield a value using native function
        let value = JitValue::from_long(42);
        let signal = unsafe { jit_runtime_yield_native(&mut ctx, value.to_bits(), 10) };

        // Should return YIELD signal
        assert_eq!(signal, super::JIT_SIGNAL_YIELD);

        // Should have stored the result
        assert_eq!(ctx.results_count, 1);
        let stored = unsafe { *ctx.results.add(0) };
        assert_eq!(stored.as_long(), 42);

        // Should have set resume_ip
        assert_eq!(ctx.resume_ip, 10);
    }

    #[test]
    fn test_collect_native_gathers_results() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Store some results manually
        unsafe {
            *ctx.results.add(0) = JitValue::from_long(1);
            *ctx.results.add(1) = JitValue::from_long(2);
            *ctx.results.add(2) = JitValue::from_long(3);
        }
        ctx.results_count = 3;

        // Collect results
        let result = unsafe { jit_runtime_collect_native(&mut ctx) };

        // Should return a heap pointer
        let tag = result & super::super::types::TAG_MASK;
        assert_eq!(tag, super::super::types::TAG_HEAP);

        // Results should be cleared
        assert_eq!(ctx.results_count, 0);

        // Verify the SExpr contents
        let ptr = (result & PAYLOAD_MASK) as *const MettaValue;
        let metta_val = unsafe { &*ptr };
        if let MettaValue::SExpr(items) = metta_val {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Long(1));
            assert_eq!(items[1], MettaValue::Long(2));
            assert_eq!(items[2], MettaValue::Long(3));
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_has_alternatives() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // No choice points = no alternatives
        let has_alts = unsafe { jit_runtime_has_alternatives(&ctx) };
        assert_eq!(has_alts, 0);

        // Add a choice point with alternatives
        let alts = [
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
        ];
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 2, alts.as_ptr(), 0, std::ptr::null());
        }

        // Should now have alternatives
        let has_alts = unsafe { jit_runtime_has_alternatives(&ctx) };
        assert_eq!(has_alts, 1);
    }

    #[test]
    fn test_fail_native_exhausts_alternatives() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.sp = 5; // Set some stack pointer

        // Add a choice point with 2 alternatives (using inline alternatives)
        let mut cp = JitChoicePoint::default();
        cp.saved_sp = 2; // Save sp at 2
        cp.alt_count = 2;
        cp.current_index = 0;
        cp.alternatives_inline[0] = JitAlternative::value(JitValue::from_long(10));
        cp.alternatives_inline[1] = JitAlternative::value(JitValue::from_long(20));
        cp.saved_ip = 100;
        cp.saved_chunk = std::ptr::null();
        cp.saved_stack_pool_idx = -1; // No saved stack
        cp.saved_stack_count = 0;
        cp.fork_depth = 0;
        cp.saved_binding_frames_count = 0;
        cp.is_collect_boundary = false;
        unsafe {
            *ctx.choice_points.add(0) = cp;
        }
        ctx.choice_point_count = 1;

        // First fail should return first alternative and restore sp
        let result1 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let jv1 = JitValue::from_raw(result1);
        assert_eq!(jv1.as_long(), 10);
        assert_eq!(ctx.sp, 2); // sp restored

        // Second fail should return second alternative
        let result2 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let jv2 = JitValue::from_raw(result2);
        assert_eq!(jv2.as_long(), 20);

        // Third fail should exhaust and return FAIL signal
        let result3 = unsafe { jit_runtime_fail_native(&mut ctx) };
        assert_eq!(result3, super::JIT_SIGNAL_FAIL as u64);
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_save_restore_stack() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.saved_stack = saved_stack.as_mut_ptr();
        ctx.saved_stack_cap = saved_stack.len();

        // Set up some stack values
        unsafe {
            *ctx.value_stack.add(0) = JitValue::from_long(100);
            *ctx.value_stack.add(1) = JitValue::from_long(200);
            *ctx.value_stack.add(2) = JitValue::from_long(300);
        }
        ctx.sp = 3;

        // Save stack
        let signal = unsafe { jit_runtime_save_stack(&mut ctx) };
        assert_eq!(signal, super::JIT_SIGNAL_OK);
        assert_eq!(ctx.saved_stack_count, 3);

        // Modify stack
        unsafe {
            *ctx.value_stack.add(0) = JitValue::from_long(999);
            *ctx.value_stack.add(1) = JitValue::from_long(888);
        }
        ctx.sp = 2;

        // Restore stack
        let signal = unsafe { jit_runtime_restore_stack(&mut ctx) };
        assert_eq!(signal, super::JIT_SIGNAL_OK);
        assert_eq!(ctx.sp, 3);

        // Verify restored values
        let v0 = unsafe { *ctx.value_stack.add(0) };
        let v1 = unsafe { *ctx.value_stack.add(1) };
        let v2 = unsafe { *ctx.value_stack.add(2) };
        assert_eq!(v0.as_long(), 100);
        assert_eq!(v1.as_long(), 200);
        assert_eq!(v2.as_long(), 300);
    }

    #[test]
    fn test_signal_constants() {
        // Verify signal constants are distinct and sensible
        assert_eq!(super::JIT_SIGNAL_OK, 0);
        assert_eq!(super::JIT_SIGNAL_YIELD, 2);
        assert_eq!(super::JIT_SIGNAL_FAIL, 3);
        assert_eq!(super::JIT_SIGNAL_ERROR, -1);

        // Verify they're all different
        assert_ne!(super::JIT_SIGNAL_OK, super::JIT_SIGNAL_YIELD);
        assert_ne!(super::JIT_SIGNAL_OK, super::JIT_SIGNAL_FAIL);
        assert_ne!(super::JIT_SIGNAL_OK, super::JIT_SIGNAL_ERROR);
        assert_ne!(super::JIT_SIGNAL_YIELD, super::JIT_SIGNAL_FAIL);
        assert_ne!(super::JIT_SIGNAL_YIELD, super::JIT_SIGNAL_ERROR);
        assert_ne!(super::JIT_SIGNAL_FAIL, super::JIT_SIGNAL_ERROR);
    }

    #[test]
    fn test_collect_results() {
        // Test the collect_results helper function
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Store some results
        unsafe {
            *ctx.results.add(0) = JitValue::from_long(10);
            *ctx.results.add(1) = JitValue::from_long(20);
            *ctx.results.add(2) = JitValue::from_long(30);
        }
        ctx.results_count = 3;

        // Collect results
        let collected = unsafe { super::collect_results(&mut ctx) };

        assert_eq!(collected.len(), 3);
        assert_eq!(collected[0], MettaValue::Long(10));
        assert_eq!(collected[1], MettaValue::Long(20));
        assert_eq!(collected[2], MettaValue::Long(30));
    }

    #[test]
    fn test_execute_once() {
        // Test the execute_once helper function with a simple JIT function
        // that just returns a constant
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Simulate a JIT function that pushes 42 and returns OK
        unsafe extern "C" fn mock_jit_fn(ctx: *mut JitContext) -> i64 {
            let ctx_ref = ctx.as_mut().unwrap();
            *ctx_ref.value_stack.add(ctx_ref.sp) = JitValue::from_long(42);
            ctx_ref.sp += 1;
            super::super::JIT_SIGNAL_OK
        }

        let result = unsafe { super::execute_once(&mut ctx, mock_jit_fn) };

        assert!(result.is_some());
        assert_eq!(result.unwrap(), MettaValue::Long(42));
    }

    // =========================================================================
    // Phase 2.2: Fork/Yield/Collect Full Cycle Integration Test
    // =========================================================================
    // Tests the complete nondeterminism workflow:
    // 1. Fork creates choice points with multiple alternatives
    // 2. Yield stores results for each alternative
    // 3. Collect gathers all results into an S-expression
    // =========================================================================

    #[test]
    fn test_fork_yield_collect_full_cycle() {
        // Create context with nondeterminism support
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 32];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 32];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::nil(); 32];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.saved_stack = saved_stack.as_mut_ptr();
        ctx.saved_stack_cap = saved_stack.len();

        // =====================================================================
        // Phase 1: Fork - Create choice point with 3 alternatives (1, 2, 3)
        // =====================================================================
        let alternatives = vec![
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
            JitAlternative::value(JitValue::from_long(3)),
        ];
        let alts_ptr = Box::leak(alternatives.into_boxed_slice()).as_ptr();

        // Push the fork choice point
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 3, alts_ptr, 0, std::ptr::null());
        }

        // Verify choice point was created
        assert_eq!(ctx.choice_point_count, 1);
        let has_alts = unsafe { jit_runtime_has_alternatives(&ctx) };
        assert_eq!(has_alts, 1);

        // =====================================================================
        // Phase 2: Process each alternative and Yield results
        // =====================================================================
        // Simulate the evaluation loop:
        // - Get next alternative via fail_native
        // - Yield the result
        // - Repeat until no more alternatives

        // Process alternative 1
        let alt1 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let val1 = JitValue::from_raw(alt1);
        assert_eq!(val1.as_long(), 1);

        // Yield alternative 1
        let signal1 = unsafe { jit_runtime_yield_native(&mut ctx, val1.to_bits(), 0) };
        assert_eq!(signal1, super::JIT_SIGNAL_YIELD);
        assert_eq!(ctx.results_count, 1);

        // Process alternative 2
        let alt2 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let val2 = JitValue::from_raw(alt2);
        assert_eq!(val2.as_long(), 2);

        // Yield alternative 2
        let signal2 = unsafe { jit_runtime_yield_native(&mut ctx, val2.to_bits(), 0) };
        assert_eq!(signal2, super::JIT_SIGNAL_YIELD);
        assert_eq!(ctx.results_count, 2);

        // Process alternative 3
        let alt3 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let val3 = JitValue::from_raw(alt3);
        assert_eq!(val3.as_long(), 3);

        // Yield alternative 3
        let signal3 = unsafe { jit_runtime_yield_native(&mut ctx, val3.to_bits(), 0) };
        assert_eq!(signal3, super::JIT_SIGNAL_YIELD);
        assert_eq!(ctx.results_count, 3);

        // No more alternatives - fail_native returns FAIL signal
        let alt4 = unsafe { jit_runtime_fail_native(&mut ctx) };
        assert_eq!(alt4, super::JIT_SIGNAL_FAIL as u64);
        assert_eq!(ctx.choice_point_count, 0);

        // =====================================================================
        // Phase 3: Collect all yielded results
        // =====================================================================
        let collected_raw = unsafe { jit_runtime_collect_native(&mut ctx) };

        // Verify it's a heap pointer (TAG_HEAP)
        let tag = collected_raw & super::super::types::TAG_MASK;
        assert_eq!(tag, super::super::types::TAG_HEAP);

        // Results should be cleared after collection
        assert_eq!(ctx.results_count, 0);

        // =====================================================================
        // Phase 4: Verify the collected S-expression
        // =====================================================================
        let ptr = (collected_raw & PAYLOAD_MASK) as *const MettaValue;
        let metta_val = unsafe { &*ptr };

        if let MettaValue::SExpr(items) = metta_val {
            assert_eq!(items.len(), 3, "Expected 3 collected results");
            assert_eq!(items[0], MettaValue::Long(1), "First result should be 1");
            assert_eq!(items[1], MettaValue::Long(2), "Second result should be 2");
            assert_eq!(items[2], MettaValue::Long(3), "Third result should be 3");
        } else {
            panic!("Expected SExpr, got {:?}", metta_val);
        }
    }

    #[test]
    fn test_nested_fork_yield_collect() {
        // Test nested Fork/Yield/Collect with two levels of nondeterminism
        // Outer fork: alternatives A, B
        // For each outer, inner fork: alternatives 1, 2
        // Expected results: (A 1), (A 2), (B 1), (B 2)

        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.saved_stack = saved_stack.as_mut_ptr();
        ctx.saved_stack_cap = saved_stack.len();

        // Create heap-allocated MettaValues for atoms
        let atom_a = Box::leak(Box::new(MettaValue::Atom("A".to_string())));
        let atom_b = Box::leak(Box::new(MettaValue::Atom("B".to_string())));

        // Outer fork: A, B
        let outer_alts = vec![
            JitAlternative::value(JitValue::from_heap_ptr(atom_a)),
            JitAlternative::value(JitValue::from_heap_ptr(atom_b)),
        ];
        let outer_ptr = Box::leak(outer_alts.into_boxed_slice()).as_ptr();

        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 2, outer_ptr, 0, std::ptr::null());
        }
        assert_eq!(ctx.choice_point_count, 1);

        let mut collected_pairs: Vec<(String, i64)> = Vec::new();

        // Process outer alternatives
        for outer_idx in 0..2 {
            // Get outer alternative
            let outer_val_raw = unsafe { jit_runtime_fail_native(&mut ctx) };
            if outer_val_raw == super::JIT_SIGNAL_FAIL as u64 {
                break;
            }
            let outer_val = JitValue::from_raw(outer_val_raw);

            // Extract atom name (to_metta() returns MettaValue directly)
            let metta = unsafe { outer_val.to_metta() };
            let outer_name = if let MettaValue::Atom(name) = metta {
                name
            } else {
                panic!("Expected Atom for outer, got {:?}", metta);
            };

            // Inner fork: 1, 2
            let inner_alts = vec![
                JitAlternative::value(JitValue::from_long(1)),
                JitAlternative::value(JitValue::from_long(2)),
            ];
            let inner_ptr = Box::leak(inner_alts.into_boxed_slice()).as_ptr();

            unsafe {
                jit_runtime_push_choice_point(&mut ctx, 2, inner_ptr, 0, std::ptr::null());
            }

            // Process inner alternatives
            for _inner_idx in 0..2 {
                let inner_val_raw = unsafe { jit_runtime_fail_native(&mut ctx) };
                if inner_val_raw == super::JIT_SIGNAL_FAIL as u64 {
                    break;
                }
                let inner_val = JitValue::from_raw(inner_val_raw);
                let inner_num = inner_val.as_long();

                // Record the pair
                collected_pairs.push((outer_name.clone(), inner_num));

                // Yield combined result (as a simple encoding: outer_idx * 10 + inner_num)
                let combined = JitValue::from_long(outer_idx as i64 * 10 + inner_num);
                unsafe {
                    jit_runtime_yield_native(&mut ctx, combined.to_bits(), 0);
                }
            }
        }

        // Verify we collected all 4 combinations
        assert_eq!(collected_pairs.len(), 4);
        assert!(collected_pairs.contains(&("A".to_string(), 1)));
        assert!(collected_pairs.contains(&("A".to_string(), 2)));
        assert!(collected_pairs.contains(&("B".to_string(), 1)));
        assert!(collected_pairs.contains(&("B".to_string(), 2)));

        // Verify results were yielded
        assert_eq!(ctx.results_count, 4);

        // Collect all results
        let collected_raw = unsafe { jit_runtime_collect_native(&mut ctx) };
        let tag = collected_raw & super::super::types::TAG_MASK;
        assert_eq!(tag, super::super::types::TAG_HEAP);

        let ptr = (collected_raw & PAYLOAD_MASK) as *const MettaValue;
        let metta_val = unsafe { &*ptr };

        if let MettaValue::SExpr(items) = metta_val {
            assert_eq!(items.len(), 4, "Expected 4 collected results");
            // Results should be: 1 (A,1), 2 (A,2), 11 (B,1), 12 (B,2)
            assert_eq!(items[0], MettaValue::Long(1));  // A*10 + 1 = 0*10 + 1 = 1
            assert_eq!(items[1], MettaValue::Long(2));  // A*10 + 2 = 0*10 + 2 = 2
            assert_eq!(items[2], MettaValue::Long(11)); // B*10 + 1 = 1*10 + 1 = 11
            assert_eq!(items[3], MettaValue::Long(12)); // B*10 + 2 = 1*10 + 2 = 12
        } else {
            panic!("Expected SExpr, got {:?}", metta_val);
        }
    }

    #[test]
    fn test_fork_with_early_cut() {
        // Test that cut properly terminates nondeterministic search
        // Fork with 5 alternatives, but cut after finding the first even number

        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 32];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 32];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Fork with 5 alternatives: 1, 2, 3, 4, 5
        let alternatives = vec![
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
            JitAlternative::value(JitValue::from_long(3)),
            JitAlternative::value(JitValue::from_long(4)),
            JitAlternative::value(JitValue::from_long(5)),
        ];
        let alts_ptr = Box::leak(alternatives.into_boxed_slice()).as_ptr();

        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 5, alts_ptr, 0, std::ptr::null());
        }
        assert_eq!(ctx.choice_point_count, 1);

        let mut found_even = false;
        let mut iterations = 0;

        while !found_even {
            iterations += 1;
            let val_raw = unsafe { jit_runtime_fail_native(&mut ctx) };
            if val_raw == super::JIT_SIGNAL_FAIL as u64 {
                break;
            }

            let val = JitValue::from_raw(val_raw);
            let num = val.as_long();

            if num % 2 == 0 {
                // Found even number, yield it and cut
                unsafe {
                    jit_runtime_yield_native(&mut ctx, val.to_bits(), 0);
                    jit_runtime_cut(&mut ctx, 0);
                }
                found_even = true;
            }
        }

        // Should have found even number (2) after 2 iterations (1, 2)
        assert!(found_even);
        assert_eq!(iterations, 2);

        // Cut should have cleared all choice points
        assert_eq!(ctx.choice_point_count, 0);

        // Should have only one result (the first even number found: 2)
        assert_eq!(ctx.results_count, 1);
        let result = unsafe { *ctx.results.add(0) };
        assert_eq!(result.as_long(), 2);
    }
}
