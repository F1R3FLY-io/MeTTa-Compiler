//! Plan 3 (2026-05-06) — JIT runtime root collection for cooperative GC.
//!
//! When a parallel-branch worker enters JIT-compiled code, the worker holds
//! NaN-boxed `JitValue`s (TAG_PTR / TAG_ERROR variants) on the JIT operand
//! stack, in `results`, in `saved_stack`, in choice-point alternatives, in
//! binding-frame entries, in template-results, and in the state cache.
//! These are slab-pointer payloads (`*const MettaValueInner`) that the
//! mark-sweep GC must see as roots, otherwise it will reap the slots and
//! the next JIT runtime callback will dereference freed memory.
//!
//! This module provides a walker that decodes every NaN-boxed `JitValue`
//! reachable from a `JitContext` and appends the corresponding
//! `MettaValue` to a caller-supplied `Vec<MettaValue>`.
//!
//! Inline NaN-boxed values (Bool/Long/Float/Unit/Empty), atom/var string
//! pointers, and opaque registry pointers are skipped. `TAG_PTR` and
//! `TAG_ERROR` payloads contribute directly to the root set; bytecode-chunk
//! pointers contribute their constant pools. `TAG_ATOM` and `TAG_VAR` point at
//! interned `String`s, not slab `MettaValueInner`, so they don't need rooting
//! through the slab GC (the strings are managed by `arc-interner`).
//!
//! # Safety
//!
//! All `*const MettaValueInner` payloads on the JIT stack must satisfy the
//! invariant that they were produced by `JitValue::from_inner_ptr` or
//! `from_error_ptr` (i.e., point to slab-allocated `'static` memory).
//! This invariant holds by construction in the JIT runtime.

use crate::backend::bytecode::jit::types::{
    JitAlternativeTag, JitChoicePoint, JitContext, JitValue, MAX_STACK_SAVE_VALUES, PAYLOAD_MASK,
    STACK_SAVE_POOL_SIZE, TAG_ERROR, TAG_PTR,
};
use crate::backend::eval::cesk::{ContinuationAddr, SpineStore};
use crate::backend::models::{MettaValue, MettaValueInner};

/// Walk `ctx` and append every live `MettaValue` root the JIT runtime is
/// currently holding to `out`.
///
/// # Safety
///
/// The caller must hold a unique borrow of `ctx`'s backing memory for the
/// duration of the call (no concurrent mutation). All `*const MettaValueInner`
/// payloads on the JIT stack must satisfy the invariant described above.
#[inline]
pub(crate) unsafe fn collect_jit_roots_into(ctx: &JitContext, out: &mut Vec<MettaValue>) {
    collect_constant_array_roots(ctx.constants, ctx.constants_len, out);
    collect_constant_array_roots(
        ctx.arena_constants as *const MettaValue,
        ctx.arena_constants_len,
        out,
    );
    collect_chunk_ptr_constants(ctx.current_chunk, out);

    // F1: value_stack[0..sp]
    if !ctx.value_stack.is_null() && ctx.sp > 0 {
        for i in 0..ctx.sp {
            let raw = (*ctx.value_stack.add(i)).0;
            collect_jit_value_into(JitValue::from_raw(raw), out);
        }
    }

    // F5: results[0..results_count]
    if !ctx.results.is_null() && ctx.results_count > 0 {
        for i in 0..ctx.results_count {
            let raw = (*ctx.results.add(i)).0;
            collect_jit_value_into(JitValue::from_raw(raw), out);
        }
    }

    // F8: saved_stack[0..saved_stack_count]
    if !ctx.saved_stack.is_null() && ctx.saved_stack_count > 0 {
        for i in 0..ctx.saved_stack_count {
            let raw = (*ctx.saved_stack.add(i)).0;
            collect_jit_value_into(JitValue::from_raw(raw), out);
        }
    }

    // F4: choice_points[0..choice_point_count] — bridge the native JIT buffer
    // into ContinuationAddr-backed nodes before walking alternatives. The raw
    // buffer remains the repr(C) execution ABI; the collector sees the same
    // live family as a typed CESK continuation-spine view.
    if !ctx.choice_points.is_null() && ctx.choice_point_count > 0 {
        let bridge = JitChoicePointSpineBridge::from_context(ctx);
        for cp in bridge.iter() {
            collect_choice_point_roots_into(ctx, cp, out);
        }
    }

    // F9: binding_frames[..].entries[..].value
    if !ctx.binding_frames.is_null() && ctx.binding_frames_count > 0 {
        for i in 0..ctx.binding_frames_count {
            let frame = &*ctx.binding_frames.add(i);
            if !frame.entries.is_null() && frame.entries_count > 0 {
                for j in 0..frame.entries_count {
                    let entry = &*frame.entries.add(j);
                    let raw = entry.value.0;
                    collect_jit_value_into(JitValue::from_raw(raw), out);
                }
            }
        }
    }

    // F12 (defensive): template_results[0..template_results_cap]. The
    // runtime doesn't track a populated count; stale slots default to
    // TAG_UNIT (initialized by the executor) so they decode to "skip".
    // If a previous run wrote slab pointers and the buffer wasn't
    // re-initialized, we'd over-root harmless slab values (no UAF;
    // just keeps them alive one cycle longer). Acceptable.
    if !ctx.template_results.is_null() && ctx.template_results_cap > 0 {
        for i in 0..ctx.template_results_cap {
            let raw = (*ctx.template_results.add(i)).0;
            collect_jit_value_into(JitValue::from_raw(raw), out);
        }
    }

    // F15: state_cache valid slots (8 slots; check state_cache_valid bit).
    for slot_idx in 0..ctx.state_cache.len() {
        let valid = (ctx.state_cache_valid >> slot_idx) & 1 != 0;
        if valid {
            let (_state_id, cached_bits) = ctx.state_cache[slot_idx];
            collect_jit_value_into(JitValue::from_raw(cached_bits), out);
        }
    }
}

