//! Call/TailCall runtime functions for JIT compilation
//!
//! This module provides FFI-callable call operations:
//! - call - Dispatch a call with native rule lookup
//! - tail_call - Dispatch a tail call with TCO hint
//! - call_n - Call with dynamic head from stack
//! - tail_call_n - Tail call with dynamic head from stack
//!
//! Also includes the grounded function fast path optimization.

use std::sync::Arc;
use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, JitAlternative,
    TAG_HEAP, TAG_NIL, PAYLOAD_MASK, MAX_ALTERNATIVES_INLINE,
};
use crate::backend::models::MettaValue;
use crate::backend::bytecode::mork_bridge::MorkBridge;
use crate::backend::bytecode::vm::BytecodeVM;
use super::metta_to_jit;

// =============================================================================
// Phase 3: Call/TailCall Support
// =============================================================================

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
        None => return TAG_NIL,
    };

    let arity = arity as usize;
    let head_index = head_index as usize;

    // Get head symbol from constant pool
    if head_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return TAG_NIL;
    }

    let head_value = &*ctx_ref.constants.add(head_index);
    let head = match head_value {
        MettaValue::Atom(s) => s.clone(),
        _ => {
            // Head must be an atom
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            return TAG_NIL;
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
            for (_name, value) in rule.bindings.iter() {
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
                    if alt_count <= MAX_ALTERNATIVES_INLINE {
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
        None => return TAG_NIL,
    };

    let arity = arity as usize;
    let head_index = head_index as usize;

    // Get head symbol from constant pool
    if head_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return TAG_NIL;
    }

    let head_value = &*ctx_ref.constants.add(head_index);
    let head = match head_value {
        MettaValue::Atom(s) => s.clone(),
        _ => {
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            return TAG_NIL;
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
        None => return TAG_NIL,
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
        None => return TAG_NIL,
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
