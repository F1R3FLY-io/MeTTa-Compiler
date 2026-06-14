//! A5.0 — `push_expr_vec_frame`: the cfg-select replacement for
//! `frame_chain::maybe_push_frame`. It pins a caller-held `Vec<MettaValue>` of
//! compiled module-import / assertion expressions as a GC root across a NESTED
//! `eval_trampoline` call (the RT-7 completeness item: those expressions are a
//! Rust-local `Vec` the caller iterates AFTER the inner trampoline returns, so
//! they are not yet in any C/K register).
//!
//! ## The two builds (the A5 cfg seam)
//!
//! - **index-gc**: push ONLY the typed K-spine `ExprVec` record
//!   ([`SuspendedActivation::ExprVec`](crate::backend::eval::cesk::k_spine::SuspendedActivation)).
//!   The index collector reads roots structurally from the K-spine — no
//!   `frame_chain`.
//! - **legacy slab opt-out**: push ONLY the `frame_chain` frame
//!   ([`EvalFrameGuard::push_vec`](crate::backend::eval::frame_chain::EvalFrameGuard)).
//!   The slab collector walks the frame chain.
//!
//! Both arms record a raw pointer to the SAME live `Vec`, read at collection
//! time (never cloned at push — see the k_spine staleness discipline). The two
//! arms are PROVEN root-equivalent: the K-spine `ExprVec` arm's
//! `extend_from_slice` is byte-identical to `frame_chain::collect_vec_roots`
//! (k_spine unit tests + the A4.x corpus machine-equivalence oracle). The
//! `FrameLabel` is used only by the slab arm (the index `ExprVec` needs no
//! label); it lives in [`super::frame_label`] so this signature stays stable
//! after A5.6 walls `frame_chain` to the slab build.
//!
//! ## Why compile-time `#[cfg]`, not runtime `gc_mode_is_index()`
//!
//! This replaces the runtime `gc_mode_is_index()` gate inside `maybe_push_frame`
//! with a COMPILE-TIME `#[cfg(feature = "index-gc")]` split. Sound because:
//! production never flips `GC_MODE` (only `#[cfg(test)]` modules call
//! `set_gc_mode_index` / `reset_gc_mode_slab`), and under `feature = "index-gc"`
//! the value factory is the compile-time `IndexFactory` and `GC_MODE`
//! static-inits to index — so the slab `frame_chain` path is statically
//! unreachable in the index build.
//!
//! A5.0 adds this helper but does not yet wire the 11 call sites
//! (modules.rs ×2 + testing_ops.rs ×9); A5.2 wired those and removed the
//! module-level `allow(dead_code)`.

use super::frame_label::FrameLabel;
use crate::backend::models::MettaValue;

#[cfg(feature = "index-gc")]
pub(crate) use index_gc::push_expr_vec_frame;
#[cfg(not(feature = "index-gc"))]
pub(crate) use slab_gc::push_expr_vec_frame;

#[cfg(feature = "index-gc")]
mod index_gc {
    use super::*;
    use crate::backend::eval::cesk::k_spine::{SuspendedActivation, SuspendedActivationGuard};

    /// Pin `data` (a caller-held `Vec<MettaValue>`) as a GC root on the typed
    /// K-spine for the lifetime of the returned guard (index-gc build).
    ///
    /// # Safety
    /// `data` must point to a `Vec<MettaValue>` that outlives the returned guard
    /// (the caller pins it in the same scope; LIFO drop order is required). The
    /// `label` is unused in the index build (the structural reader needs no
    /// label).
    #[inline]
    pub(crate) unsafe fn push_expr_vec_frame(
        _label: FrameLabel,
        data: *const Vec<MettaValue>,
    ) -> SuspendedActivationGuard {
        // SAFETY: forwarded caller contract (`data` outlives the guard).
        unsafe { SuspendedActivationGuard::push(SuspendedActivation::ExprVec { exprs: data }) }
    }
}

#[cfg(not(feature = "index-gc"))]
mod slab_gc {
    use super::*;
    use crate::backend::eval::cesk::k_spine::{SuspendedActivation, SuspendedActivationGuard};
    use crate::backend::eval::frame_chain::EvalFrameGuard;

    /// Pin `data` (a caller-held `Vec<MettaValue>`) as a GC root for the slab
    /// build. Byte-identical to the pre-A5.2 `frame_chain::maybe_push_frame`:
    /// push the frame_chain frame AND (when `gc_mode_is_index()` — e.g. a slab
    /// test running in index mode) the typed K-spine `ExprVec` sibling over the
    /// SAME pointer. Keeping the runtime-gated K-spine half matches the
    /// spine-guard slab arm (which keeps the runtime-gated `Spine`), so
    /// `SuspendedActivation::ExprVec` is constructed in the slab build too (no
    /// dead-variant warning). Both guards pop on drop (different thread-locals,
    /// so drop order is immaterial).
    ///
    /// # Safety
    /// `data` must point to a `Vec<MettaValue>` that outlives the returned guards
    /// (the caller pins it in the same scope; LIFO drop order is required).
    #[inline]
    pub(crate) unsafe fn push_expr_vec_frame(
        label: FrameLabel,
        data: *const Vec<MettaValue>,
    ) -> (EvalFrameGuard, Option<SuspendedActivationGuard>) {
        // SAFETY: forwarded caller contract (`data` outlives the guards).
        let frame = unsafe { EvalFrameGuard::push_vec(label, data) };
        let kspine = if crate::backend::models::metta_value::gc_mode_is_index() {
            // SAFETY: as above — `data` outlives the guard.
            Some(unsafe {
                SuspendedActivationGuard::push(SuspendedActivation::ExprVec { exprs: data })
            })
        } else {
            None
        };
        (frame, kspine)
    }
}
