//! Pattern matching runtime functions for JIT compilation
//!
//! This module provides FFI-callable pattern matching operations:
//! - pattern_match - Pattern match without binding
//! - pattern_match_bind - Pattern match with variable binding
//! - match_arity - Check S-expression arity
//! - match_head - Check S-expression head symbol
//! - unify - Bidirectional unification
//! - unify_bind - Bidirectional unification with binding

use super::bindings::jit_runtime_store_binding;
use super::helpers::metta_to_jit;
use crate::backend::bytecode::jit::types::{
    JitContext, JitValue, PAYLOAD_MASK, TAG_ATOM, TAG_BOOL, TAG_LONG, TAG_NIL, TAG_UNIT,
    VAR_INDEX_CACHE_SIZE,
};
use crate::backend::models::MettaValue;

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
        let pattern_ptr = (pattern & PAYLOAD_MASK) as *const String;
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
    if pattern_tag == TAG_LONG && value_tag == TAG_LONG {
        // Compare raw payloads directly (both are 48-bit signed integers)
        return Some((pattern & PAYLOAD_MASK) == (value & PAYLOAD_MASK));
    }

    // Bool comparison
    if pattern_tag == TAG_BOOL && value_tag == TAG_BOOL {
        return Some((pattern & 1) == (value & 1));
    }

    // Nil comparison
    if pattern_tag == TAG_NIL && value_tag == TAG_NIL {
        return Some(true);
    }

    // Unit comparison
    if pattern_tag == TAG_UNIT && value_tag == TAG_UNIT {
        return Some(true);
    }

    // Atom comparison (interned string pointer equality)
    if pattern_tag == TAG_ATOM && value_tag == TAG_ATOM {
        // Same pointer means same interned string
        let pattern_ptr = pattern & PAYLOAD_MASK;
        let value_ptr = value & PAYLOAD_MASK;
        if pattern_ptr == value_ptr {
            return Some(true);
        }
        // Different pointers - need to compare string contents
        // Fall back to full implementation for safety
        // (interning should make this rare)
    }

    // Fast path 4: Type mismatch for ground values = no match
    // If pattern is a ground type and value is a different ground type, no match
    let pattern_is_ground = matches!(pattern_tag, TAG_LONG | TAG_BOOL | TAG_NIL | TAG_UNIT);
    let value_is_ground = matches!(value_tag, TAG_LONG | TAG_BOOL | TAG_NIL | TAG_UNIT);

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
        None => return TAG_BOOL, // false
    };

    // Optimization 3.1: Try fast path first
    if let Some(result) = try_pattern_match_fast_path(pattern, value) {
        return if result {
            TAG_BOOL | 1 // true
        } else {
            TAG_BOOL // false
        };
    }

    // Fall back to full implementation for complex patterns
    let pattern_val = JitValue::from_raw(pattern).to_metta();
    let value_val = JitValue::from_raw(value).to_metta();

    let matches = pattern_matches_impl(&pattern_val, &value_val);

    if matches {
        TAG_BOOL | 1 // true
    } else {
        TAG_BOOL // false
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
pub(crate) unsafe fn lookup_var_index_cached(
    ctx: *mut JitContext,
    name: &str,
    constants: &[MettaValue],
) -> Option<usize> {
    let name_hash = hash_var_name(name);
    let cache_slot = (name_hash as usize) % VAR_INDEX_CACHE_SIZE;

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
    let name_idx = constants
        .iter()
        .position(|c| matches!(c, MettaValue::Atom(s) if s == name));

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
        let var_ptr = (pattern & PAYLOAD_MASK) as *const String;
        if !var_ptr.is_null() {
            let var_name = &*var_ptr;

            // Find the variable name in constants to get its index (Optimization 5.3: cached lookup)
            let ctx_ref = ctx.as_ref()?;
            if ctx_ref.constants_len > 0 && !ctx_ref.constants.is_null() {
                let constants =
                    std::slice::from_raw_parts(ctx_ref.constants, ctx_ref.constants_len);
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
        let pattern_ptr = (pattern & PAYLOAD_MASK) as *const String;
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
                    let constants =
                        std::slice::from_raw_parts(ctx_ref.constants, ctx_ref.constants_len);
                    let name_idx = lookup_var_index_cached(ctx, pattern_str, constants);

                    if let Some(idx) = name_idx {
                        let store_result = jit_runtime_store_binding(ctx, idx as u64, value, 0);
                        return Some(store_result == 0);
                    }
                }
                return None; // Fall back
            }

            // Regular atom - compare with value
            if value_tag == TAG_ATOM {
                let value_ptr = (value & PAYLOAD_MASK) as *const String;
                if !value_ptr.is_null() {
                    let value_str = &*value_ptr;
                    return Some(pattern_str == value_str);
                }
            }
        }
    }

    // Fast path 3: Ground value comparison (no binding needed)
    // Long comparison
    if pattern_tag == TAG_LONG && value_tag == TAG_LONG {
        return Some((pattern & PAYLOAD_MASK) == (value & PAYLOAD_MASK));
    }

    // Bool comparison
    if pattern_tag == TAG_BOOL && value_tag == TAG_BOOL {
        return Some((pattern & 1) == (value & 1));
    }

    // Nil comparison
    if pattern_tag == TAG_NIL && value_tag == TAG_NIL {
        return Some(true);
    }

    // Unit comparison
    if pattern_tag == TAG_UNIT && value_tag == TAG_UNIT {
        return Some(true);
    }

    // Fast path 4: Type mismatch for ground values = no match
    let pattern_is_ground = matches!(pattern_tag, TAG_LONG | TAG_BOOL | TAG_NIL | TAG_UNIT);
    let value_is_ground = matches!(value_tag, TAG_LONG | TAG_BOOL | TAG_NIL | TAG_UNIT);

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
        None => return TAG_BOOL, // false
    };

    // Optimization 3.1: Try fast path first
    if let Some(result) = try_pattern_match_bind_fast_path(ctx, pattern, value) {
        return if result {
            TAG_BOOL | 1 // true
        } else {
            TAG_BOOL // false
        };
    }

    // Fall back to full implementation for complex patterns
    let pattern_val = JitValue::from_raw(pattern).to_metta();
    let value_val = JitValue::from_raw(value).to_metta();

    let mut bindings = Vec::new();
    if !pattern_match_bind_impl(&pattern_val, &value_val, &mut bindings) {
        return TAG_BOOL; // false
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
                    return TAG_BOOL; // false - binding failed
                }
            }
            // If name not in constants, we skip binding (this is a limitation)
        }
    }

    TAG_BOOL | 1 // true
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
        MettaValue::SExpr(items) => items.len() == expected_arity as usize,
        _ => false,
    };

    if matches {
        TAG_BOOL | 1 // true
    } else {
        TAG_BOOL // false
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
        None => return TAG_BOOL, // false
    };

    let val = JitValue::from_raw(value).to_metta();

    // Get the expected head from the constant pool
    let expected_head =
        if !ctx_ref.constants.is_null() && expected_head_idx < ctx_ref.constants_len as u64 {
            &*ctx_ref.constants.add(expected_head_idx as usize)
        } else {
            return TAG_BOOL; // false - invalid index
        };

    let matches = match val {
        MettaValue::SExpr(items) if !items.is_empty() => &items[0] == expected_head,
        _ => false,
    };

    if matches {
        TAG_BOOL | 1 // true
    } else {
        TAG_BOOL // false
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
        TAG_BOOL | 1 // true
    } else {
        TAG_BOOL // false
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
        None => return TAG_BOOL, // false
    };

    let a_val = JitValue::from_raw(a).to_metta();
    let b_val = JitValue::from_raw(b).to_metta();

    let mut bindings = Vec::new();
    if !unify_impl(&a_val, &b_val, &mut bindings) {
        return TAG_BOOL; // false
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
                    return TAG_BOOL; // false - binding failed
                }
            }
        }
    }

    TAG_BOOL | 1 // true
}

// =============================================================================
// Pattern Matching Helper Functions
// =============================================================================

/// Pattern match implementation (without binding)
pub(crate) fn pattern_matches_impl(pattern: &MettaValue, value: &MettaValue) -> bool {
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
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Vec<(String, MettaValue)>,
) -> bool {
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
fn unify_impl(a: &MettaValue, b: &MettaValue, bindings: &mut Vec<(String, MettaValue)>) -> bool {
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