#[derive(Debug)]
struct JitChoicePointSpineBridge {
    order: Vec<ContinuationAddr>,
    store: SpineStore<JitChoicePoint>,
}

impl JitChoicePointSpineBridge {
    fn new() -> Self {
        Self {
            order: Vec::new(),
            store: SpineStore::new(),
        }
    }

    /// Materialize the live native JIT choice-point prefix as typed
    /// continuation-spine nodes for root collection.
    ///
    /// # Safety
    /// `ctx.choice_points[0..ctx.choice_point_count]` must be readable for the
    /// duration of the call.
    unsafe fn from_context(ctx: &JitContext) -> Self {
        let mut bridge = Self::new();
        if ctx.choice_points.is_null() {
            return bridge;
        }
        let live_count = ctx.choice_point_count.min(ctx.choice_point_cap);
        bridge.order.reserve(live_count);
        for i in 0..live_count {
            bridge.push((*ctx.choice_points.add(i)).clone());
        }
        bridge
    }

    fn push(&mut self, choice_point: JitChoicePoint) {
        let addr = self.store.alloc(choice_point);
        self.order.push(addr);
    }

    fn iter(&self) -> impl Iterator<Item = &JitChoicePoint> {
        self.order.iter().map(|addr| {
            self.store
                .get(*addr)
                .expect("JIT choice-point bridge address missing from spine store")
        })
    }

    #[cfg(test)]
    fn live_node_count_for_tests(&self) -> usize {
        self.store.len()
    }
}

#[inline]
unsafe fn collect_constant_array_roots(
    constants: *const MettaValue,
    constants_len: usize,
    out: &mut Vec<MettaValue>,
) {
    if constants.is_null() || constants_len == 0 {
        return;
    }
    for i in 0..constants_len {
        out.push(*constants.add(i));
    }
}

#[inline]
unsafe fn collect_chunk_ptr_constants(chunk: *const (), out: &mut Vec<MettaValue>) {
    if chunk.is_null() {
        return;
    }
    let chunk = &*(chunk as *const crate::backend::bytecode::BytecodeChunk);
    crate::backend::bytecode::cache::collect_chunk_constants(chunk, out);
}

