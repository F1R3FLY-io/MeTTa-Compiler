//! Non-determinism runtime functions for JIT compilation
//!
//! This module provides FFI-callable non-determinism operations:
//! - Choice point management (push, fail, get_current_alternative)
//! - Fork/Yield/Collect for backtracking
//! - Native dispatcher functions for Stage 2 JIT
//! - Dispatcher loop for nondeterministic execution

use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, JitChoicePoint, JitAlternative, JitAlternativeTag,
    TAG_HEAP, TAG_NIL, PAYLOAD_MASK,
    JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL, JIT_SIGNAL_ERROR,
    MAX_ALTERNATIVES_INLINE, MAX_STACK_SAVE_VALUES,
};
use crate::backend::models::MettaValue;

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
    if alt_count as usize > MAX_ALTERNATIVES_INLINE {
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
        None => return TAG_NIL,
    };

    let count = count as usize;

    // If no alternatives, this is a fail
    if count == 0 {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Fork;
        return TAG_NIL;
    }

    // Get the first alternative from constant pool
    let first_index = if !indices_ptr.is_null() {
        *indices_ptr as usize
    } else {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Fork;
        return TAG_NIL;
    };

    if first_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return TAG_NIL;
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
        None => return TAG_NIL,
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

    TAG_NIL
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
        None => return TAG_NIL,
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
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No results - return empty SExpr
    let empty = MettaValue::SExpr(Vec::new());
    let boxed = Box::new(empty);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
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
        None => return TAG_NIL,
    };

    let count = count as usize;

    // If no alternatives, return Nil (fail case)
    if count == 0 {
        return TAG_NIL;
    }

    // Check if we have nondet support
    if !ctx_ref.has_nondet_support() {
        // Fall back to bailout mode
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Fork;
        return TAG_NIL;
    }

    // Get the first alternative from constant pool
    let first_index = if !indices_ptr.is_null() {
        *indices_ptr as usize
    } else {
        return TAG_NIL;
    };

    if first_index >= ctx_ref.constants_len {
        return TAG_NIL;
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
        if alt_count > MAX_ALTERNATIVES_INLINE {
            // Too many alternatives for inline storage - fall back to bailout
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::Fork;
            return first_jit.to_bits();
        }

        // Optimization 5.2: Check if stack fits in pool
        let stack_count = ctx_ref.sp;
        if stack_count > MAX_STACK_SAVE_VALUES {
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
                // Call the function from parent module (mod.rs) since it depends on bindings
                super::jit_runtime_resume_space_match(ctx, alt as *const JitAlternative)
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
        None => return TAG_NIL,
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
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // No results - return empty SExpr
    let empty = MettaValue::SExpr(Vec::new());
    let boxed = Box::new(empty);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
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

// NOTE: jit_runtime_resume_space_match is defined in mod.rs as it depends on
// jit_runtime_restore_bindings which is in the bindings section.

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
