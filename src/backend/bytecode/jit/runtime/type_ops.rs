//! Type operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable type operations:
//! - get_type - Get the type name of a value
//! - check_type - Check if value type matches expected type
//! - assert_type - Assert type match or signal error
//!
//! ## Zero-Conversion Support
//!
//! Generic variants (`get_type_generic`) support both heap and arena modes
//! without type conversion overhead by using factories for value creation.

use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValueMode, PAYLOAD_MASK, TAG_ATOM, TAG_BOOL, TAG_ERROR,
    TAG_HEAP, TAG_LONG, TAG_MASK, TAG_NIL, TAG_UNIT, TAG_VAR,
};
use crate::backend::models::{
    ArenaValue, ArenaValueFactory, MettaValue, MettaValueFactory, MettaValueInner, MettaValueTrait,
};
use bumpalo::Bump;

use super::helpers::value_to_jit_generic;

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
/// Dispatches at runtime based on `ctx.value_mode`:
/// - Heap mode: Creates a heap-allocated `MettaValue::Atom`
/// - Arena mode: Uses `ArenaValueFactory` for arena allocation
///
/// # Safety
/// For heap pointers, the referenced MettaValue must be valid.
/// For arena mode, ctx.arena must be a valid arena pointer.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_type(ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    // Check context for mode dispatch
    if !ctx.is_null() {
        let ctx_ref = &*ctx;
        if ctx_ref.is_arena_mode() {
            // Arena mode: delegate to generic implementation
            let arena_ptr = ctx_ref.arena_ptr();
            debug_assert!(
                !arena_ptr.is_null(),
                "jit_runtime_get_type: Arena mode requires arena pointer"
            );
            let arena: &'static Bump = &*(arena_ptr as *const Bump);
            let factory = ArenaValueFactory::new(arena);
            return get_type_generic::<ArenaValue<'static>, ArenaValueFactory<'static>>(
                ctx,
                val,
                &factory,
                JitValueMode::Arena,
            );
        }
    }

    // Heap mode: original implementation
    let tag = val & TAG_MASK;

    let type_name: &'static str = match tag {
        TAG_LONG => TYPE_NAME_NUMBER,
        TAG_BOOL => TYPE_NAME_BOOL,
        TAG_NIL => TYPE_NAME_NIL,
        TAG_UNIT => TYPE_NAME_UNIT,
        TAG_ERROR => TYPE_NAME_ERROR,
        TAG_VAR => TYPE_NAME_VARIABLE,
        TAG_ATOM => {
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
        TAG_HEAP => {
            // Need to inspect the heap value
            let ptr = (val & PAYLOAD_MASK) as *const MettaValue;
            if ptr.is_null() {
                TYPE_NAME_UNKNOWN
            } else {
                match (*ptr).inner() {
                    MettaValueInner::SExpr(_) => TYPE_NAME_EXPRESSION,
                    MettaValueInner::String(_) => TYPE_NAME_STRING,
                    MettaValueInner::Type(_) => TYPE_NAME_TYPE,
                    MettaValueInner::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                    MettaValueInner::Space(_) => TYPE_NAME_SPACE,
                    MettaValueInner::State(_) => TYPE_NAME_STATE,
                    MettaValueInner::Memo(_) => TYPE_NAME_MEMO,
                    MettaValueInner::Empty => TYPE_NAME_EMPTY,
                    MettaValueInner::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                    MettaValueInner::Atom(_) => TYPE_NAME_SYMBOL,
                    MettaValueInner::Bool(_) => TYPE_NAME_BOOL,
                    MettaValueInner::Long(_) | MettaValueInner::Float(_) => TYPE_NAME_NUMBER,
                    MettaValueInner::Nil => TYPE_NAME_NIL,
                    MettaValueInner::Error(_, _) => TYPE_NAME_ERROR,
                    MettaValueInner::Unit => TYPE_NAME_UNIT,
                }
            }
        }
        _ => TYPE_NAME_UNKNOWN,
    };

    // Return as a Symbol (heap-allocated MettaValue::Atom)
    // We create a new MettaValue::Atom and return it as a heap pointer
    let atom = Box::new(MettaValue::Atom(type_name.to_string()));
    let ptr = Box::into_raw(atom);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
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
        let type_tag = type_atom & TAG_MASK;
        match type_tag {
            TAG_ATOM => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const String;
                if !ptr.is_null() {
                    Some((&*ptr).as_str())
                } else {
                    None
                }
            }
            TAG_HEAP => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const MettaValue;
                if !ptr.is_null() {
                    if let MettaValueInner::Atom(s) = (*ptr).inner() {
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
        return TAG_BOOL; // false
    };

    // Type variables match anything
    if expected.starts_with('$') {
        return TAG_BOOL | 1; // true
    }

    // Get the actual type of the value
    let actual_type = get_type_name(val);

    // Compare types
    let matches = actual_type == expected;
    TAG_BOOL | (matches as u64)
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
        let type_tag = type_atom & TAG_MASK;
        match type_tag {
            TAG_ATOM => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const String;
                if !ptr.is_null() {
                    Some((&*ptr).as_str())
                } else {
                    None
                }
            }
            TAG_HEAP => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const MettaValue;
                if !ptr.is_null() {
                    if let MettaValueInner::Atom(s) = (*ptr).inner() {
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
    let actual_type = get_type_name(val);

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
// Generic Type Operations (Zero-Conversion Support)
// =============================================================================

/// Get the type name of a value using a factory (generic version).
///
/// This function supports both heap and arena modes by using the provided
/// factory to create the type name atom.
///
/// # Type Parameters
/// - `V`: The value type implementing `MettaValueTrait`
/// - `F`: The factory type for creating values
///
/// # Safety
/// For heap pointers, the referenced value must be valid.
pub unsafe fn get_type_generic<V, F>(
    _ctx: *mut JitContext,
    val: u64,
    factory: &F,
    mode: JitValueMode,
) -> u64
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let tag = val & TAG_MASK;

    let type_name: &'static str = match tag {
        TAG_LONG => TYPE_NAME_NUMBER,
        TAG_BOOL => TYPE_NAME_BOOL,
        TAG_NIL => TYPE_NAME_NIL,
        TAG_UNIT => TYPE_NAME_UNIT,
        TAG_ERROR => TYPE_NAME_ERROR,
        TAG_VAR => TYPE_NAME_VARIABLE,
        TAG_ATOM => {
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
        TAG_HEAP => {
            // For generic, we need to use MettaValueTrait
            // Since we can't know the concrete type at compile time for the pointer,
            // we fall back to checking if it's a MettaValue pointer
            let ptr = (val & PAYLOAD_MASK) as *const MettaValue;
            if ptr.is_null() {
                TYPE_NAME_UNKNOWN
            } else {
                match (*ptr).inner() {
                    MettaValueInner::SExpr(_) => TYPE_NAME_EXPRESSION,
                    MettaValueInner::String(_) => TYPE_NAME_STRING,
                    MettaValueInner::Type(_) => TYPE_NAME_TYPE,
                    MettaValueInner::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                    MettaValueInner::Space(_) => TYPE_NAME_SPACE,
                    MettaValueInner::State(_) => TYPE_NAME_STATE,
                    MettaValueInner::Memo(_) => TYPE_NAME_MEMO,
                    MettaValueInner::Empty => TYPE_NAME_EMPTY,
                    MettaValueInner::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                    MettaValueInner::Atom(_) => TYPE_NAME_SYMBOL,
                    MettaValueInner::Bool(_) => TYPE_NAME_BOOL,
                    MettaValueInner::Long(_) | MettaValueInner::Float(_) => TYPE_NAME_NUMBER,
                    MettaValueInner::Nil => TYPE_NAME_NIL,
                    MettaValueInner::Error(_, _) => TYPE_NAME_ERROR,
                    MettaValueInner::Unit => TYPE_NAME_UNIT,
                }
            }
        }
        _ => TYPE_NAME_UNKNOWN,
    };

    // Create the type name atom using the factory
    let atom = factory.atom(type_name);
    value_to_jit_generic(&atom, mode).to_bits()
}

// =============================================================================
// Internal Helpers
// =============================================================================

/// Internal helper: Get the type name as a string slice (not exported)
unsafe fn get_type_name(val: u64) -> &'static str {
    let tag = val & TAG_MASK;

    match tag {
        TAG_LONG => TYPE_NAME_NUMBER,
        TAG_BOOL => TYPE_NAME_BOOL,
        TAG_NIL => TYPE_NAME_NIL,
        TAG_UNIT => TYPE_NAME_UNIT,
        TAG_ERROR => TYPE_NAME_ERROR,
        TAG_VAR => TYPE_NAME_VARIABLE,
        TAG_ATOM => {
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
        TAG_HEAP => {
            let ptr = (val & PAYLOAD_MASK) as *const MettaValue;
            if ptr.is_null() {
                return TYPE_NAME_UNKNOWN;
            }
            match (*ptr).inner() {
                MettaValueInner::SExpr(_) => TYPE_NAME_EXPRESSION,
                MettaValueInner::String(_) => TYPE_NAME_STRING,
                MettaValueInner::Type(_) => TYPE_NAME_TYPE,
                MettaValueInner::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                MettaValueInner::Space(_) => TYPE_NAME_SPACE,
                MettaValueInner::State(_) => TYPE_NAME_STATE,
                MettaValueInner::Memo(_) => TYPE_NAME_MEMO,
                MettaValueInner::Empty => TYPE_NAME_EMPTY,
                MettaValueInner::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                MettaValueInner::Atom(_) => TYPE_NAME_SYMBOL,
                MettaValueInner::Bool(_) => TYPE_NAME_BOOL,
                MettaValueInner::Long(_) | MettaValueInner::Float(_) => TYPE_NAME_NUMBER,
                MettaValueInner::Nil => TYPE_NAME_NIL,
                MettaValueInner::Error(_, _) => TYPE_NAME_ERROR,
                MettaValueInner::Unit => TYPE_NAME_UNIT,
            }
        }
        _ => TYPE_NAME_UNKNOWN,
    }
}

