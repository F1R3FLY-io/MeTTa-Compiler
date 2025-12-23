//! Rule dispatch runtime functions for JIT compilation
//!
//! This module provides FFI-callable rule dispatch operations:
//! - dispatch_rules - Dispatch rules for an expression
//! - try_rule - Try a single rule
//! - next_rule - Advance to next matching rule
//! - commit_rule - Commit to current rule (cut)
//! - fail_rule - Signal explicit rule failure
//! - lookup_rules - Look up rules by head symbol
//! - apply_subst - Apply substitution to an expression
//! - define_rule - Define a new rule

use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, JitBindingEntry,
    JIT_SIGNAL_FAIL,
};
use crate::backend::models::{MettaValue, Bindings};
use crate::backend::bytecode::mork_bridge::{MorkBridge, CompiledRule};
use crate::backend::eval::apply_bindings;
use super::helpers::{box_long, metta_to_jit};

// =============================================================================
// Phase C: Rule Dispatch Operations
// =============================================================================

/// Dispatch rules for an expression, returning the count of matching rules
///
/// Stack: [expr] -> [count]
///
/// # Arguments
/// * `ctx` - JIT context (stores matching rules internally)
/// * `expr` - NaN-boxed expression to match against rules
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Long - number of matching rules (0 if no match)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_dispatch_rules(
    ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return box_long(0),
    };

    // Check if bridge is available
    if ctx_ref.bridge_ptr.is_null() {
        return box_long(0);
    }

    // Convert expression to MettaValue
    let expr_val = JitValue::from_raw(expr);
    let expr_metta = expr_val.to_metta();

    // Get the MorkBridge and call dispatch_rules
    let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
    let rules = bridge.dispatch_rules(&expr_metta);
    let count = rules.len() as i64;

    // Free any previous rules
    if !ctx_ref.current_rules.is_null() {
        let _ = Box::from_raw(ctx_ref.current_rules as *mut Vec<CompiledRule>);
    }

    // Store rules in context for subsequent TryRule calls
    if count > 0 {
        let rules_box = Box::new(rules);
        ctx_ref.current_rules = Box::into_raw(rules_box) as *mut ();
    } else {
        ctx_ref.current_rules = std::ptr::null_mut();
    }
    ctx_ref.current_rule_idx = 0;

    box_long(count)
}

/// Try a single rule, pushing result or signaling failure
///
/// Stack: [expr] -> [result] or signal FAIL
///
/// # Arguments
/// * `ctx` - JIT context
/// * `rule_idx` - Index of rule in the match list (from previous DispatchRules)
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result value, or nil if rule doesn't match/doesn't exist
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_try_rule(
    ctx: *mut JitContext,
    rule_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        // No context - return nil as a valid "no match" result
        None => return JitValue::nil().to_bits(),
    };

    // Check if we have rules available
    if ctx_ref.current_rules.is_null() {
        // No rules dispatched yet - return nil
        return JitValue::nil().to_bits();
    }

    let rules = &*(ctx_ref.current_rules as *const Vec<CompiledRule>);
    let idx = rule_idx as usize;

    // Check bounds
    if idx >= rules.len() {
        // Rule index out of bounds - return nil
        return JitValue::nil().to_bits();
    }

    let rule = &rules[idx];

    // Install bindings from the pattern match into the JIT context
    // We need to push a new binding frame and populate it with the rule's bindings
    if ctx_ref.binding_frames_count < ctx_ref.binding_frames_cap && !ctx_ref.binding_frames.is_null() {
        // Push a new binding frame for this rule
        let frame_ptr = ctx_ref.binding_frames.add(ctx_ref.binding_frames_count);
        let frame = &mut *frame_ptr;

        // Count bindings
        let binding_count = rule.bindings.iter().count();
        if binding_count > 0 {
            // Allocate entries for this frame
            let layout = std::alloc::Layout::array::<JitBindingEntry>(binding_count)
                .expect("Layout calculation failed");
            frame.entries = std::alloc::alloc(layout) as *mut JitBindingEntry;
            frame.entries_cap = binding_count;
            frame.entries_count = 0;
            frame.scope_depth = ctx_ref.binding_frames_count as u32;

            // Install each binding
            for (name, value) in rule.bindings.iter() {
                // We need to store the variable name index - for now store as hash
                let name_idx = hash_string(name) as u32;
                let jit_value = metta_to_jit(value);

                let entry_ptr = frame.entries.add(frame.entries_count);
                *entry_ptr = JitBindingEntry::new(name_idx, jit_value);
                frame.entries_count += 1;
            }
        } else {
            frame.entries = std::ptr::null_mut();
            frame.entries_cap = 0;
            frame.entries_count = 0;
            frame.scope_depth = ctx_ref.binding_frames_count as u32;
        }

        ctx_ref.binding_frames_count += 1;
    }

    // Update current rule index
    ctx_ref.current_rule_idx = idx;

    // Signal bailout for the VM to execute the rule body
    // The rule body is in rule.body, which needs to be executed by the VM
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Call;

    // Return unit for now - actual result comes from rule body execution
    JitValue::unit().to_bits()
}

