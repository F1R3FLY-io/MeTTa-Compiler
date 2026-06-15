//! S-expression operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable S-expression operations:
//! - push_empty - Create an empty S-expression
//! - get_head - Get the first element of an S-expression
//! - get_tail - Get all elements except the first
//! - get_arity - Get the number of elements
//! - get_element - Get element at a specific index
//! - structural_head / structural_tail - `car-atom`/`cdr-atom` with
//!   tree-walker-equivalent pre-eval semantics: pops the raw (unreduced)
//!   argument, applies the 4-condition predicate (variable head, grounded
//!   op, eager special form, or arrow-typed head) against the live env
//!   from `ctx.env_ptr`, optionally reduces via the trampoline, then takes
//!   head/tail.

use super::helpers::{metta_to_jit, value_to_jit_generic};
use crate::backend::bytecode::jit::types::{JitContext, JitValue, TAG_UNIT};
use crate::backend::models::{MettaValue, ValueView};

// =============================================================================
// S-Expression Operations (Stage 14: Head/Tail/Arity/Element)
// =============================================================================

/// Runtime function for PushEmpty opcode
///
/// Creates and returns an empty S-expression ().
///
/// # Returns
/// NaN-boxed inner pointer to empty SExpr
///
/// # Safety
/// No special safety requirements.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_empty() -> u64 {
    let empty = MettaValue::SExpr(Vec::new());
    value_to_jit_generic(&empty).to_bits()
}

/// Runtime function for GetHead opcode
///
/// Gets the head (first element) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed head element, or TAG_UNIT if empty/not an SExpr
///
/// # Safety
/// The inner pointer must be valid if val is TAG_PTR.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_head(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    // H3 (2026-05-05) hard-cut: empty/non-expr → return HE-bisimilar Error atom
    // (NaN-boxed). Quoted-transparency extension removed.
    let jit_val = JitValue::from_raw(val);

    fn make_car_error(input_metta: &MettaValue) -> u64 {
        let call = MettaValue::SExpr(vec![MettaValue::Atom("car-atom"), input_metta.clone()]);
        let err = MettaValue::Error(
            call,
            MettaValue::String("car-atom expects a non-empty expression as an argument"),
        );
        metta_to_jit(&err).to_bits()
    }

    if !jit_val.is_heap() {
        // Non-heap (Long/Bool/Float/Unit/Empty inline) — emit Error.
        let metta_val = MettaValue::Unit(); // Placeholder — original not reconstructable.
        return make_car_error(&metta_val);
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return make_car_error(&MettaValue::Unit());
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    match metta_val.view() {
        ValueView::SExpr(items) => {
            if items.is_empty() {
                make_car_error(&metta_val)
            } else {
                let head = &items[0];
                value_to_jit_generic(head).to_bits()
            }
        }
        _ => make_car_error(&metta_val),
    }
}

/// Runtime function for GetTail opcode
///
/// Gets the tail (all elements except first) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed heap pointer to tail SExpr, or empty SExpr if empty/not an SExpr
///
/// # Safety
/// The inner pointer must be valid if val is TAG_PTR.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_tail(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    // H3 (2026-05-05) hard-cut: empty sexpr / non-expr → HE Error atom.
    // Quoted-transparency extension removed.
    let jit_val = JitValue::from_raw(val);

    fn make_cdr_error(input_metta: &MettaValue) -> u64 {
        let call = MettaValue::SExpr(vec![MettaValue::Atom("cdr-atom"), input_metta.clone()]);
        let err = MettaValue::Error(
            call,
            MettaValue::String("cdr-atom expects a non-empty expression as an argument"),
        );
        metta_to_jit(&err).to_bits()
    }

    if !jit_val.is_heap() {
        return make_cdr_error(&MettaValue::Unit());
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return make_cdr_error(&MettaValue::Unit());
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    match metta_val.view() {
        ValueView::SExpr(items) => {
            if items.is_empty() {
                make_cdr_error(&metta_val)
            } else {
                let tail: Vec<MettaValue> = items[1..].to_vec();
                let expr = MettaValue::SExpr(tail);
                value_to_jit_generic(&expr).to_bits()
            }
        }
        _ => make_cdr_error(&metta_val),
    }
}

