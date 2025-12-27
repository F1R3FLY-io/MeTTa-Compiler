//! Advanced call runtime functions for JIT compilation
//!
//! This module provides FFI-callable advanced call operations:
//! - call_native - Call a native Rust function by ID
//! - call_external - Call an external function by name index
//! - call_cached - Call a function with memoization

use super::helpers::metta_to_jit;
use crate::backend::bytecode::external_registry::{ExternalContext, ExternalRegistry};
use crate::backend::bytecode::jit::types::{JitContext, JitValue};
use crate::backend::bytecode::mork_bridge::MorkBridge;
use crate::backend::bytecode::vm::BytecodeVM;
use crate::backend::models::MettaValue;
use std::sync::Arc;
use tracing::warn;

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
///
/// # Safety
/// The caller must ensure `ctx` points to a valid `JitContext` with valid stack.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_call_native(
    ctx: *mut JitContext,
    func_id: u64,
    arg_count: u64,
    ip: u64,
) -> u64 {
    use crate::backend::bytecode::native_registry::{NativeContext, NativeRegistry};
    use crate::backend::Environment;

    if ctx.is_null() {
        return JitValue::nil().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Get arguments from stack
    let mut args = Vec::with_capacity(arg_count as usize);
    for _i in 0..arg_count as usize {
        if ctx_ref.sp < 1 {
            warn!(target: "mettatron::jit::runtime::call", ip, "Stack underflow in call_native");
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
            warn!(target: "mettatron::jit::runtime::call", ip, error = %e, "Native call error");
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
///
/// # Safety
/// The caller must ensure `ctx` points to a valid `JitContext` with valid stack and constants.
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
        warn!(target: "mettatron::jit::runtime::call", ip, "Stack underflow in call_external");
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
            warn!(target: "mettatron::jit::runtime::call", func_name, error = %e, "External call failed");
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
///
/// # Safety
/// The caller must ensure `ctx` points to a valid `JitContext` with valid stack and constants.
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
        warn!(target: "mettatron::jit::runtime::call", ip, "Stack underflow in call_cached");
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
