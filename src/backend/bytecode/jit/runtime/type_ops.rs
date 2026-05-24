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
    JitBailoutReason, JitContext, PAYLOAD_MASK, TAG_ATOM, TAG_BOOL, TAG_ERROR, TAG_LONG, TAG_MASK,
    TAG_PTR, TAG_UNIT, TAG_VAR,
};
use crate::backend::models::{
    GcFactory, MettaValue, MettaValueFactory, MettaValueInner, MettaValueTrait, SlabAllocator,
};

use super::helpers::value_to_jit_generic;

// =============================================================================
// Type Operations Runtime (Phase 1 JIT)
// =============================================================================

// Static type name strings for efficient atom creation
// These are leaked to get 'static lifetimes that survive JIT code
static TYPE_NAME_NUMBER: &str = "Number";
static TYPE_NAME_BOOL: &str = "Bool";
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
/// Plan S0a (2026-05-13) — HE `NotReducible` sentinel type name.
static TYPE_NAME_NOT_REDUCIBLE: &str = "NotReducible";
static TYPE_NAME_UNKNOWN: &str = "Unknown";

/// Get the type name of a NaN-boxed value.
///
/// S6 (RC-GET-TYPE-CONSULTS-ENV): HE parity. The dispatch order is:
/// 1. If env is available, delegate to `infer_types_generic` — the shared
///    helper that handles typed-primitives, atom symbol → space `(: name $T)`
///    lookups, SExpr arrow-return types, and the `%Undefined%` fallback.
/// 2. If no env, fall back to syntactic `get_type_generic` (legacy semantics
///    for the no-environment case used by some tests).
///
/// For nondeterministic results (multiple `(: name $T)` assertions), the
/// JIT FFI must return a single scalar, so we pick the first result. The
/// T0 trampoline path handles full nondet enumeration via `eval_get_type_generic`.
///
/// Type names match MettaValue::type_name() in the syntactic fallback:
/// - TAG_LONG → "Number"
/// - TAG_BOOL → "Bool"
/// - TAG_UNIT → "Unit"
/// - TAG_PTR → depends on heap value type
/// - TAG_ERROR → "Error"
/// - TAG_ATOM → "Symbol" (or "Variable" if starts with $)
/// - TAG_VAR → "Variable"
///
/// Uses the slab allocator (via GcFactory) for all value creation.
///
/// # Safety
/// For pointer payloads, the referenced value must be valid.
/// If `ctx->env_ptr` is set, it must point to a valid `MettaEnvironment`
/// that outlives this call.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_type(ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let arena_ptr = if !ctx.is_null() {
        (*ctx).arena_ptr()
    } else {
        std::ptr::null()
    };
    let alloc: &'static SlabAllocator = if !arena_ptr.is_null() {
        &*(arena_ptr as *const SlabAllocator)
    } else {
        crate::backend::models::global_allocator()
    };
    let factory = GcFactory::new(alloc);

    // S6: consult environment for type assertions if available.
    if !ctx.is_null() {
        let env_ptr = (*ctx).env_ptr;
        if !env_ptr.is_null() {
            use super::helpers::jit_to_value_generic;
            use crate::backend::bytecode::jit::types::JitValue;
            use crate::backend::eval::types::{infer_types_generic, SkipInferredGuard};
            let env = &*(env_ptr as *const crate::backend::bytecode::MettaEnvironment);
            // Reconstruct MettaValue from NaN-boxed payload.
            let value: MettaValue =
                jit_to_value_generic::<MettaValue, GcFactory>(JitValue::from_raw(val), &factory);
            // Plan Phase F (2026-05-20): `get-type` consults declared
            // types only across all tiers (T0/T1/T2/T3). The MTT-only
            // `get-deep-type` op bypasses this guard.
            let _skip_guard = SkipInferredGuard::enter();
            let types = infer_types_generic(&value, &factory, env);
            // HE parity: empty result → %Undefined%. Single representative
            // for the JIT FFI; full nondet enumeration handled at T0.
            let result = if types.is_empty() {
                factory.atom("%Undefined%")
            } else {
                types[0].clone()
            };
            return value_to_jit_generic(&result).to_bits();
        }
    }

    // Fallback: no env attached — use the legacy syntactic helper.
    get_type_generic::<MettaValue, GcFactory>(val, &factory)
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
            TAG_PTR => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const MettaValueInner;
                if !ptr.is_null() {
                    if let MettaValueInner::Atom(s) = &*ptr {
                        Some(*s)
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
            TAG_PTR => {
                let ptr = (type_atom & PAYLOAD_MASK) as *const MettaValueInner;
                if !ptr.is_null() {
                    if let MettaValueInner::Atom(s) = &*ptr {
                        Some(*s)
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
pub unsafe fn get_type_generic<V, F>(val: u64, factory: &F) -> u64
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let tag = val & TAG_MASK;

    let type_name: &'static str = match tag {
        TAG_LONG => TYPE_NAME_NUMBER,
        TAG_BOOL => TYPE_NAME_BOOL,
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
        TAG_PTR => {
            // For generic, we need to use MettaValueTrait
            // Since we can't know the concrete type at compile time for the pointer,
            // we fall back to checking if it's a MettaValue pointer
            let ptr = (val & PAYLOAD_MASK) as *const MettaValueInner;
            if ptr.is_null() {
                TYPE_NAME_UNKNOWN
            } else {
                match &*ptr {
                    MettaValueInner::SExpr(_) => TYPE_NAME_EXPRESSION,
                    MettaValueInner::String(_) => TYPE_NAME_STRING,
                    MettaValueInner::Type(_) => TYPE_NAME_TYPE,
                    MettaValueInner::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                    MettaValueInner::Space(_) => TYPE_NAME_SPACE,
                    MettaValueInner::State(_) => TYPE_NAME_STATE,
                    MettaValueInner::Memo(_) => TYPE_NAME_MEMO,
                    MettaValueInner::Empty => TYPE_NAME_EMPTY,
                    MettaValueInner::NotReducible => TYPE_NAME_NOT_REDUCIBLE,
                    MettaValueInner::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                    MettaValueInner::Atom(_) => TYPE_NAME_SYMBOL,
                    MettaValueInner::Bool(_) => TYPE_NAME_BOOL,
                    MettaValueInner::Long(_) | MettaValueInner::Float(_) => TYPE_NAME_NUMBER,
                    MettaValueInner::Unit => TYPE_NAME_UNIT,
                    MettaValueInner::Error(_, _) => TYPE_NAME_ERROR,
                    // Quoted is transparent to get-metatype — it appears as "Expression"
                    MettaValueInner::Quoted(_) => TYPE_NAME_EXPRESSION,
                    // Lazy is INVISIBLE — delegate to inner MettaValue's type_name().
                    MettaValueInner::Lazy(v) => v.type_name(),
                    // Spanned: strip span and inspect inner value
                    MettaValueInner::Spanned(inner, _) => {
                        // Recurse through inner — use .inner_ref() field (raw access)
                        match inner.inner_ref() {
                            MettaValueInner::SExpr(_) => TYPE_NAME_EXPRESSION,
                            MettaValueInner::String(_) => TYPE_NAME_STRING,
                            MettaValueInner::Type(_) => TYPE_NAME_TYPE,
                            MettaValueInner::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                            MettaValueInner::Space(_) => TYPE_NAME_SPACE,
                            MettaValueInner::State(_) => TYPE_NAME_STATE,
                            MettaValueInner::Memo(_) => TYPE_NAME_MEMO,
                            MettaValueInner::Empty => TYPE_NAME_EMPTY,
                            MettaValueInner::NotReducible => TYPE_NAME_NOT_REDUCIBLE,
                            MettaValueInner::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                            MettaValueInner::Atom(_) => TYPE_NAME_SYMBOL,
                            MettaValueInner::Bool(_) => TYPE_NAME_BOOL,
                            MettaValueInner::Long(_) | MettaValueInner::Float(_) => {
                                TYPE_NAME_NUMBER
                            }
                            MettaValueInner::Unit => TYPE_NAME_UNIT,
                            MettaValueInner::Error(_, _) => TYPE_NAME_ERROR,
                            MettaValueInner::Quoted(_) => TYPE_NAME_EXPRESSION,
                            // Lazy is INVISIBLE — delegate to inner MettaValue's type_name().
                            MettaValueInner::Lazy(v) => v.type_name(),
                            // Nested Spanned: delegate to the inner MettaValue's type_name()
                            MettaValueInner::Spanned(v, _) => v.type_name(),
                        }
                    }
                }
            }
        }
        _ => TYPE_NAME_UNKNOWN,
    };

    // Create the type name atom using the factory
    let atom = factory.atom(type_name);
    value_to_jit_generic(&atom).to_bits()
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
        TAG_PTR => {
            // TAG_PTR payload is *const MettaValueInner (slab-allocated)
            let ptr = (val & PAYLOAD_MASK) as *const MettaValueInner;
            if ptr.is_null() {
                return TYPE_NAME_UNKNOWN;
            }
            match &*ptr {
                MettaValueInner::SExpr(_) => TYPE_NAME_EXPRESSION,
                MettaValueInner::String(_) => TYPE_NAME_STRING,
                MettaValueInner::Type(_) => TYPE_NAME_TYPE,
                MettaValueInner::Conjunction(_) => TYPE_NAME_CONJUNCTION,
                MettaValueInner::Space(_) => TYPE_NAME_SPACE,
                MettaValueInner::State(_) => TYPE_NAME_STATE,
                MettaValueInner::Memo(_) => TYPE_NAME_MEMO,
                MettaValueInner::Empty => TYPE_NAME_EMPTY,
                MettaValueInner::NotReducible => TYPE_NAME_NOT_REDUCIBLE,
                MettaValueInner::Atom(s) if s.starts_with('$') => TYPE_NAME_VARIABLE,
                MettaValueInner::Atom(_) => TYPE_NAME_SYMBOL,
                MettaValueInner::Bool(_) => TYPE_NAME_BOOL,
                MettaValueInner::Long(_) | MettaValueInner::Float(_) => TYPE_NAME_NUMBER,
                MettaValueInner::Unit => TYPE_NAME_UNIT,
                MettaValueInner::Error(_, _) => TYPE_NAME_ERROR,
                // Quoted is transparent to get-metatype — it appears as "Expression"
                MettaValueInner::Quoted(_) => TYPE_NAME_EXPRESSION,
                // Lazy is INVISIBLE — delegate to inner MettaValue's type_name().
                MettaValueInner::Lazy(v) => v.type_name(),
                // Spanned: delegate to the inner MettaValue's type_name()
                MettaValueInner::Spanned(v, _) => v.type_name(),
            }
        }
        _ => TYPE_NAME_UNKNOWN,
    }
}

// =============================================================================
// is-function Runtime (Phase G)
// =============================================================================

/// Check if a value is an arrow type (S-expression starting with "->").
/// Returns TAG_BOOL | 1 (true) or TAG_BOOL | 0 (false).
///
/// # Safety
/// `val` must be a valid NaN-boxed JIT value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_is_function(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let tag = val & TAG_MASK;
    let is_fn = if tag == TAG_PTR {
        let ptr = (val & PAYLOAD_MASK) as *const MettaValueInner;
        if !ptr.is_null() {
            match &*ptr {
                MettaValueInner::SExpr(items) => {
                    items.first().and_then(|v| v.as_atom()) == Some("->")
                }
                MettaValueInner::Spanned(inner, _) => match &inner.inner_ref() {
                    MettaValueInner::SExpr(items) => {
                        items.first().and_then(|v| v.as_atom()) == Some("->")
                    }
                    _ => false,
                },
                _ => false,
            }
        } else {
            false
        }
    } else {
        false
    };
    TAG_BOOL | (is_fn as u64)
}