/// Decode a single NaN-boxed JitValue and push its slab root to `out`,
/// if the tag indicates a slab-allocated payload (`TAG_PTR` or `TAG_ERROR`).
/// Inline values (Long/Bool/Unit/Empty/Float) and atom/var string-pointer
/// values are skipped silently.
#[inline]
pub(crate) unsafe fn collect_jit_value_into(v: JitValue, out: &mut Vec<MettaValue>) {
    let tag = v.tag();
    if tag == TAG_PTR || tag == TAG_ERROR {
        // Index mode (B4): the payload is the bare arena `Addr` bits, NOT a slab
        // pointer — reconstruct the handle (do NOT deref). This makes the JIT
        // root-walker the structural index-leaf reader for `VmLeaf::Jit` (`Addr(0)`
        // is a valid arena address, so there is no null-skip in the index arm).
        if crate::backend::models::metta_value::gc_mode_is_index() {
            let addr = crate::backend::eval::cesk::index_arena::Addr::from_raw(
                (v.0 & PAYLOAD_MASK) as u32,
            );
            // exp18: TAG5 recovery — NO exemption (design v4.1 Option A): the
            // from_long fence guarantees every TAG_PTR payload is
            // inner_ptr()-packed, so the tag is present here like anywhere else.
            let tag = ((v.0 >> 32) & 0x1F) as u8;
            debug_assert!(tag <= 18, "non-inner_ptr-packed payload leak (gc_roots)");
            out.push(MettaValue::from_addr(
                addr,
                crate::backend::models::metta_value::FLAG_HAS_VARIABLES,
                tag,
            ));
        } else {
            let p = (v.0 & PAYLOAD_MASK) as *const MettaValueInner;
            if !p.is_null() {
                // SAFETY: under slab, TAG_PTR/TAG_ERROR payloads are produced
                // exclusively by `JitValue::from_inner_ptr` / `from_error_ptr`,
                // only ever called on slab-allocated 'static MettaValueInner.
                out.push(MettaValue::from_inner(&*p));
            }
        }
    }
    // Inline tags (TAG_LONG/BOOL/UNIT/EMPTY) and atom/var string-pointer
    // tags (TAG_ATOM/TAG_VAR) don't reference slab memory; skip silently.
    // QNAN-clear (Float) values also fall through silently.
}

/// Walk one `JitChoicePoint`'s saved stack pool slice and alternatives, pushing
/// slab roots to `out`.
///
/// Alternatives use a 4-byte tag enum (`JitAlternativeTag`) plus up to three
/// payloads. `Value` and `SpaceMatch` carry slab pointers as their primary
/// payload; `Chunk` and `RuleMatch` carry bytecode chunks whose constants must
/// be rooted; `Index` is a numeric index and does not contribute.
///
/// `RuleMatch.payload2` is a `*const Bindings` whose contents are owned
/// by the dispatch arena; the array length is opaque from the choice
/// point alone, and the only constructor in use today (`call_support.rs`)
/// uses the `Value` form. The `RuleMatch` form is in unused code paths.
/// `SpaceMatch.payload2`/`payload3` are `JitBindingEntry` arrays whose
/// values are also reachable through `binding_frames` (F9), so they're
/// covered by the main walk above.
#[inline]
unsafe fn collect_choice_point_roots_into(
    ctx: &JitContext,
    cp: &JitChoicePoint,
    out: &mut Vec<MettaValue>,
) {
    collect_chunk_ptr_constants(cp.saved_chunk, out);
    collect_choice_point_saved_stack_pool_roots(ctx, cp, out);
    if cp.alt_count == 0 {
        return;
    }
    for i in 0..(cp.alt_count as usize) {
        if i >= cp.alternatives_inline.len() {
            break;
        }
        let alt = &cp.alternatives_inline[i];
        match alt.tag {
            JitAlternativeTag::Value => {
                collect_jit_value_into(JitValue::from_raw(alt.payload), out);
            }
            JitAlternativeTag::SpaceMatch => {
                // `payload` is a NaN-boxed JitValue holding the template
                // `*const MettaValueInner`. Decode it.
                collect_jit_value_into(JitValue::from_raw(alt.payload), out);
            }
            JitAlternativeTag::Chunk => {
                collect_chunk_ptr_constants(alt.payload as *const (), out);
            }
            JitAlternativeTag::RuleMatch => {
                collect_chunk_ptr_constants(alt.payload as *const (), out);
                // payload2 is *const Bindings (covered by binding_frames).
            }
        }
    }
}

