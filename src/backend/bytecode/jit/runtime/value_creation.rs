//! Value creation runtime functions for JIT compilation
//!
//! This module provides FFI-callable value creation operations:
//! - make_sexpr - Create an S-expression from array of values
//! - cons_atom - Prepend a value to an S-expression
//! - push_uri - Load a URI from the constant pool
//! - make_list - Create a proper Cons-based list
//! - make_quote - Wrap a value in a quote expression
//!
//! ## Generic Variants (Zero-Conversion Support)
//!
//! Generic versions using `MettaValueFactory` are provided for each function
//! to support arena-allocated values without conversion overhead:
//! - `make_sexpr_generic` - Generic S-expression creation
//! - `cons_atom_generic` - Generic cons operation
//! - `make_list_generic` - Generic list creation
//! - `make_quote_generic` - Generic quote wrapper

use super::helpers::{jit_to_value_generic, value_to_jit_generic};
use super::stack_ops::jit_runtime_load_constant;
use crate::backend::bytecode::jit::types::{JitContext, JitValue, TAG_PTR, TAG_MASK, TAG_UNIT};
use crate::backend::models::{
    MettaValue, GcFactory, MettaValueFactory, MettaValueTrait,
    SlabAllocator,
};

// =============================================================================
// Phase 2a: Value Creation Runtime (MakeSExpr, ConsAtom)
// =============================================================================

/// Create an S-expression from an array of NaN-boxed values.
///
/// Uses the slab allocator (via GcFactory) for all value creation.
///
/// # Arguments
/// * `ctx` - JIT context (provides arena/allocator pointer)
/// * `values_ptr` - Pointer to array of NaN-boxed u64 values
/// * `count` - Number of elements in the array
/// * `_ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_PTR pointer to the new S-expression
///
/// # Safety
/// * values_ptr must point to a valid array of count u64 values
/// * Each value must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_sexpr(
    ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    let arena_ptr = if !ctx.is_null() { (*ctx).arena_ptr() } else { std::ptr::null() };
    let alloc: &'static SlabAllocator = if !arena_ptr.is_null() {
        &*(arena_ptr as *const SlabAllocator)
    } else {
        crate::backend::models::global_allocator()
    };
    let factory = GcFactory::new(alloc);
    make_sexpr_generic::<MettaValue, GcFactory>(values_ptr, count as usize, &factory).to_bits()
}

/// Prepend a value to an S-expression (cons operation).
///
/// Uses the slab allocator (via GcFactory) for all value creation.
///
/// # Arguments
/// * `ctx` - JIT context (provides arena/allocator pointer)
/// * `head` - NaN-boxed value to prepend
/// * `tail` - NaN-boxed S-expression or Unit
/// * `_ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_PTR pointer to the new S-expression
///
/// # Safety
/// * head and tail must be valid NaN-boxed values
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cons_atom(
    ctx: *mut JitContext,
    head: u64,
    tail: u64,
    _ip: u64,
) -> u64 {
    let arena_ptr = if !ctx.is_null() { (*ctx).arena_ptr() } else { std::ptr::null() };
    let alloc: &'static SlabAllocator = if !arena_ptr.is_null() {
        &*(arena_ptr as *const SlabAllocator)
    } else {
        crate::backend::models::global_allocator()
    };
    let factory = GcFactory::new(alloc);
    cons_atom_generic::<MettaValue, GcFactory>(head, tail, &factory).to_bits()
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
/// Builds a linked list using the (Cons elem rest) structure.
/// Uses the slab allocator (via GcFactory) for all value creation.
///
/// # Arguments
/// * `ctx` - JIT context (provides arena/allocator pointer)
/// * `values_ptr` - Pointer to array of NaN-boxed u64 values
/// * `count` - Number of elements in the array
/// * `_ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_PTR pointer to the list (or TAG_UNIT for empty list)
///
/// # Safety
/// * values_ptr must point to a valid array of count u64 values
/// * Each value must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_list(
    ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    let arena_ptr = if !ctx.is_null() { (*ctx).arena_ptr() } else { std::ptr::null() };
    let alloc: &'static SlabAllocator = if !arena_ptr.is_null() {
        &*(arena_ptr as *const SlabAllocator)
    } else {
        crate::backend::models::global_allocator()
    };
    let factory = GcFactory::new(alloc);
    make_list_generic::<MettaValue, GcFactory>(values_ptr, count as usize, &factory).to_bits()
}

