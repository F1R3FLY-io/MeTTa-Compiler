//! Call/TailCall runtime functions for JIT compilation
//!
//! This module provides FFI-callable call operations:
//! - call - Dispatch a call with native rule lookup
//! - tail_call - Dispatch a tail call with TCO hint
//! - call_n - Call with dynamic head from stack
//! - tail_call_n - Tail call with dynamic head from stack
//!
//! Also includes the grounded function fast path optimization.

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use super::helpers::value_to_jit_generic;
use super::metta_to_jit;
use crate::backend::bytecode::jit::types::{
    JitAlternative, JitBailoutReason, JitContext, JitValue, TypeClassification,
    TypeSignatureRegistry, MAX_ALTERNATIVES_INLINE, TAG_UNIT,
};
use crate::backend::bytecode::mork_bridge::MorkBridge;
use crate::backend::bytecode::vm::BytecodeVM;
use crate::backend::models::{MettaValue, ValueView};
use std::sync::Arc;

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
                        // wrapping_div avoids SIGFPE on i64::MIN / -1 per spec §13.2
                        Some(JitValue::from_long(a.wrapping_div(b)))
                    } else {
                        None // Division by zero - fall back to regular path
                    }
                }
                "%" => {
                    if b != 0 {
                        // wrapping_rem avoids SIGFPE on i64::MIN % -1; wraps to 0
                        Some(JitValue::from_long(a.wrapping_rem(b)))
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

/// Pre-evaluate a single S-expression argument using the trampoline evaluator.
///
/// Used by type-driven applicative evaluation (MeTTa HE parity): when a function
/// has an arrow type `(-> T1 T2 ... Tret)`, non-meta-typed arguments are
/// pre-evaluated before rule dispatch.
///
/// Returns `Some(evaluated)` if evaluation produced a result different from the
/// input, `None` if no environment is available or evaluation returned the
/// same expression (fixpoint).
///
/// # Safety
/// `ctx_ref.env_ptr` must point to a valid `MettaEnvironment` (or be null).
unsafe fn jit_pre_eval_arg(ctx_ref: &JitContext, arg: &MettaValue) -> Option<MettaValue> {
    // Plan 3 hook H-1 (2026-05-06): cooperative GC safepoint at the JIT
    // tier-return edge. Parallel-branch workers entering the trampoline from JIT
    // must surrender their EvalGuard so quiescence-driven GC can fire; the walker
    // registers JitContext slab roots + the local `arg` before the safepoint.
    //
    // SLAB ARM (B4.2 cfg-wall): on slab this is a discovery-style channel
    // (collect_jit_roots_into → worker_cooperative_safepoint, on the slab
    // parallel-worker `is_gc_requested()` rendezvous).
    #[cfg(not(feature = "index-gc"))]
    {
        let is_worker =
            crate::backend::eval::trampoline::eval_loop::IS_PARALLEL_WORKER.with(|f| f.get());
        if is_worker && crate::backend::models::gc_allocator::is_gc_requested() {
            let mut roots: Vec<MettaValue> = Vec::with_capacity(64);
            roots.push(arg.clone());
            crate::backend::bytecode::jit::runtime::gc_roots::collect_jit_roots_into(
                ctx_ref, &mut roots,
            );
            crate::backend::eval::trampoline::eval_loop::worker_cooperative_safepoint(&roots);
        }
    }
    // INDEX ARM (E1-c step 4, design §Part-6): the JIT tier must also PARK for the
    // dedicated GC thread under FANOUT>0. This adds LIVENESS (the cooperative
    // safepoint), NOT discovery — the same register-file values are ALSO structural
    // via the `VmLeaf::Jit` K-leaf pushed around `native_fn` (hybrid/arena.rs), read
    // by `collect_k_spine`. We self-root them eagerly here only so the park-window
    // snapshot is complete the instant the worker leaves the trampoline loop (the
    // dedicated thread drains WORKER_ROOT_BUFFER, not the parked register file), so
    // the Phase-A "no discovery side-channel" invariant is preserved (discovery stays
    // structural; this is the §Part-8 self-root-then-park). The gate omits `is_worker`
    // (the dedicated driver counts every EvalGuard-holding thread). `collect_jit_
    // roots_into` is index-aware (reconstructs the arena `Addr` handle, never derefs).
    // Slab stays inert because dedicated_gc_enabled() is false there; in index
    // mode, a pending FANOUT rendezvous makes this tier-return edge publish JIT
    // roots before parking.
    #[cfg(feature = "index-gc")]
    {
        if crate::backend::models::gc_allocator::is_gc_requested() {
            let mut roots: Vec<MettaValue> = Vec::with_capacity(64);
            roots.push(arg.clone());
            crate::backend::bytecode::jit::runtime::gc_roots::collect_jit_roots_into(
                ctx_ref, &mut roots,
            );
            crate::backend::eval::trampoline::eval_loop::worker_cooperative_safepoint(&roots);
        }
    }

    if ctx_ref.env_ptr.is_null() {
        return None;
    }

    // Reconstruct the environment reference from the raw pointer.
    let env = &*(ctx_ref.env_ptr as *const crate::backend::bytecode::MettaEnvironment);

    // Use a lightweight EvalContext adapter for the trampoline.
    use crate::backend::eval::trampoline::eval_loop::eval_trampoline;
    use crate::backend::eval::trampoline::EvalContext;
    use crate::backend::models::{global_factory, ActiveFactory};

    struct JitEvalContext {
        factory: ActiveFactory,
    }

    impl EvalContext for JitEvalContext {
        #[inline]
        fn factory(&self) -> &ActiveFactory {
            &self.factory
        }

        // should_safepoint / perform_safepoint inherit the trait defaults
        // (honor `is_gc_requested()`, run the canonical quiescent protocol).
    }

    let ctx = JitEvalContext {
        factory: global_factory(),
    };

    let (results, _) = eval_trampoline(arg.clone(), env.clone(), &ctx);

    // Take the first result. If it differs from the original, use it.
    if let Some((first, _b)) = results.into_iter().next() {
        if first != *arg {
            return Some(first);
        }
    }

    None
}

/// S-step (2026-05-17): Call-site type checking (T2/T3 in-tier).
///
/// JIT-tier analog of T0's `check_call_site_types` wire (`eval/step/sexpr.rs:
/// 3413-3427`) and T1's `op_dispatch_rules` no-match arm. After rule matching
/// produces zero results, check whether the call site is ill-typed against
/// the head's declared arrow type. The check itself reuses the generic
/// helper `crate::backend::eval::types::check_call_site_types` — same status
/// as `env.get_types_generic` / `apply_bindings_generic` already used here
/// (shared environment infrastructure, not a tier delegate). Permissive mode
/// (default) only fires when both head has a concrete `(-> ...)` declaration
/// and arg types are determinable; auto mode also fires on `%Undefined%`.
///
/// HE parity: `hyperon-experimental/lib/src/metta/types.rs::check_type`.
/// Errors are shaped as
///   `(Error <call-form> (BadArgType <1-indexed-N> <expected> <inferred>))`
/// or `(Error <call-form> IncorrectNumberOfArguments)`.
///
/// Returns `Some(error_jit_bits)` when ill-typed, `None` otherwise.
///
/// # Safety
/// `ctx_ref.env_ptr`, if non-null, must point to a valid `MettaEnvironment`.
#[inline]
unsafe fn jit_check_call_site_types(ctx_ref: &JitContext, expr: &MettaValue) -> Option<u64> {
    if ctx_ref.env_ptr.is_null() {
        return None;
    }
    let items = expr.as_sexpr()?;
    let env = &*(ctx_ref.env_ptr as *const crate::backend::bytecode::MettaEnvironment);
    let factory = crate::backend::models::global_factory();
    let err = crate::backend::eval::types::check_call_site_types(items, &factory, env)?;
    Some(value_to_jit_generic(&err).to_bits())
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
/// NaN-boxed TAG_PTR pointer to the call expression
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
        None => return TAG_UNIT,
    };

    let arity = arity as usize;
    let head_index = head_index as usize;

    // Get head symbol from constant pool
    if head_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return TAG_UNIT;
    }

    let head_value = &*ctx_ref.constants.add(head_index);
    let head: &str = match head_value.view() {
        ValueView::Atom(s) => s,
        ValueView::Float(_)
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Unit
        | ValueView::Empty
        | ValueView::NotReducible
        | ValueView::String(_)
        | ValueView::SExpr(_)
        | ValueView::Error(_, _)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::Space(_)
        | ValueView::State(_)
        | ValueView::Memo(_)
        | ValueView::Quoted(_)
        | ValueView::Lazy(_) => {
            // Head must be an atom
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            return TAG_UNIT;
        }
    };

    // Phase 9.6: All-error-types early exit — if every declared type for
    // the head is an Error type, short-circuit with an error value.
    if !ctx_ref.env_ptr.is_null() {
        let env = &*(ctx_ref.env_ptr as *const crate::backend::bytecode::MettaEnvironment);
        let op_types = env.get_types_generic(head);
        if !op_types.is_empty()
            && op_types.iter().all(|t| {
                t.as_sexpr().map_or(false, |ti| {
                    ti.first().and_then(|v| v.as_atom()) == Some("Error")
                })
            })
        {
            use crate::backend::models::MettaValueFactory;
            let factory = crate::backend::models::global_factory();
            // Build a minimal error expression
            let mut items = Vec::with_capacity(arity + 1);
            items.push(MettaValue::Atom(head));
            for i in 0..arity {
                items.push(JitValue::from_raw(*args_ptr.add(i)).to_metta());
            }
            let call_expr = MettaValue::SExpr(items);
            let err = factory.error(
                factory.string(&format!("All types for '{}' are errors", head)),
                call_expr,
            );
            return value_to_jit_generic(&err).to_bits();
        }
    }

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if !args_ptr.is_null() {
        if let Some(result) = try_grounded_fast_path(head, args_ptr, arity) {
            return result;
        }
    }

    // Build argument list, with optional type-driven pre-evaluation.
    // If the type registry is available and the head has an arrow type signature,
    // pre-evaluate non-meta-typed S-expression arguments via the trampoline.
    let mut items = Vec::with_capacity(arity + 1);
    items.push(MettaValue::Atom(head));

    let has_type_info = !ctx_ref.type_registry_ptr.is_null();
    let type_info = if has_type_info {
        let registry = &*(ctx_ref.type_registry_ptr as *const TypeSignatureRegistry);
        registry.get(head)
    } else {
        None
    };

    for i in 0..arity {
        let arg_raw = *args_ptr.add(i);
        let arg_jit = JitValue::from_raw(arg_raw);
        let arg_metta = arg_jit.to_metta();

        // Type-driven applicative evaluation: if the head has an arrow type,
        // pre-evaluate S-expression arguments whose formal type is NOT a meta-type.
        if let Some(info) = type_info {
            if i < info.arg_types.len()
                && info.arg_types[i] == TypeClassification::Evaluate
                && arg_metta.as_sexpr().is_some()
            {
                // Pre-evaluate this argument via the trampoline evaluator.
                if let Some(evaluated) = jit_pre_eval_arg(ctx_ref, &arg_metta) {
                    items.push(evaluated);
                    continue;
                }
            }
        }

        items.push(arg_metta);
    }

    // Create the call expression
    let expr = MettaValue::SExpr(items);

    // Phase 9.5: Normal-form memoization — skip dispatch for known-irreducible S-exprs
    if crate::backend::eval::trampoline::is_memoized_normal_form(&expr) {
        return value_to_jit_generic(&expr).to_bits();
    }

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // S-step (2026-05-17): Call-site type checking (T2/T3 in-tier).
            // See `jit_check_call_site_types` for full rationale. Mirrors
            // T0 Step 3.6 and T1's no-match arm. We check BEFORE memoizing
            // as normal form — otherwise the error would never fire on
            // subsequent identical calls.
            if let Some(err_bits) = jit_check_call_site_types(ctx_ref, &expr) {
                return err_bits;
            }
            // 2026-05-23 PT-canonical fix: at nested call_depth (>0), if the
            // head HAS rules for this arity but no match fires, return Empty
            // (HE-bisimilar branch-death) AND do NOT memoize as normal form.
            // Memoizing would poison `is_memoized_normal_form` so future T0
            // dispatches short-circuit at `eval_loop.rs:3207` and skip the
            // depth>0 Empty gate at `processing/ops.rs:304`. Without this,
            // PLN-main conjunction enumeration in foldl-atom produces a
            // spurious duplicate for failed-conjunct paths.
            if ctx_ref.call_depth > 0 {
                if let Some(items) = expr.as_sexpr() {
                    if let Some(head_atom) = items.first().and_then(|v| v.as_atom()) {
                        let arity = items.len().saturating_sub(1);
                        let has_any_rules = bridge.has_any_rules(head_atom, arity);
                        if has_any_rules {
                            return JitValue::empty().to_bits();
                        }
                    }
                }
            }
            // No rules match - return expression unchanged (irreducible)
            // Phase 9.5: Memoize as normal form for future fast-path.
            // Only safe at top-level or for true data constructors (no rules).
            crate::backend::eval::trampoline::memoize_normal_form(&expr);
            // S1 TOPLEVEL (2026-05-13): HE ADD-mode emits NOTHING at the
            // top level (call_depth==0, !interpret_mode). The T1 path is
            // the single source of truth for add-to-space side-effects;
            // T2/T3 JIT skips that and only contributes to observable
            // output when in INTERPRET mode.
            if ctx_ref.call_depth == 0 && !ctx_ref.interpret_mode {
                return JitValue::empty().to_bits();
            }
            // This is a major optimization: no bailout needed!
            return value_to_jit_generic(&expr).to_bits();
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
                    let result = results
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| MettaValue::Unit());
                    return metta_to_jit(&result).to_bits();
                }
                Err(_) => {
                    // Execution error - bailout for VM to handle
                    ctx_ref.bailout = true;
                    ctx_ref.bailout_ip = ip as usize;
                    ctx_ref.bailout_reason = JitBailoutReason::Call;
                    return value_to_jit_generic(&expr).to_bits();
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

        return value_to_jit_generic(&expr).to_bits();
    }

    // No bridge - signal bailout for VM to handle
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Call;

    // Return the expression via value_to_jit_generic (handles inline NaN-boxed values safely)
    value_to_jit_generic(&expr).to_bits()
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
        None => return TAG_UNIT,
    };

    let arity = arity as usize;
    let head_index = head_index as usize;

    // Get head symbol from constant pool
    if head_index >= ctx_ref.constants_len {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::UnsupportedOperation;
        return TAG_UNIT;
    }

    let head_value = &*ctx_ref.constants.add(head_index);
    let head: &str = match head_value.view() {
        ValueView::Atom(s) => s,
        ValueView::Float(_)
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Unit
        | ValueView::Empty
        | ValueView::NotReducible
        | ValueView::String(_)
        | ValueView::SExpr(_)
        | ValueView::Error(_, _)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::Space(_)
        | ValueView::State(_)
        | ValueView::Memo(_)
        | ValueView::Quoted(_)
        | ValueView::Lazy(_) => {
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            return TAG_UNIT;
        }
    };

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if !args_ptr.is_null() {
        if let Some(result) = try_grounded_fast_path(head, args_ptr, arity) {
            return result;
        }
    }

    // Build argument list, with optional type-driven pre-evaluation (MeTTa HE parity).
    let mut items = Vec::with_capacity(arity + 1);
    items.push(MettaValue::Atom(head));

    let has_type_info = !ctx_ref.type_registry_ptr.is_null();
    let type_info = if has_type_info {
        let registry = &*(ctx_ref.type_registry_ptr as *const TypeSignatureRegistry);
        registry.get(head)
    } else {
        None
    };

    for i in 0..arity {
        let arg_raw = *args_ptr.add(i);
        let arg_jit = JitValue::from_raw(arg_raw);
        let arg_metta = arg_jit.to_metta();

        // Type-driven applicative evaluation: pre-evaluate S-expression arguments
        // whose formal type is NOT a meta-type.
        if let Some(info) = type_info {
            if i < info.arg_types.len()
                && info.arg_types[i] == TypeClassification::Evaluate
                && arg_metta.as_sexpr().is_some()
            {
                if let Some(evaluated) = jit_pre_eval_arg(ctx_ref, &arg_metta) {
                    items.push(evaluated);
                    continue;
                }
            }
        }

        items.push(arg_metta);
    }

    // Create the call expression
    let expr = MettaValue::SExpr(items);

    // Phase 9.5: Normal-form memoization — skip dispatch for known-irreducible S-exprs
    if crate::backend::eval::trampoline::is_memoized_normal_form(&expr) {
        return value_to_jit_generic(&expr).to_bits();
    }

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // S-step (2026-05-17): Call-site type checking (T2/T3 in-tier,
            // tail-call path). Mirrors T0 Step 3.6 and T1's no-match arm.
            // See `jit_check_call_site_types` for full rationale.
            if let Some(err_bits) = jit_check_call_site_types(ctx_ref, &expr) {
                return err_bits;
            }
            // 2026-05-23 PT-canonical Empty gate (see primary site for full
            // rationale): at nested call_depth, has-rules-no-match → Empty.
            if ctx_ref.call_depth > 0 {
                if let Some(items) = expr.as_sexpr() {
                    if let Some(head_atom) = items.first().and_then(|v| v.as_atom()) {
                        let arity = items.len().saturating_sub(1);
                        if bridge.has_any_rules(head_atom, arity) {
                            return JitValue::empty().to_bits();
                        }
                    }
                }
            }
            // No rules match - return expression unchanged (irreducible)
            // Phase 9.5: Memoize as normal form for future fast-path.
            crate::backend::eval::trampoline::memoize_normal_form(&expr);
            // S1 TOPLEVEL (2026-05-13): HE ADD-mode emits NOTHING.
            if ctx_ref.call_depth == 0 && !ctx_ref.interpret_mode {
                return JitValue::empty().to_bits();
            }
            // This is a major optimization: no bailout needed!
            return value_to_jit_generic(&expr).to_bits();
        }

        // Rules matched - bailout for VM to execute rule bodies with TCO
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::TailCall;

        return value_to_jit_generic(&expr).to_bits();
    }

    // No bridge - signal bailout for VM to handle (with TCO hint)
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::TailCall;

    // Return the expression via value_to_jit_generic (handles inline NaN-boxed values safely)
    value_to_jit_generic(&expr).to_bits()
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
        None => return TAG_UNIT,
    };

    let arity = arity as usize;

    // Convert head from NaN-boxed to MettaValue
    let head_jit = JitValue::from_raw(head_val);
    let head_metta = head_jit.to_metta();

    // Phase 9.1: Variable-head guard — ($f x) is data, not callable
    if let ValueView::Atom(head_str) = head_metta.view() {
        if head_str.starts_with('$') {
            // Variable head — return as data S-expression
            let mut items = Vec::with_capacity(arity + 1);
            items.push(head_metta);
            for i in 0..arity {
                let arg_raw = *args_ptr.add(i);
                items.push(JitValue::from_raw(arg_raw).to_metta());
            }
            let expr = MettaValue::SExpr(items);
            return value_to_jit_generic(&expr).to_bits();
        }
    }

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if let ValueView::Atom(head_str) = head_metta.view() {
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

    // Phase 9.5: Normal-form memoization — skip dispatch for known-irreducible S-exprs
    if crate::backend::eval::trampoline::is_memoized_normal_form(&expr) {
        return value_to_jit_generic(&expr).to_bits();
    }

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // S-step (2026-05-17): Call-site type checking (T2/T3 in-tier,
            // CallN path). Mirrors T0 Step 3.6 and T1's no-match arm.
            // See `jit_check_call_site_types` for full rationale.
            if let Some(err_bits) = jit_check_call_site_types(ctx_ref, &expr) {
                return err_bits;
            }
            // 2026-05-23 PT-canonical Empty gate (see primary site).
            if ctx_ref.call_depth > 0 {
                if let Some(items) = expr.as_sexpr() {
                    if let Some(head_atom) = items.first().and_then(|v| v.as_atom()) {
                        let arity = items.len().saturating_sub(1);
                        if bridge.has_any_rules(head_atom, arity) {
                            return JitValue::empty().to_bits();
                        }
                    }
                }
            }
            // No rules match - return expression unchanged (irreducible)
            // Phase 9.5: Memoize as normal form for future fast-path.
            crate::backend::eval::trampoline::memoize_normal_form(&expr);
            // S1 TOPLEVEL (2026-05-13): HE ADD-mode emits NOTHING.
            if ctx_ref.call_depth == 0 && !ctx_ref.interpret_mode {
                return JitValue::empty().to_bits();
            }
            return value_to_jit_generic(&expr).to_bits();
        }

        // Rules matched - bailout for VM to execute rule bodies
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::Call;

        return value_to_jit_generic(&expr).to_bits();
    }

    // No bridge - signal bailout for VM to handle
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::Call;

    // Return the expression via value_to_jit_generic (handles inline NaN-boxed values safely)
    value_to_jit_generic(&expr).to_bits()
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
        None => return TAG_UNIT,
    };

    let arity = arity as usize;

    // Convert head from NaN-boxed to MettaValue
    let head_jit = JitValue::from_raw(head_val);
    let head_metta = head_jit.to_metta();

    // Phase 9.1: Variable-head guard — ($f x) is data, not callable
    if let ValueView::Atom(head_str) = head_metta.view() {
        if head_str.starts_with('$') {
            // Variable head — return as data S-expression
            let mut items = Vec::with_capacity(arity + 1);
            items.push(head_metta);
            for i in 0..arity {
                let arg_raw = *args_ptr.add(i);
                items.push(JitValue::from_raw(arg_raw).to_metta());
            }
            let expr = MettaValue::SExpr(items);
            return value_to_jit_generic(&expr).to_bits();
        }
    }

    // Optimization 3.2: Fast path for grounded functions
    // Try to execute grounded ops directly without MorkBridge lookup
    if let ValueView::Atom(head_str) = head_metta.view() {
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

    // Phase 9.5: Normal-form memoization — skip dispatch for known-irreducible S-exprs
    if crate::backend::eval::trampoline::is_memoized_normal_form(&expr) {
        return value_to_jit_generic(&expr).to_bits();
    }

    // Try native rule dispatch if bridge is available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let matches = bridge.dispatch_rules(&expr);

        if matches.is_empty() {
            // S-step (2026-05-17): Call-site type checking (T2/T3 in-tier,
            // TailCallN path). Mirrors T0 Step 3.6 and T1's no-match arm.
            // See `jit_check_call_site_types` for full rationale.
            if let Some(err_bits) = jit_check_call_site_types(ctx_ref, &expr) {
                return err_bits;
            }
            // 2026-05-23 PT-canonical Empty gate (see primary site).
            if ctx_ref.call_depth > 0 {
                if let Some(items) = expr.as_sexpr() {
                    if let Some(head_atom) = items.first().and_then(|v| v.as_atom()) {
                        let arity = items.len().saturating_sub(1);
                        if bridge.has_any_rules(head_atom, arity) {
                            return JitValue::empty().to_bits();
                        }
                    }
                }
            }
            // No rules match - return expression unchanged (irreducible)
            // Phase 9.5: Memoize as normal form for future fast-path.
            crate::backend::eval::trampoline::memoize_normal_form(&expr);
            // S1 TOPLEVEL (2026-05-13): HE ADD-mode emits NOTHING.
            if ctx_ref.call_depth == 0 && !ctx_ref.interpret_mode {
                return JitValue::empty().to_bits();
            }
            return value_to_jit_generic(&expr).to_bits();
        }

        // Rules matched - bailout for VM to execute rule bodies with TCO
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::TailCall;

        return value_to_jit_generic(&expr).to_bits();
    }

    // No bridge - signal bailout for VM to handle (with TCO hint)
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::TailCall;

    // Return the expression via value_to_jit_generic (handles inline NaN-boxed values safely)
    value_to_jit_generic(&expr).to_bits()
}
