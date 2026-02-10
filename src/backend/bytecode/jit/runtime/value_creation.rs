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
use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, JitValueMode, PAYLOAD_MASK, TAG_HEAP, TAG_MASK, TAG_UNIT,
};
use crate::backend::models::{
    ArenaValue, ArenaValueFactory, MettaValue, MettaValueFactory, MettaValueInner, MettaValueTrait,
};
use bumpalo::Bump;

// =============================================================================
// Phase 2a: Value Creation Runtime (MakeSExpr, ConsAtom)
// =============================================================================

/// Create an S-expression from an array of NaN-boxed values.
///
/// This function takes a pointer to an array of NaN-boxed values (u64) and
/// creates an S-expression. It dispatches at runtime based on `ctx.value_mode`:
/// - Heap mode: Creates a `MettaValue::SExpr`
/// - Arena mode: Uses `ArenaValueFactory` for arena allocation
///
/// # Arguments
/// * `ctx` - JIT context (for error handling and mode dispatch)
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
/// * For arena mode, ctx.arena must be a valid arena pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_sexpr(
    ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    // Check context for mode dispatch
    if !ctx.is_null() {
        let ctx_ref = &*ctx;
        if ctx_ref.is_arena_mode() {
            // Arena mode: delegate to generic implementation
            let arena_ptr = ctx_ref.arena_ptr();
            debug_assert!(
                !arena_ptr.is_null(),
                "jit_runtime_make_sexpr: Arena mode requires arena pointer"
            );
            let arena: &'static Bump = &*(arena_ptr as *const Bump);
            let factory = ArenaValueFactory::new(arena);
            return make_sexpr_generic::<ArenaValue<'static>, ArenaValueFactory<'static>>(
                values_ptr,
                count as usize,
                &factory,
                JitValueMode::Arena,
            )
            .to_bits();
        }
    }

    // Heap mode: original implementation
    let count = count as usize;

    // Sanity check: count should be reasonable (prevent garbage allocation size)
    debug_assert!(
        count <= 1_000_000,
        "jit_runtime_make_sexpr: Suspiciously large count: {} (raw: {:#x})",
        count,
        count
    );

    // Handle empty S-expression
    if count == 0 {
        let sexpr = Box::new(MettaValue::SExpr(Vec::new()));
        let ptr = Box::into_raw(sexpr);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // Validate values_ptr is not null
    debug_assert!(
        !values_ptr.is_null(),
        "jit_runtime_make_sexpr: Null values_ptr with count={}",
        count
    );

    // Convert each value to MettaValue
    let mut elements = Vec::with_capacity(count);
    for i in 0..count {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);

        // Validate each value before conversion
        debug_assert!(
            jit_val.is_valid_tag(),
            "jit_runtime_make_sexpr: Invalid JitValue at index {}: raw={:#018x}, tag={:#06x}",
            i,
            raw_val,
            (raw_val >> 48) as u16
        );

        elements.push(jit_val.to_metta());
    }

    // Create the S-expression and return as heap pointer
    let sexpr = Box::new(MettaValue::SExpr(elements));
    let ptr = Box::into_raw(sexpr);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Prepend a value to an S-expression (cons operation).
