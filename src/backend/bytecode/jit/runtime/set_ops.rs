//! Set operations and alpha-equivalence runtime functions for JIT compilation
//!
//! Provides FFI-callable runtime helpers for:
//! - eval_if_equal: alpha-equivalence conditional
//! - unique_atom: deduplicate list by alpha-equivalence
//! - union_atom: concatenate two lists
//! - intersection_atom: multiset intersection
//! - subtraction_atom: multiset subtraction

use super::helpers::metta_to_jit;
use crate::backend::bytecode::jit::types::{JitContext, JitValue};
use crate::backend::eval::alpha_equiv;
use crate::backend::models::{MettaValue, MettaValueInner};

/// eval_if_equal: alpha-equivalence conditional
///
/// Stack: [pred1, pred2, then_val, else_val] -> [result]
/// Uses alpha-equivalence to compare pred1 and pred2 (matching MeTTa HE semantics).
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_if_equal(
    _ctx: *mut JitContext,
    pred1: u64,
    pred2: u64,
    then_val: u64,
    else_val: u64,
    _ip: u64,
) -> u64 {
    let p1 = JitValue::from_raw(pred1).to_metta();
    let p2 = JitValue::from_raw(pred2).to_metta();

    if alpha_equiv::atoms_are_alpha_equivalent(&p1, &p2) {
        then_val
    } else {
        else_val
    }
}

/// unique_atom: deduplicate list by alpha-equivalence
///
/// Stack: [list] -> [deduped_list]
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_unique_atom(
    _ctx: *mut JitContext,
    list: u64,
    _ip: u64,
) -> u64 {
    let jit_list = JitValue::from_raw(list);
    let metta_list = jit_list.to_metta();

    // Handle Unit as empty list
    let items = match metta_list.inner() {
        MettaValueInner::SExpr(items) => *items,
        MettaValueInner::Unit => return list, // Unit is already deduplicated
        _ => return list, // Not a list, return as-is
    };

    // O(n²) alpha-equivalence dedup (matches MeTTa HE)
    let mut unique: Vec<MettaValue> = Vec::with_capacity(items.len());
    for item in items {
        let already_seen = unique
            .iter()
            .any(|seen| alpha_equiv::atoms_are_alpha_equivalent(seen, item));
        if !already_seen {
            unique.push(*item);
        }
    }

    let result = MettaValue::SExpr(unique);
    metta_to_jit(&result).to_bits()
}

/// union_atom: concatenate two lists
///
/// Stack: [left, right] -> [combined]
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_union_atom(
    _ctx: *mut JitContext,
    left: u64,
    right: u64,
    _ip: u64,
) -> u64 {
    let left_val = JitValue::from_raw(left).to_metta();
    let right_val = JitValue::from_raw(right).to_metta();

    // Handle Unit as empty list
    let empty: &[MettaValue] = &[];
    let left_items = match left_val.inner() {
        MettaValueInner::SExpr(items) => &**items,
        MettaValueInner::Unit => empty,
        _ => return JitValue::unit().to_bits(),
    };
    let right_items = match right_val.inner() {
        MettaValueInner::SExpr(items) => &**items,
        MettaValueInner::Unit => empty,
        _ => return JitValue::unit().to_bits(),
    };

    let mut combined = Vec::with_capacity(left_items.len() + right_items.len());
    combined.extend(left_items.iter().copied());
    combined.extend(right_items.iter().copied());

    let result = MettaValue::SExpr(combined);
    metta_to_jit(&result).to_bits()
}

/// intersection_atom: multiset intersection
///
/// Stack: [left, right] -> [intersection]
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_intersection_atom(
    _ctx: *mut JitContext,
    left: u64,
    right: u64,
    _ip: u64,
) -> u64 {
    let left_val = JitValue::from_raw(left).to_metta();
    let right_val = JitValue::from_raw(right).to_metta();

    // Handle Unit as empty list
    let empty: &[MettaValue] = &[];
    let left_items = match left_val.inner() {
        MettaValueInner::SExpr(items) => &**items,
        MettaValueInner::Unit => empty,
        _ => return JitValue::unit().to_bits(),
    };
    let right_items = match right_val.inner() {
        MettaValueInner::SExpr(items) => &**items,
        MettaValueInner::Unit => empty,
        _ => return JitValue::unit().to_bits(),
    };

    // Build count map from right (structural equality)
    let mut right_remaining: Vec<(MettaValue, usize)> = Vec::new();
    for item in right_items {
        let mut found = false;
        for entry in right_remaining.iter_mut() {
            if entry.0 == *item {
                entry.1 += 1;
                found = true;
                break;
            }
        }
        if !found {
            right_remaining.push((*item, 1));
        }
    }

    // Iterate left, emit if found in right
    let mut result = Vec::new();
    for item in left_items {
        for entry in right_remaining.iter_mut() {
            if entry.0 == *item && entry.1 > 0 {
                entry.1 -= 1;
                result.push(*item);
                break;
            }
        }
    }

    let result = MettaValue::SExpr(result);
    metta_to_jit(&result).to_bits()
}

/// subtraction_atom: multiset subtraction
///
/// Stack: [left, right] -> [difference]
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_subtraction_atom(
    _ctx: *mut JitContext,
    left: u64,
    right: u64,
    _ip: u64,
) -> u64 {
    let left_val = JitValue::from_raw(left).to_metta();
    let right_val = JitValue::from_raw(right).to_metta();

    // Handle Unit as empty list
    let empty: &[MettaValue] = &[];
    let left_items = match left_val.inner() {
        MettaValueInner::SExpr(items) => &**items,
        MettaValueInner::Unit => empty,
        _ => return JitValue::unit().to_bits(),
    };
    let right_items = match right_val.inner() {
        MettaValueInner::SExpr(items) => &**items,
        MettaValueInner::Unit => empty,
        _ => return JitValue::unit().to_bits(),
    };

    // Build count map from right (structural equality)
    let mut right_remaining: Vec<(MettaValue, usize)> = Vec::new();
    for item in right_items {
        let mut found = false;
        for entry in right_remaining.iter_mut() {
            if entry.0 == *item {
                entry.1 += 1;
                found = true;
                break;
            }
        }
        if !found {
            right_remaining.push((*item, 1));
        }
    }

    // Iterate left, skip items found in right
    let mut result = Vec::new();
    for item in left_items {
        let mut subtracted = false;
        for entry in right_remaining.iter_mut() {
            if entry.0 == *item && entry.1 > 0 {
                entry.1 -= 1;
                subtracted = true;
                break;
            }
        }
        if !subtracted {
            result.push(*item);
        }
    }

    let result = MettaValue::SExpr(result);
    metta_to_jit(&result).to_bits()
}
