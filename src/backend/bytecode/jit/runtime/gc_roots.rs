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
    JitAlternativeTag, JitChoicePoint, JitContext, JitValue, PAYLOAD_MASK, TAG_ERROR, TAG_PTR,
};
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

    // F4: choice_points[0..choice_point_count] — alternatives + saved-stack
    // pool (saved_stack already covered above; pool entries reach via cp.idx)
    if !ctx.choice_points.is_null() && ctx.choice_point_count > 0 {
        for i in 0..ctx.choice_point_count {
            let cp = &*ctx.choice_points.add(i);
            collect_choice_point_roots_into(cp, out);
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
        let p = (v.0 & PAYLOAD_MASK) as *const MettaValueInner;
        if !p.is_null() {
            // SAFETY: TAG_PTR/TAG_ERROR payloads are produced exclusively
            // by `JitValue::from_inner_ptr` / `from_error_ptr`, which are
            // only ever called on slab-allocated 'static MettaValueInner.
            out.push(MettaValue::from_inner(&*p));
        }
    }
    // Inline tags (TAG_LONG/BOOL/UNIT/EMPTY) and atom/var string-pointer
    // tags (TAG_ATOM/TAG_VAR) don't reference slab memory; skip silently.
    // QNAN-clear (Float) values also fall through silently.
}

/// Walk one `JitChoicePoint`'s alternatives and push slab roots to `out`.
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
unsafe fn collect_choice_point_roots_into(cp: &JitChoicePoint, out: &mut Vec<MettaValue>) {
    collect_chunk_ptr_constants(cp.saved_chunk, out);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::backend::bytecode::jit::types::{JitAlternative, JitChoicePoint};
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
}
