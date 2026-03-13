//! Rule dispatch runtime functions for JIT compilation
//!
//! This module provides FFI-callable rule dispatch operations:
//! - dispatch_rules - Dispatch rules for an expression
//! - dispatch_rules_profiling - Profile-collecting variant of dispatch_rules
//! - try_rule - Try a single rule
//! - next_rule - Advance to next matching rule
//! - commit_rule - Commit to current rule (cut)
//! - fail_rule - Signal explicit rule failure
//! - lookup_rules - Look up rules by head symbol
//! - apply_subst - Apply substitution to an expression
//! - define_rule - Define a new rule
//! - check_head - Fast head symbol check for specialized dispatch
//! - get_arity_fast - Fast arity check for specialized dispatch
//! - eval_with_bindings - Evaluate a rule body with pre-extracted bindings

use super::helpers::{box_long, metta_to_jit};
use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitBindingEntry, JitContext, JitValue, JIT_SIGNAL_FAIL,
};
use crate::backend::bytecode::mork_bridge::{CompiledRule, MorkBridge};
use crate::backend::bytecode::runtime_profile::RuleDispatchSite;
use crate::backend::eval::apply_bindings;
use crate::backend::models::Bindings;

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

    // Phase 9.5: Normal-form memoization — skip dispatch for known-irreducible S-exprs
    if expr_metta.as_sexpr().is_some()
        && crate::backend::eval::trampoline::is_memoized_normal_form(&expr_metta)
    {
        return box_long(0);
    }

    // Get the MorkBridge and call dispatch_rules
    let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
    let rules = bridge.dispatch_rules(&expr_metta);
    let count = rules.len() as i64;

    // Phase 9 wiring: Collect dispatch profile data when profiling is active.
    // During the JIT1 execution window (200-2000 execs), profile_ptr is set
    // on JitContext, enabling automatic profile collection without requiring
    // a separate profiling-specific codegen variant.
    if !ctx_ref.profile_ptr.is_null() && count > 0 {
        record_dispatch_profile(ctx_ref, &expr_metta, &rules);
    }

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
pub unsafe extern "C" fn jit_runtime_try_rule(ctx: *mut JitContext, rule_idx: u64, ip: u64) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        // No context - return nil as a valid "no match" result
        None => return JitValue::unit().to_bits(),
    };

    // Check if we have rules available
    if ctx_ref.current_rules.is_null() {
        // No rules dispatched yet - return nil
        return JitValue::unit().to_bits();
    }

    let rules = &*(ctx_ref.current_rules as *const Vec<CompiledRule>);
    let idx = rule_idx as usize;

    // Check bounds
    if idx >= rules.len() {
        // Rule index out of bounds - return nil
        return JitValue::unit().to_bits();
    }

    let rule = &rules[idx];

    // Install bindings from the pattern match into the JIT context
    // We need to push a new binding frame and populate it with the rule's bindings
    if ctx_ref.binding_frames_count < ctx_ref.binding_frames_cap
        && !ctx_ref.binding_frames.is_null()
    {
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
    use xxhash_rust::xxh3::xxh3_64;
    xxh3_64(s.as_bytes())
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
pub unsafe extern "C" fn jit_runtime_apply_subst(ctx: *mut JitContext, expr: u64, _ip: u64) -> u64 {
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
unsafe fn find_binding_name_by_hash(ctx: *mut JitContext, name_hash: u64) -> Option<&'static str> {
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
                return Some(name);
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

// =============================================================================
// Phase 9: Profile-Collecting and Specialized Dispatch Operations
// =============================================================================

/// Profile-collecting variant of dispatch_rules.
///
/// Called when `ctx.profile_ptr` is non-null, indicating profiling is active
/// for this expression. Records which rule indices matched and how often
/// into the RuntimeTypeProfile's `dispatch_sites` field.
///
/// This function:
/// 1. Dispatches rules normally (same as `jit_runtime_dispatch_rules`)
/// 2. Computes a site_hash from the expression's head symbol + arity
/// 3. For each matching rule, records the match in the profile's
///    `dispatch_sites` vector
///
/// # Safety
/// Same requirements as `jit_runtime_dispatch_rules`, plus:
/// - `ctx.profile_ptr` must point to a valid
///   `Arc<parking_lot::Mutex<RuntimeTypeProfile>>`
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_dispatch_rules_profiling(
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

    // Normal-form memoization check (same as non-profiling variant)
    if expr_metta.as_sexpr().is_some()
        && crate::backend::eval::trampoline::is_memoized_normal_form(&expr_metta)
    {
        return box_long(0);
    }

    // Get the MorkBridge and call dispatch_rules
    let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
    let rules = bridge.dispatch_rules(&expr_metta);
    let count = rules.len() as i64;

    // === Profile collection ===
    // Only record if profiling is active and we have matching rules
    if !ctx_ref.profile_ptr.is_null() && count > 0 {
        record_dispatch_profile(ctx_ref, &expr_metta, &rules);
    }

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

/// Record dispatch profile data for a set of matching rules.
///
/// Called from `jit_runtime_dispatch_rules_profiling` to update the
/// `RuntimeTypeProfile.dispatch_sites` with per-rule match counts.
///
/// # Safety
/// `ctx_ref.profile_ptr` must be a valid `Arc<parking_lot::Mutex<RuntimeTypeProfile>>`.
unsafe fn record_dispatch_profile(
    ctx_ref: &JitContext,
    expr: &crate::backend::models::MettaValue,
    rules: &[CompiledRule],
) {
    use crate::backend::bytecode::runtime_profile::RuntimeTypeProfile;
    use std::sync::Arc;

    // Compute site_hash from expression head + arity
    let (head_name, arity) = match expr.as_sexpr() {
        Some(items) if !items.is_empty() => {
            let head = items[0].as_atom().unwrap_or("");
            (head, items.len() as u16)
        }
        _ => return, // Non-S-expr, skip profiling
    };

    let site_hash = compute_site_hash(head_name, arity);

    // Acquire the profile lock and record
    let profile_arc = Arc::from_raw(ctx_ref.profile_ptr as *const parking_lot::Mutex<RuntimeTypeProfile>);
    {
        let mut profile = profile_arc.lock();

        // Find or create the dispatch site entry
        let site = match profile.dispatch_sites.iter_mut().find(|s| s.site_hash == site_hash) {
            Some(existing) => existing,
            None => {
                profile.dispatch_sites.push(RuleDispatchSite::new(
                    site_hash,
                    head_name.to_string(),
                    arity,
                ));
                profile.dispatch_sites.last_mut().expect("just pushed")
            }
        };

        // Record each matching rule
        for (idx, rule) in rules.iter().enumerate() {
            let lhs_hash = hash_metta_value_quick(&rule.lhs);
            // Hash the body chunk pointer as an identity proxy for the RHS.
            // Different chunk objects → different addresses → different hashes.
            let rhs_hash = {
                use xxhash_rust::xxh3::xxh3_64;
                let body_ptr = std::sync::Arc::as_ptr(&rule.body) as usize;
                xxh3_64(&body_ptr.to_le_bytes())
            };
            // The RHS "has variables" if any bindings were produced by matching
            let rhs_has_variables = !rule.bindings.is_empty();
            site.record_match(idx as u16, lhs_hash, rhs_hash, rhs_has_variables);
        }
    }

    // IMPORTANT: Don't drop the Arc — we need to leak it back since the
    // profile_ptr is still live. Increment the strong count to compensate.
    let _ = Arc::into_raw(profile_arc);
}

/// Compute a site hash from head symbol name and arity.
///
/// Uses xxh3 for fast, high-quality hashing.
#[inline]
fn compute_site_hash(head: &str, arity: u16) -> u64 {
    use xxhash_rust::xxh3::xxh3_64;
    let mut buf = [0u8; 256];
    let head_bytes = head.as_bytes();
    let len = head_bytes.len().min(254);
    buf[..len].copy_from_slice(&head_bytes[..len]);
    buf[len] = (arity >> 8) as u8;
    buf[len + 1] = arity as u8;
    xxh3_64(&buf[..len + 2])
}

/// Quick hash of a MettaValue for rule identity tracking.
///
/// Used to detect stale profiles after rule index changes.
/// Not cryptographic — just needs to detect mutations.
#[inline]
fn hash_metta_value_quick(val: &crate::backend::models::MettaValue) -> u64 {
    use xxhash_rust::xxh3::xxh3_64;
    // Use the inner pointer as a fast identity hash.
    // For slab-allocated values, different content → different pointer.
    let ptr = val.inner_ptr() as usize;
    xxh3_64(&ptr.to_le_bytes())
}

/// Fast head symbol check for specialized dispatch.
///
/// Returns 1 if the expression's head symbol matches the expected atom name,
/// 0 otherwise. Uses pointer comparison for interned atoms first (O(1)),
/// falls back to string comparison if needed.
///
/// # Safety
/// `ctx` must be valid. `expr` must be a valid NaN-boxed value.
/// `expected_atom_ptr` is a `*const str` (interned, 'static lifetime),
/// encoded as a u64 fat pointer pair: the first u64 is data ptr, the
/// second is the length. For FFI, we pass a pointer to the `&str` itself.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_check_head(
    _ctx: *mut JitContext,
    expr: u64,
    expected_name_ptr: u64,
    expected_name_len: u64,
) -> u64 {
    let expr_val = JitValue::from_raw(expr);
    let expr_metta = expr_val.to_metta();

    if let Some(items) = expr_metta.as_sexpr() {
        if !items.is_empty() {
            if let Some(head_name) = items[0].as_atom() {
                // Reconstruct the expected name from pointer + length
                let expected_bytes = std::slice::from_raw_parts(
                    expected_name_ptr as *const u8,
                    expected_name_len as usize,
                );
                let expected = std::str::from_utf8_unchecked(expected_bytes);

                // Pointer equality first (interned atoms share addresses)
                if std::ptr::eq(head_name.as_ptr(), expected.as_ptr())
                    && head_name.len() == expected.len()
                {
                    return 1;
                }
                // Fall back to string comparison
                if head_name == expected {
                    return 1;
                }
            }
        }
    }
    0
}

/// Fast arity check for specialized dispatch.
///
/// Returns the number of elements in an S-expression, or 0 for non-S-exprs.
///
/// # Safety
/// `ctx` must be valid. `expr` must be a valid NaN-boxed value.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_arity_fast(
    _ctx: *mut JitContext,
    expr: u64,
) -> u64 {
    let expr_val = JitValue::from_raw(expr);
    let expr_metta = expr_val.to_metta();

    if let Some(items) = expr_metta.as_sexpr() {
        items.len() as u64
    } else {
        0
    }
}

/// Evaluate a rule body with pre-extracted bindings.
///
/// Called from specialized JIT code when the RHS is too complex to inline
/// in Cranelift IR. The bindings are passed as a flat array of
/// (name_hash, JitValue) pairs extracted by the inline pattern matching.
///
/// # Arguments
/// * `ctx` - JIT context (used for binding name resolution)
/// * `rhs_ptr` - NaN-boxed pointer to the rule RHS MettaValue (TAG_PTR)
/// * `bindings_ptr` - Pointer to array of (u64, u64) = (name_hash, JitValue) pairs
/// * `binding_count` - Number of bindings in the array
///
/// # Returns
/// NaN-boxed result value after applying bindings and evaluating
///
/// # Safety
/// - `ctx` must be valid
/// - `rhs_ptr` must be a valid TAG_PTR NaN-boxed MettaValue
/// - `bindings_ptr` must point to a valid array of `binding_count` (u64, u64) pairs
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_eval_with_bindings(
    ctx: *mut JitContext,
    rhs_ptr: u64,
    bindings_ptr: u64,
    binding_count: u64,
) -> u64 {
    // Reconstruct MettaValue from NaN-boxed pointer
    let rhs = JitValue::from_raw(rhs_ptr).to_metta();

    // Reconstruct bindings from flat array
    let mut bindings = Bindings::new();
    if binding_count > 0 && bindings_ptr != 0 {
        let pairs = std::slice::from_raw_parts(
            bindings_ptr as *const (u64, u64),
            binding_count as usize,
        );
        for &(name_hash, value_bits) in pairs {
            if let Some(name) = find_binding_name_by_hash(ctx, name_hash) {
                let value = JitValue::from_raw(value_bits).to_metta();
                bindings.insert(name, value);
            }
        }
    }

    // Apply bindings to the RHS body
    let result = apply_bindings(&rhs, &bindings);
    metta_to_jit(&result.into_owned()).to_bits()
}