/// Wrap a value in a quote expression.
///
/// Creates (quote value) S-expression to prevent evaluation.
/// Uses the slab allocator (via GcFactory) for all value creation.
///
/// # Arguments
/// * `ctx` - JIT context (provides arena/allocator pointer)
/// * `val` - NaN-boxed value to quote
/// * `_ip` - Instruction pointer (unused, for consistency)
///
/// # Returns
/// NaN-boxed TAG_PTR pointer to the (quote value) S-expression
///
/// # Safety
/// * val must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_quote(ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let arena_ptr = if !ctx.is_null() { (*ctx).arena_ptr() } else { std::ptr::null() };
    let alloc: &'static SlabAllocator = if !arena_ptr.is_null() {
        &*(arena_ptr as *const SlabAllocator)
    } else {
        crate::backend::models::global_allocator()
    };
    let factory = GcFactory::new(alloc);
    make_quote_generic::<MettaValue, GcFactory>(val, &factory).to_bits()
}

// =============================================================================
// Generic Value Creation (Zero-Conversion Support)
// =============================================================================

/// Create an S-expression from an array of NaN-boxed values using a factory.
///
/// This is the generic version that works with any value type implementing
/// `MettaValueTrait`. It uses the provided factory to construct the S-expression.
///
/// # Type Parameters
/// - `V`: The value type (e.g., `MettaValue` or `MettaValue`)
/// - `F`: The factory type for constructing values
///
/// # Arguments
/// - `values_ptr`: Pointer to array of NaN-boxed u64 values
/// - `count`: Number of elements in the array
/// - `factory`: Factory for creating values
///
/// # Returns
/// A `JitValue` containing the new S-expression
///
/// # Safety
/// - `values_ptr` must point to a valid array of `count` NaN-boxed values
/// - Each value must be a valid NaN-boxed value
pub unsafe fn make_sexpr_generic<V, F>(
    values_ptr: *const u64,
    count: usize,
    factory: &F,
) -> JitValue
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    // Sanity check
    debug_assert!(
        count <= 1_000_000,
        "make_sexpr_generic: Suspiciously large count: {}",
        count
    );

    // Handle empty S-expression
    if count == 0 {
        let sexpr = factory.sexpr(Vec::new());
        return value_to_jit_generic(&sexpr);
    }

    // Validate values_ptr is not null
    debug_assert!(
        !values_ptr.is_null(),
        "make_sexpr_generic: Null values_ptr with count={}",
        count
    );

    // Convert each value
    let mut elements = Vec::with_capacity(count);
    for i in 0..count {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);

        debug_assert!(
            jit_val.is_valid_tag(),
            "make_sexpr_generic: Invalid JitValue at index {}: raw={:#018x}",
            i,
            raw_val
        );

        elements.push(jit_to_value_generic::<V, F>(jit_val, factory));
    }

    // Create the S-expression
    let sexpr = factory.sexpr(elements);
    value_to_jit_generic(&sexpr)
}