#[inline]
unsafe fn collect_choice_point_saved_stack_pool_roots(
    ctx: &JitContext,
    cp: &JitChoicePoint,
    out: &mut Vec<MettaValue>,
) {
    if cp.saved_stack_pool_idx < 0 || cp.saved_stack_count == 0 || ctx.stack_save_pool.is_null() {
        return;
    }

    let slot_idx = cp.saved_stack_pool_idx as usize;
    if slot_idx >= STACK_SAVE_POOL_SIZE {
        return;
    }

    let start = slot_idx.saturating_mul(MAX_STACK_SAVE_VALUES);
    if start >= ctx.stack_save_pool_cap {
        return;
    }

    let count = cp
        .saved_stack_count
        .min(MAX_STACK_SAVE_VALUES)
        .min(ctx.stack_save_pool_cap - start);
    for i in 0..count {
        let raw = (*ctx.stack_save_pool.add(start + i)).0;
        collect_jit_value_into(JitValue::from_raw(raw), out);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::backend::bytecode::jit::types::{JitAlternative, JitChoicePoint, JitContext};
    use crate::backend::bytecode::ChunkBuilder;

    fn chunk_with_constant(
        name: &str,
        value: MettaValue,
    ) -> Arc<crate::backend::bytecode::BytecodeChunk> {
        let mut builder = ChunkBuilder::new(name);
        builder.add_constant(value);
        builder.build_arc()
    }

    #[test]
    fn test_jit_choice_point_bridge_materializes_live_prefix() {
        let mut stack = vec![JitValue::unit(); 4];
        let constants: Vec<MettaValue> = Vec::new();
        let mut choice_points = vec![JitChoicePoint::default(); 3];
        let mut results = vec![JitValue::unit(); 2];
        choice_points[0].saved_ip = 10;
        choice_points[1].saved_ip = 20;
        choice_points[2].saved_ip = 30;

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.choice_point_count = 2;

        let bridge = unsafe { JitChoicePointSpineBridge::from_context(&ctx) };
        let saved_ips: Vec<u64> = bridge.iter().map(|cp| cp.saved_ip).collect();

        assert_eq!(bridge.live_node_count_for_tests(), 2);
        assert_eq!(saved_ips, vec![10, 20]);
    }

    #[test]
    fn test_collect_jit_roots_includes_choice_point_stack_save_pool() {
        use crate::backend::bytecode::jit::runtime::helpers::metta_to_jit;

        let pool_root = MettaValue::SExpr(vec![MettaValue::sym("jit-stack-save-pool-root")]);
        let mut stack = vec![JitValue::unit(); 4];
        let constants: Vec<MettaValue> = Vec::new();
        let mut choice_points = vec![JitChoicePoint::default(); 1];
        let mut results = vec![JitValue::unit(); 2];
        let mut stack_save_pool =
            vec![JitValue::unit(); STACK_SAVE_POOL_SIZE * MAX_STACK_SAVE_VALUES];

        choice_points[0].saved_stack_pool_idx = 1;
        choice_points[0].saved_stack_count = 1;
        stack_save_pool[MAX_STACK_SAVE_VALUES] = metta_to_jit(&pool_root);

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.choice_point_count = 1;
        ctx.stack_save_pool = stack_save_pool.as_mut_ptr();
        ctx.stack_save_pool_cap = stack_save_pool.len();

        let mut roots = Vec::new();
        unsafe {
            collect_jit_roots_into(&ctx, &mut roots);
        }

        assert!(
            roots.contains(&pool_root),
            "missing JIT choice-point saved stack pool root"
        );
    }

    #[test]
    fn test_collect_jit_roots_preserves_live_stack_save_pool_slots_after_later_alloc() {
        use crate::backend::bytecode::jit::runtime::helpers::metta_to_jit;

        let early_root = MettaValue::SExpr(vec![MettaValue::sym("jit-stack-save-pool-early")]);
        let later_root = MettaValue::SExpr(vec![MettaValue::sym("jit-stack-save-pool-later")]);
        let mut stack = vec![JitValue::unit(); 4];
        let constants: Vec<MettaValue> = Vec::new();
        let mut choice_points = vec![JitChoicePoint::default(); 2];
        let mut results = vec![JitValue::unit(); 2];
        let mut stack_save_pool =
            vec![JitValue::unit(); STACK_SAVE_POOL_SIZE * MAX_STACK_SAVE_VALUES];

        choice_points[0].saved_stack_pool_idx = 0;
        choice_points[0].saved_stack_count = 1;
        stack_save_pool[0] = metta_to_jit(&early_root);

        choice_points[1].saved_stack_pool_idx = 1;
        choice_points[1].saved_stack_count = 1;
        stack_save_pool[MAX_STACK_SAVE_VALUES] = metta_to_jit(&later_root);

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.choice_point_count = 2;
        ctx.stack_save_pool = stack_save_pool.as_mut_ptr();
        ctx.stack_save_pool_cap = stack_save_pool.len();
        ctx.stack_save_pool_next = 2;

        let next_slot = unsafe { ctx.stack_save_pool_alloc(1) };
        assert_eq!(next_slot, 2, "later allocation must use a fresh slot");

        let mut roots = Vec::new();
        unsafe {
            collect_jit_roots_into(&ctx, &mut roots);
        }

        assert!(
            roots.contains(&early_root),
            "earlier live choice-point stack-pool root was overwritten or skipped"
        );
        assert!(
            roots.contains(&later_root),
            "later live choice-point stack-pool root was skipped"
        );
    }

    #[test]
    fn test_collect_jit_roots_includes_constant_arrays_and_chunks() {
        let array_const = MettaValue::sym("jit-array-root");
        let current_const = MettaValue::sym("jit-current-chunk-root");
        let saved_const = MettaValue::sym("jit-saved-chunk-root");
        let alt_const = MettaValue::sym("jit-alt-chunk-root");
        let rule_const = MettaValue::sym("jit-rule-chunk-root");

        let constants = vec![array_const];
        let current_chunk = chunk_with_constant("jit-current", current_const);
        let saved_chunk = chunk_with_constant("jit-saved", saved_const);
        let alt_chunk = chunk_with_constant("jit-alt", alt_const);
        let rule_chunk = chunk_with_constant("jit-rule", rule_const);

        let mut stack = vec![JitValue::unit(); 4];
        let mut results = vec![JitValue::unit(); 4];
        let mut choice_points = vec![JitChoicePoint::default(); 1];
        choice_points[0].saved_chunk = Arc::as_ptr(&saved_chunk) as *const ();
        choice_points[0].alt_count = 2;
        choice_points[0].alternatives_inline[0] =
            JitAlternative::chunk(Arc::as_ptr(&alt_chunk) as *const ());
        choice_points[0].alternatives_inline[1] =
            JitAlternative::rule_match(Arc::as_ptr(&rule_chunk) as *const (), std::ptr::null());

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.current_chunk = Arc::as_ptr(&current_chunk) as *const ();
        ctx.choice_point_count = 1;

        let mut roots = Vec::new();
        unsafe {
            collect_jit_roots_into(&ctx, &mut roots);
        }

        for expected in [
            array_const,
            current_const,
            saved_const,
            alt_const,
            rule_const,
        ] {
            assert!(
                roots.contains(&expected),
                "missing JIT chunk/constant root: {expected:?}"
            );
        }
    }

    /// B4: `collect_jit_value_into` reconstructs the arena handle from a TAG_PTR
    /// payload in index mode (the bare `Addr` bits), NOT a slab deref — so it is the
    /// structural root-walker for `VmLeaf::Jit`. Pre-B4 it deref'd the `Addr` bits as
    /// a `*const MettaValueInner` (garbage / UAF under index where payload = addr.raw()).
    #[test]
    fn collect_jit_value_into_index_reconstructs_handle() {
        use crate::backend::bytecode::jit::runtime::helpers::metta_to_jit;
        use crate::backend::eval::cesk::index_heap::IndexFactory;
        use crate::backend::models::MettaValueFactory;

        let f = IndexFactory;
        // A heap value (ground SExpr) packs to TAG_PTR carrying its Addr bits.
        let v = f.sexpr(vec![f.atom("foo"), f.long(7)]);
        let jv = metta_to_jit(&v);
        assert_eq!(jv.tag(), TAG_PTR, "heap value packs to TAG_PTR");

        let mut out = Vec::new();
        unsafe { collect_jit_value_into(jv, &mut out) };
        assert_eq!(out.len(), 1, "TAG_PTR contributes exactly one root");
        assert_eq!(
            out[0], v,
            "the root is the reconstructed handle, NOT a slab deref of the Addr bits"
        );

        // An inline scalar (TAG_LONG) references no arena node → contributes nothing.
        out.clear();
        unsafe { collect_jit_value_into(metta_to_jit(&f.long(42)), &mut out) };
        assert!(out.is_empty(), "inline scalar is not a GC root");
    }
}