/// Apply the 4-condition structural-arg pre-eval predicate and optionally
/// reduce via the trampoline. Identical semantics to tree-walker's
/// `is_reducible_structural_arg` (src/backend/eval/step/sexpr.rs) and the
/// bytecode VM's `maybe_pre_eval_structural` (src/backend/bytecode/vm/mod.rs).
///
/// Returns the reduced value if any reducer condition holds, otherwise the
/// original value.
///
/// # Safety
/// `ctx_ref.env_ptr` must point to a valid `MettaEnvironment` or be null.
unsafe fn jit_maybe_pre_eval_structural(ctx_ref: &JitContext, v: MettaValue) -> MettaValue {
    use crate::backend::eval::step::should_pre_eval_by_type;
    use crate::backend::eval::trampoline::dispatch_hints::is_embedded_kernel_op;
    use crate::backend::eval::trampoline::eval_loop::eval_trampoline;
    use crate::backend::eval::trampoline::EvalContext;
    use crate::backend::eval::{is_eager_special_form, is_grounded_op};
    use crate::backend::models::{global_factory, ActiveFactory};

    // Plan 3 hook H-2 (2026-05-06): cooperative GC safepoint before
    // JIT→trampoline re-entry. Same pattern as `jit_pre_eval_arg`.
    {
        // E1-c step 4 (design §Part-6): this JIT structural pre-eval poll fires for
        // ANY EvalGuard-holding thread that observes `is_gc_requested()` (the
        // dedicated driver counts every such thread, not only `IS_PARALLEL_WORKER`-
        // flagged ones). Dedicated OFF: `is_gc_requested()` is false (nothing sets
        // it), so the body never runs.
        if crate::backend::models::gc_allocator::is_gc_requested() {
            let mut roots: Vec<MettaValue> = Vec::with_capacity(64);
            roots.push(v.clone());
            crate::backend::bytecode::jit::runtime::gc_roots::collect_jit_roots_into(
                ctx_ref, &mut roots,
            );
            crate::backend::eval::trampoline::eval_loop::worker_cooperative_safepoint(&roots);
        }
    }

    let items = match v.as_sexpr() {
        Some(s) => s,
        None => return v,
    };
    let head = match items.first().and_then(|h| h.as_atom()) {
        Some(h) => h,
        None => return v,
    };
    if ctx_ref.env_ptr.is_null() {
        return v;
    }
    let env = &*(ctx_ref.env_ptr as *const crate::backend::bytecode::MettaEnvironment);

    // Plan 1 audit (2026-05-06): the tree-walker's StartChain dispatches
    // `is_embedded_kernel_op` heads through the kernel-step branch; the JIT
    // structural pre-eval must mirror this so chain-bound results from
    // map-atom/filter-atom/foldl-atom/etc. are reduced before the body is
    // evaluated. Without this addition, the JIT path leaves the unreduced
    // S-expr in place and downstream destructuring (let-pattern, freeze-tuple
    // ...) sees the literal expression — same shape as the Direct.metta bug.
    let should_reduce = head.starts_with('$')
        || is_grounded_op(head)
        || is_eager_special_form(head)
        || is_embedded_kernel_op(head)
        || should_pre_eval_by_type::<MettaValue, ActiveFactory>(head, env);
    if !should_reduce {
        return v;
    }

    // Reduce via the trampoline. Mirrors `jit_pre_eval_arg` in call_support.rs.
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
    let (results, _) = eval_trampoline(v.clone(), env.clone(), &ctx);
    results.into_iter().next().map(|(val, _)| val).unwrap_or(v)
}