/// Simple hash function for binding names
#[inline]
pub(crate) fn hash_string(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Advance to next matching rule in choice point
///
/// Stack: [] -> [] (modifies internal state)
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 0 if advanced successfully, -1 if no more rules
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_next_rule(_ctx: *mut JitContext, _ip: u64) -> i64 {
    // Currently returns -1 (no more rules)
    // In full implementation, this would advance the rule index in the choice point
    -1
}

/// Commit to current rule (cut), removing alternative rules
///
/// Stack: [] -> []
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 0 on success
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_commit_rule(_ctx: *mut JitContext, _ip: u64) -> i64 {
    // Currently a no-op
    // In full implementation, this would remove alternative rules from choice points
    0
}

/// Signal explicit rule failure
///
/// Stack: [] -> [] (signals backtracking)
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// JIT_SIGNAL_FAIL to trigger backtracking
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fail_rule(_ctx: *mut JitContext, _ip: u64) -> i64 {
    JIT_SIGNAL_FAIL
}

/// Look up rules by head symbol
///
/// Stack: [] -> [count]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `head_idx` - Index of head symbol in constant pool
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Long - number of matching rules
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_lookup_rules(
    ctx: *mut JitContext,
    head_idx: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return box_long(0),
    };

    // Check if bridge is available
    if ctx_ref.bridge_ptr.is_null() {
        return box_long(0);
    }

    // Get head symbol from constants
    let head_metta = if head_idx < ctx_ref.constants_len as u64 {
        let constant_ptr = ctx_ref.constants.add(head_idx as usize);
        (*constant_ptr).clone()
    } else {
        return box_long(0);
    };

    // Dispatch rules using the head as expression
    let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
    let rules = bridge.dispatch_rules(&head_metta);
    let count = rules.len() as i64;

    // Free any previous rules
    if !ctx_ref.current_rules.is_null() {
        let _ = Box::from_raw(ctx_ref.current_rules as *mut Vec<CompiledRule>);
    }

    // Store rules in context
    if count > 0 {
        let rules_box = Box::new(rules);
        ctx_ref.current_rules = Box::into_raw(rules_box) as *mut ();
    } else {
        ctx_ref.current_rules = std::ptr::null_mut();
    }
    ctx_ref.current_rule_idx = 0;

    box_long(count)
}

/// Apply substitution to an expression
///
/// Stack: [expr, bindings] -> [result]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `expr` - NaN-boxed expression to substitute into
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result with variables substituted
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_apply_subst(
    ctx: *mut JitContext,
    expr: u64,
    _ip: u64,
) -> u64 {
    let expr_val = JitValue::from_raw(expr);
    let expr_metta = expr_val.to_metta();

    // Get bindings from context
    let bindings = collect_bindings_from_ctx(ctx);

    // Apply bindings to the expression
    let result = apply_bindings(&expr_metta, &bindings);

    // Convert result back to JitValue
    metta_to_jit(&result.into_owned()).to_bits()
}

/// Collect all bindings from the JIT context's binding frames
///
/// This iterates through all binding frames and collects their entries
/// into a Bindings struct for use with apply_bindings.
///
/// # Safety
/// The ctx pointer must be valid and point to a properly initialized JitContext.
pub(crate) unsafe fn collect_bindings_from_ctx(ctx: *mut JitContext) -> Bindings {
    let mut bindings = Bindings::new();

    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return bindings,
    };

    if ctx_ref.binding_frames.is_null() || ctx_ref.binding_frames_count == 0 {
        return bindings;
    }

    // Iterate through all binding frames (from innermost to outermost)
    // We collect from all frames to handle nested scopes
    for frame_idx in (0..ctx_ref.binding_frames_count).rev() {
        let frame = &*ctx_ref.binding_frames.add(frame_idx);

        if frame.entries.is_null() || frame.entries_count == 0 {
            continue;
        }

        // Collect entries from this frame
        for entry_idx in 0..frame.entries_count {
            let entry = &*frame.entries.add(entry_idx);

            // Convert JitValue back to MettaValue
            let value = entry.value.to_metta();

            // We need to recover the variable name from the hash
            // For now, we'll use the current_rules to find the original names
            // This is a workaround - ideally we'd store the actual names
            if let Some(name) = find_binding_name_by_hash(ctx, entry.name_idx as u64) {
                // Only insert if not already present (inner scope shadows outer)
                if bindings.get(&name).is_none() {
                    bindings.insert(name, value);
                }
            }
        }
    }

    bindings
}

/// Find binding name by hash from the current rules' bindings
///
/// This is a helper to recover variable names from their hashes.
/// It searches through the current rules' bindings to find matching names.
unsafe fn find_binding_name_by_hash(ctx: *mut JitContext, name_hash: u64) -> Option<String> {
    let ctx_ref = ctx.as_ref()?;

    if ctx_ref.current_rules.is_null() {
        return None;
    }

    let rules = &*(ctx_ref.current_rules as *const Vec<CompiledRule>);

    // Search through all rules' bindings for a matching name hash
    for rule in rules.iter() {
        // Use SmartBindings::iter() to get an iterator
        for (name, _value) in rule.bindings.iter() {
            if hash_string(name) as u32 == name_hash as u32 {
                return Some(name.clone());
            }
        }
    }

    None
}

/// Define a new rule in the environment
///
/// Stack: [pattern, body] -> [Unit]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `pattern_idx` - Index of pattern in constant pool
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit on success
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_define_rule(
    _ctx: *mut JitContext,
    _pattern_idx: u64,
    _ip: u64,
) -> u64 {
    // Currently a no-op returning Unit
    // In full implementation, this would add the rule to the environment
    JitValue::unit().to_bits()
}