///
/// This function implements the cons-atom operation:
/// - If tail is an S-expression, prepend head to it
/// - If tail is Nil, create a single-element S-expression
/// - Otherwise, signal a type error
///
/// Dispatches at runtime based on `ctx.value_mode`:
/// - Heap mode: Creates a `MettaValue::SExpr`
/// - Arena mode: Uses `ArenaValueFactory` for arena allocation
///
/// # Arguments
/// * `ctx` - JIT context (for error handling and mode dispatch)
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
/// * For arena mode, ctx.arena must be a valid arena pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cons_atom(
    ctx: *mut JitContext,
    head: u64,
    tail: u64,
    ip: u64,
) -> u64 {
    // Check context for mode dispatch
    if !ctx.is_null() {
        let ctx_ref = &*ctx;
        if ctx_ref.is_arena_mode() {
            // Arena mode: delegate to generic implementation
            let arena_ptr = ctx_ref.arena_ptr();
            debug_assert!(
                !arena_ptr.is_null(),
                "jit_runtime_cons_atom: Arena mode requires arena pointer"
            );
            let arena: &'static Bump = &*(arena_ptr as *const Bump);
            let factory = ArenaValueFactory::new(arena);
            return cons_atom_generic::<ArenaValue<'static>, ArenaValueFactory<'static>>(
                head,
                tail,
                &factory,
                JitValueMode::Arena,
            )
            .to_bits();
        }
    }

    // Heap mode: original implementation
    let head_val = JitValue::from_raw(head);
    let tail_val = JitValue::from_raw(tail);

    // Validate both values have valid tags
    debug_assert!(
        head_val.is_valid_tag(),
        "jit_runtime_cons_atom: Invalid head JitValue: raw={:#018x}, tag={:#06x}",
        head,
        (head >> 48) as u16
    );
    debug_assert!(
        tail_val.is_valid_tag(),
        "jit_runtime_cons_atom: Invalid tail JitValue: raw={:#018x}, tag={:#06x}",
        tail,
        (tail >> 48) as u16
    );

    let head_metta = head_val.to_metta();

    let tail_tag = tail & TAG_MASK;

    // Handle Unit tail
    if tail_tag == TAG_UNIT {
        let sexpr = Box::new(MettaValue::SExpr(vec![head_metta]));
        let ptr = Box::into_raw(sexpr);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // Must be a heap pointer (S-expression)
    if tail_tag != TAG_HEAP {
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        return TAG_UNIT;
    }

    // Get the tail as MettaValue
    let tail_ptr = (tail & PAYLOAD_MASK) as *const MettaValue;
    if tail_ptr.is_null() {
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        return TAG_UNIT;
    }

    // Check if tail is an S-expression
    match (*tail_ptr).inner() {
        MettaValueInner::SExpr(elements) => {
            // Prepend head to the elements
            let mut new_elements = Vec::with_capacity(elements.len() + 1);
            new_elements.push(head_metta);
            new_elements.extend(elements.iter().cloned());

            let sexpr = Box::new(MettaValue::SExpr(new_elements));
            let ptr = Box::into_raw(sexpr);
            TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        MettaValueInner::Unit => {
            // Treat Unit as empty S-expression
            let sexpr = Box::new(MettaValue::SExpr(vec![head_metta]));
            let ptr = Box::into_raw(sexpr);
            TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        _ => {
            // Type error: tail is not an S-expression or Nil
            if let Some(ctx) = ctx.as_mut() {
                ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
            }
            TAG_UNIT
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
/// - Elements are popped in order and reversed to build (Cons elem (Cons ... Unit))
/// - Empty list is just Unit
///
/// For example, with values [1, 2, 3], creates:
/// (Cons 1 (Cons 2 (Cons 3 Nil)))
///
/// Dispatches at runtime based on `ctx.value_mode`:
/// - Heap mode: Creates a `MettaValue`-based list
/// - Arena mode: Uses `ArenaValueFactory` for arena allocation
///
/// # Arguments
/// * `ctx` - JIT context (for error handling and mode dispatch)
/// * `values_ptr` - Pointer to array of NaN-boxed u64 values
/// * `count` - Number of elements in the array
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the list (or TAG_UNIT for empty list)
///
/// # Safety
/// * The context pointer must be valid
/// * values_ptr must point to a valid array of count u64 values
/// * Each value must be a valid NaN-boxed value
/// * For arena mode, ctx.arena must be a valid arena pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_list(
    ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    // Check context for mode dispatch
    if !ctx.is_null() {
        let ctx_ref = &*ctx;
        if ctx_ref.is_arena_mode() {
            // Arena mode: delegate to generic implementation
            let arena_ptr = ctx_ref.arena_ptr();
            debug_assert!(
                !arena_ptr.is_null(),
                "jit_runtime_make_list: Arena mode requires arena pointer"
            );
            let arena: &'static Bump = &*(arena_ptr as *const Bump);
            let factory = ArenaValueFactory::new(arena);
            return make_list_generic::<ArenaValue<'static>, ArenaValueFactory<'static>>(
                values_ptr,
                count as usize,
                &factory,
                JitValueMode::Arena,
            )
            .to_bits();
        }
    }

    // Heap mode: original implementation
    let count = count as usize;

    // Sanity check: count should be reasonable
    debug_assert!(
        count <= 1_000_000,
        "jit_runtime_make_list: Suspiciously large count: {} (raw: {:#x})",
        count,
        count
    );

    // Empty list is Unit
    if count == 0 {
        return TAG_UNIT;
    }

    // Validate values_ptr is not null
    debug_assert!(
        !values_ptr.is_null(),
        "jit_runtime_make_list: Null values_ptr with count={}",
        count
    );

    // Build the list from the end (reverse order to get proper Cons structure)
    // Start with Unit, then Cons each element from the end
    let mut list = MettaValue::Unit();

    for i in (0..count).rev() {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);

        // Validate each value before conversion
        debug_assert!(
            jit_val.is_valid_tag(),
            "jit_runtime_make_list: Invalid JitValue at index {}: raw={:#018x}, tag={:#06x}",
            i,
            raw_val,
            (raw_val >> 48) as u16
        );

        let elem = jit_val.to_metta();

        // Build (Cons elem list)
        list = MettaValue::SExpr(vec![MettaValue::Atom("Cons".to_string()), elem, list]);
    }

    // Return as heap pointer
    let boxed = Box::new(list);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Wrap a value in a quote expression.
///
/// Creates (quote value) S-expression to prevent evaluation.
///
/// Dispatches at runtime based on `ctx.value_mode`:
/// - Heap mode: Creates a `MettaValue::SExpr`
/// - Arena mode: Uses `ArenaValueFactory` for arena allocation
///
/// # Arguments
/// * `ctx` - JIT context (for mode dispatch)
/// * `val` - NaN-boxed value to quote
/// * `ip` - Instruction pointer (unused, for consistency)
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the (quote value) S-expression
///
/// # Safety
/// * val must be a valid NaN-boxed value
/// * For arena mode, ctx.arena must be a valid arena pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_quote(ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    // Check context for mode dispatch
    if !ctx.is_null() {
        let ctx_ref = &*ctx;
        if ctx_ref.is_arena_mode() {
            // Arena mode: delegate to generic implementation
            let arena_ptr = ctx_ref.arena_ptr();
            debug_assert!(
                !arena_ptr.is_null(),
                "jit_runtime_make_quote: Arena mode requires arena pointer"
            );
            let arena: &'static Bump = &*(arena_ptr as *const Bump);
            let factory = ArenaValueFactory::new(arena);
            return make_quote_generic::<ArenaValue<'static>, ArenaValueFactory<'static>>(
                val,
                &factory,
                JitValueMode::Arena,
            )
            .to_bits();
        }
    }

    // Heap mode: original implementation
    let jit_val = JitValue::from_raw(val);

    // Validate value has valid tag
    debug_assert!(
        jit_val.is_valid_tag(),
        "jit_runtime_make_quote: Invalid JitValue: raw={:#018x}, tag={:#06x}",
        val,
        (val >> 48) as u16
    );

    let inner = jit_val.to_metta();

    // Create (quote value)
    let quoted = MettaValue::SExpr(vec![MettaValue::Atom("quote".to_string()), inner]);

    let boxed = Box::new(quoted);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
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
/// - `V`: The value type (e.g., `MettaValue` or `ArenaValue<'static>`)
/// - `F`: The factory type for constructing values
///
/// # Arguments
/// - `values_ptr`: Pointer to array of NaN-boxed u64 values
/// - `count`: Number of elements in the array
/// - `factory`: Factory for creating values
/// - `mode`: JIT value mode (Heap or Arena)
///
/// # Returns
/// A `JitValue` containing the new S-expression
///
/// # Safety
/// - `values_ptr` must point to a valid array of `count` NaN-boxed values
/// - Each value must be a valid NaN-boxed value created in the same mode
pub unsafe fn make_sexpr_generic<V, F>(
    values_ptr: *const u64,
    count: usize,
    factory: &F,
    mode: JitValueMode,
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
        return value_to_jit_generic(&sexpr, mode);
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
    value_to_jit_generic(&sexpr, mode)
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
/// - `mode`: JIT value mode
///
/// # Returns
/// A `JitValue` containing the new S-expression, or nil on error
///
/// # Safety
/// - `head` and `tail` must be valid NaN-boxed values
pub unsafe fn cons_atom_generic<V, F>(
    head: u64,
    tail: u64,
    factory: &F,
    mode: JitValueMode,
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
        return value_to_jit_generic(&sexpr, mode);
    }

    // Must be a heap pointer (S-expression)
    if tail_tag != TAG_HEAP {
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
        value_to_jit_generic(&sexpr, mode)
    } else if tail_val.is_unit() {
        // Treat Unit as empty S-expression
        let sexpr = factory.sexpr(vec![head_val]);
        value_to_jit_generic(&sexpr, mode)
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
/// - `mode`: JIT value mode
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
    mode: JitValueMode,
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

    value_to_jit_generic(&list, mode)
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
/// - `mode`: JIT value mode
///
/// # Returns
/// A `JitValue` containing the (quote value) S-expression
///
/// # Safety
/// - `val` must be a valid NaN-boxed value
pub unsafe fn make_quote_generic<V, F>(val: u64, factory: &F, mode: JitValueMode) -> JitValue
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

    value_to_jit_generic(&quoted, mode)
}