/// Runtime function for StructuralHead opcode (`car-atom`).
///
/// Pops the raw (unreduced) argument, applies the 4-condition pre-eval
/// predicate against the live environment, optionally reduces, then takes
/// the head. Mirrors tree-walker Arm B-structural + bytecode VM
/// `op_structural_head` exactly.
///
/// # Arguments
/// * `ctx` - JIT context pointer (provides env_ptr and bailout state)
/// * `val` - NaN-boxed raw argument value
/// * `ip` - Instruction pointer for error reporting (unused currently)
///
/// # Returns
/// NaN-boxed head of the (optionally reduced) argument.
///
/// # Safety
/// Pointers must be valid; `ctx.env_ptr` must point to a valid env or null.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_structural_head(
    ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return TAG_UNIT,
    };
    let jit_val = JitValue::from_raw(val);
    if !jit_val.is_heap() {
        return TAG_UNIT;
    }
    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return TAG_UNIT;
    }
    let raw = MettaValue::from_inner(&*inner_ptr);
    let evaluated = jit_maybe_pre_eval_structural(ctx_ref, raw);
    match evaluated.view() {
        ValueView::SExpr(items) => {
            if items.is_empty() {
                TAG_UNIT
            } else {
                value_to_jit_generic(&items[0]).to_bits()
            }
        }
        ValueView::Quoted(_) => {
            let quote_atom = MettaValue::Atom("quote".to_string());
            metta_to_jit(&quote_atom).to_bits()
        }
        _ => TAG_UNIT,
    }
}

/// Runtime function for StructuralTail opcode (`cdr-atom`).
/// See `jit_runtime_structural_head` for semantics.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_structural_tail(
    ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return JitValue::unit().to_bits(),
    };
    let jit_val = JitValue::from_raw(val);
    if !jit_val.is_heap() {
        return JitValue::unit().to_bits();
    }
    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return JitValue::unit().to_bits();
    }
    let raw = MettaValue::from_inner(&*inner_ptr);
    let evaluated = jit_maybe_pre_eval_structural(ctx_ref, raw);
    match evaluated.view() {
        ValueView::SExpr(items) => {
            let tail: Vec<MettaValue> = if items.len() > 1 {
                items[1..].to_vec()
            } else {
                Vec::new()
            };
            value_to_jit_generic(&MettaValue::SExpr(tail)).to_bits()
        }
        ValueView::Quoted(inner) => {
            let tail = MettaValue::SExpr(vec![inner]);
            value_to_jit_generic(&tail).to_bits()
        }
        _ => JitValue::unit().to_bits(),
    }
}

/// Runtime function for GetArity opcode
///
/// Gets the arity (number of elements) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed Long containing the arity, or 0 if not an SExpr
///
/// # Safety
/// The inner pointer must be valid if val is TAG_PTR.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_arity(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return JitValue::from_long(0).to_bits();
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return JitValue::from_long(0).to_bits();
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    match metta_val.view() {
        ValueView::SExpr(items) => JitValue::from_long(items.len() as i64).to_bits(),
        ValueView::Float(_)
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Unit
        | ValueView::Empty
        | ValueView::NotReducible
        | ValueView::Atom(_)
        | ValueView::String(_)
        | ValueView::Error(_, _)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::Space(_)
        | ValueView::State(_)
        | ValueView::Memo(_)
        | ValueView::Quoted(_)
        | ValueView::Lazy(_) => JitValue::from_long(0).to_bits(),
    }
}

/// Runtime function for GetElement opcode
///
/// Gets an element at a specific index from an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `index` - Element index (0-based)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed element at index, or TAG_UNIT if out of bounds/not an SExpr
///
/// # Safety
/// The inner pointer must be valid if val is TAG_PTR.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_element(
    _ctx: *mut JitContext,
    val: u64,
    index: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return TAG_UNIT;
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return TAG_UNIT;
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    let idx = index as usize;

    match metta_val.view() {
        ValueView::SExpr(items) => {
            if idx >= items.len() {
                TAG_UNIT
            } else {
                value_to_jit_generic(&items[idx]).to_bits()
            }
        }
        ValueView::Float(_)
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Unit
        | ValueView::Empty
        | ValueView::NotReducible
        | ValueView::Atom(_)
        | ValueView::String(_)
        | ValueView::Error(_, _)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::Space(_)
        | ValueView::State(_)
        | ValueView::Memo(_)
        | ValueView::Quoted(_)
        | ValueView::Lazy(_) => TAG_UNIT,
    }
}