/// Prepend a value to an S-expression using a factory (generic cons operation).
///
/// This is the generic version of `jit_runtime_cons_atom`.
///
/// # Type Parameters
/// - `V`: The value type
/// - `F`: The factory type
///
/// # Arguments
/// - `head`: NaN-boxed value to prepend
/// - `tail`: NaN-boxed S-expression or Nil
/// - `factory`: Factory for creating values
///
/// # Returns
/// A `JitValue` containing the new S-expression, or unit on error
///
/// # Safety
/// - `head` and `tail` must be valid NaN-boxed values
pub unsafe fn cons_atom_generic<V, F>(
    head: u64,
    tail: u64,
    factory: &F,
) -> JitValue
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let head_jit = JitValue::from_raw(head);
    let tail_jit = JitValue::from_raw(tail);

    debug_assert!(head_jit.is_valid_tag(), "cons_atom_generic: Invalid head");
    debug_assert!(tail_jit.is_valid_tag(), "cons_atom_generic: Invalid tail");

    let head_val = jit_to_value_generic::<V, F>(head_jit, factory);

    let tail_tag = tail & TAG_MASK;

    // Handle Unit tail
    if tail_tag == TAG_UNIT {
        let sexpr = factory.sexpr(vec![head_val]);
        return value_to_jit_generic(&sexpr);
    }

    // Must be a heap pointer (S-expression)
    if tail_tag != TAG_PTR {
        return JitValue::unit();
    }

    // Get tail and check if it's an S-expression
    let tail_val = jit_to_value_generic::<V, F>(tail_jit, factory);

    if let Some(elements) = tail_val.as_sexpr() {
        // Prepend head to the elements
        let mut new_elements = Vec::with_capacity(elements.len() + 1);
        new_elements.push(head_val);
        new_elements.extend(elements.iter().cloned());

        let sexpr = factory.sexpr(new_elements);
        value_to_jit_generic(&sexpr)
    } else if tail_val.is_unit() {
        // Treat Unit as empty S-expression
        let sexpr = factory.sexpr(vec![head_val]);
        value_to_jit_generic(&sexpr)
    } else {
        // Type error
        JitValue::unit()
    }
}

/// Create a proper MeTTa list from an array of NaN-boxed values using a factory.
///
/// Builds a linked list using the (Cons elem rest) structure.
///
/// # Type Parameters
/// - `V`: The value type
/// - `F`: The factory type
///
/// # Arguments
/// - `values_ptr`: Pointer to array of NaN-boxed u64 values
/// - `count`: Number of elements
/// - `factory`: Factory for creating values
///
/// # Returns
/// A `JitValue` containing the list
///
/// # Safety
/// - `values_ptr` must point to a valid array
pub unsafe fn make_list_generic<V, F>(
    values_ptr: *const u64,
    count: usize,
    factory: &F,
) -> JitValue
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    debug_assert!(
        count <= 1_000_000,
        "make_list_generic: Suspiciously large count: {}",
        count
    );

    // Empty list is Unit
    if count == 0 {
        return JitValue::unit();
    }

    debug_assert!(
        !values_ptr.is_null(),
        "make_list_generic: Null values_ptr with count={}",
        count
    );

    // Build from the end (reverse order for proper Cons structure)
    let mut list = factory.unit();

    for i in (0..count).rev() {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);

        debug_assert!(
            jit_val.is_valid_tag(),
            "make_list_generic: Invalid JitValue at index {}",
            i
        );

        let elem = jit_to_value_generic::<V, F>(jit_val, factory);

        // Build (Cons elem list)
        let cons_atom = factory.atom("Cons");
        list = factory.sexpr(vec![cons_atom, elem, list]);
    }

    value_to_jit_generic(&list)
}

/// Wrap a value in a quote expression using a factory.
///
/// Creates (quote value) S-expression to prevent evaluation.
///
/// # Type Parameters
/// - `V`: The value type
/// - `F`: The factory type
///
/// # Arguments
/// - `val`: NaN-boxed value to quote
/// - `factory`: Factory for creating values
///
/// # Returns
/// A `JitValue` containing the (quote value) S-expression
///
/// # Safety
/// - `val` must be a valid NaN-boxed value
pub unsafe fn make_quote_generic<V, F>(val: u64, factory: &F) -> JitValue
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let jit_val = JitValue::from_raw(val);

    debug_assert!(
        jit_val.is_valid_tag(),
        "make_quote_generic: Invalid JitValue: raw={:#018x}",
        val
    );

    let inner = jit_to_value_generic::<V, F>(jit_val, factory);

    // Create (quote value)
    let quote_atom = factory.atom("quote");
    let quoted = factory.sexpr(vec![quote_atom, inner]);

    value_to_jit_generic(&quoted)
}

