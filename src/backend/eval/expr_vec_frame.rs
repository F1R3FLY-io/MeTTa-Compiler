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
//! label); it lives in [`super::frame_label`] so this signature stayed stable
//! after A5.6 routed root discovery off the (now-deleted) `frame_chain`.
//!
//! ## Why this is unconditional, not a runtime `gc_mode_is_index()` gate
//!
//! This replaced the runtime `gc_mode_is_index()` gate inside `maybe_push_frame`
//! with a compile-time split, and F4 then deleted the slab arm entirely. Sound
//! because: runtime `--gc` / `MTT_GC` requests are assertions rather than mode
//! switches, and the value factory is the compile-time `IndexFactory` — so the
//! old slab `frame_chain` path was statically unreachable and is now gone.
//!
//! A5.0 adds this helper but does not yet wire the 11 call sites
//! (modules.rs ×2 + testing_ops.rs ×9); A5.2 wired those and removed the
//! module-level `allow(dead_code)`.

use super::frame_label::FrameLabel;
use crate::backend::models::MettaValue;

pub(crate) use index_gc::push_expr_vec_frame;

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

