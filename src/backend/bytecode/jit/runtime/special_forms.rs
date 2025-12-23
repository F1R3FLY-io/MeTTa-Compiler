//! Special forms runtime functions for JIT compilation
//!
//! This module provides FFI-callable special form operations:
//! - eval_if - Conditional evaluation
//! - eval_let - Single binding
//! - eval_let_star - Sequential bindings
//! - eval_match - Pattern match expression
//! - eval_case - Pattern-based switch
//! - eval_chain - Sequential evaluation
//! - eval_quote - Prevent evaluation
//! - eval_unquote - Force evaluation of quoted
//! - eval_eval - Force evaluation
//! - eval_bind - Create binding in space
//! - eval_new - Create new space
//! - eval_collapse - Determinize nondeterministic results
//! - eval_superpose - Create nondeterministic choice
//! - eval_memo - Memoized evaluation
//! - eval_memo_first - Memoize only first result
//! - eval_pragma - Compiler directive
//! - eval_function - Function definition
//! - eval_lambda - Create closure
//! - eval_apply - Apply closure to arguments

use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, JitBindingEntry, JitAlternative,
    JIT_SIGNAL_FAIL,
};
use crate::backend::models::MettaValue;
use crate::backend::eval::pattern_match;
use super::helpers::{box_long, metta_to_jit};
use super::bindings::{jit_runtime_store_binding, jit_runtime_push_binding_frame};
use super::pattern_matching::jit_runtime_pattern_match;
use super::value_creation::jit_runtime_make_quote;
use super::rule_dispatch::{hash_string, collect_bindings_from_ctx};
use super::MAX_ALTERNATIVES_INLINE;

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
                        let layout = std::alloc::Layout::array::<JitBindingEntry>(binding_count)
                            .expect("Layout calculation failed");
                        frame.entries = std::alloc::alloc(layout) as *mut JitBindingEntry;
                        frame.entries_cap = binding_count;
                        frame.entries_count = 0;
                    }

                    for (name, val) in bindings.iter() {
                        if frame.entries_count < frame.entries_cap {
                            let name_idx = hash_string(name) as u32;
                            let jit_value = metta_to_jit(val);
                            let entry_ptr = frame.entries.add(frame.entries_count);
                            *entry_ptr = JitBindingEntry::new(name_idx, jit_value);
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
    use crate::backend::models::SpaceHandle;
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
                if alt_count > MAX_ALTERNATIVES_INLINE {
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
    if ctx.is_null() {
        // No context - create minimal closure representation
        let closure = MettaValue::SExpr(vec![
            MettaValue::Atom("lambda".to_string()),
            MettaValue::Long(param_count as i64),
            MettaValue::SExpr(vec![]), // Empty captured env
        ]);
        return metta_to_jit(&closure).to_bits();
    }

    let _ctx_ref = &*ctx;

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
    use std::sync::Arc;

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
                    Arc::new(closure_val),
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
