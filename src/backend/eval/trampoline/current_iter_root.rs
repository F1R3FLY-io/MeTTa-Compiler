//! Per-thread current-iteration GC root provider.
//!
//! ## Why this exists
//!
//! The GC is **purely asynchronous** by mandate. The mark-sweep cycle runs on
//! the `AdaptiveGcPool` workers and reads its root set from the global
//! `ROOT_REGISTRY` (see `gc_allocator::register_root_provider` and
//! `collect_all_roots`). Each `RootProvider` lives in a `Weak<dyn RootProvider>`;
//! the GC walks them with `Acquire` ordering at snapshot time.
//!
//! In a purely-async design, mutator threads MUST NEVER block waiting for GC
//! progress. That rules out the older "cooperative safepoint" pattern where the
//! trampoline would drop its `EvalGuard` and call `safepoint_wait_for_quiescence`
//! so mark-sweep could fire under `ACTIVE_EVALUATORS == 0`. Phase 9 replaces
//! that dance with a per-thread `RootProvider` registered with the global
//! registry.
//!
//! The per-iteration cell closes the residual gap that the safepoint used to
//! protect: the `MettaValue` currently being evaluated between two
//! `EvalGuard` operations. Inputs and outputs of every parallel dispatch are
//! already covered by `ParallelDispatchRootProvider` /
//! `ParallelCollapseRootProvider` (Phase 6 + Phase 8). Thread-local caches
//! (`EVAL_MEMO`, `MATCH_RESULT_CACHE`) are covered by
//! `refresh_thread_local_cache_roots` (`eval/mod.rs`). The remaining hole is
//! the current iteration's value — and this provider plugs it.
//!
//! ## Cost
//!
//! One `Release`-ordered atomic store per iteration write (`store_current_iter`)
//! and one `Acquire`-ordered atomic load per GC root walk (`collect_roots`).
//! No allocation, no Mutex, no condvar. The `Arc` is created once per thread
//! (lazy on first use) and registered as a `Weak` in the registry; when the
//! thread exits, the `Arc` drops, and the `Weak` is auto-pruned on the next
//! root walk.
//!
//! ## Safety
//!
//! `MettaValue` is `#[derive(Clone, Copy)]` with a single `tagged: usize` field
//! (`pub(crate)`). Reading/writing `tagged` as a `usize` is atomic and
//! reconstructs an identical `MettaValue`. The sentinel value `0` is never a
//! valid `MettaValue` (slab pointers are 16-byte aligned, so the lowest
//! non-zero slab `tagged` is `0x10`; inline NaN-boxed values have the NaN tag
//! `>= 0x7FF8` in the upper 16 bits, never all-zero), so `0` safely encodes
//! "no current value".

use std::cell::OnceCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::backend::models::gc_allocator::{register_root_provider, RootProvider};
use crate::backend::models::MettaValue;

/// Per-thread current-iteration root provider. See module docs.
pub struct CurrentIterRootProvider {
    /// Tagged value mirror of `MettaValue.tagged`. `0` = no current value.
    /// Written by the owning thread (`store` / `clear`) with `Release`;
    /// read by the GC pool worker (`collect_roots`) with `Acquire`.
    tagged: AtomicUsize,
}

impl CurrentIterRootProvider {
    pub fn new() -> Self {
        Self {
            tagged: AtomicUsize::new(0),
        }
    }

    /// Store the current MettaValue. Cheap: one atomic store.
    #[inline]
    pub fn store(&self, v: MettaValue) {
        self.tagged.store(v.tagged, Ordering::Release);
    }

    /// Clear the cell. Called when the owning thread leaves a scope where
    /// a current value is well-defined (e.g., worker closure exit).
    #[inline]
    pub fn clear(&self) {
        self.tagged.store(0, Ordering::Release);
    }
}

impl Default for CurrentIterRootProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl RootProvider for CurrentIterRootProvider {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        let raw = self.tagged.load(Ordering::Acquire);
        if raw != 0 {
            // SAFETY: `tagged` was last written by `store(v)` where
            // `v: MettaValue { tagged: raw }`. MettaValue is Copy with a
            // single `tagged: usize` field; reconstructing it from `raw`
            // produces an identical value.
            roots.push(MettaValue { tagged: raw });
        }
    }
}

impl std::fmt::Debug for CurrentIterRootProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurrentIterRootProvider")
            .field(
                "tagged",
                &format_args!("0x{:x}", self.tagged.load(Ordering::Relaxed)),
            )
            .finish()
    }
}

thread_local! {
    /// One `CurrentIterRootProvider` per thread, lazy-initialized.
    ///
    /// On first `store` / `clear` call, allocates the provider and registers
    /// it with the global `ROOT_REGISTRY`. The registry stores a `Weak`; when
    /// the thread exits and this `OnceCell` drops, the `Arc` refcount goes to
    /// zero, the provider drops, and the `Weak` is auto-pruned on the next
    /// `collect_all_roots` walk.
    static CURRENT_ITER_CELL: OnceCell<Arc<CurrentIterRootProvider>> = const { OnceCell::new() };
}

/// Get (lazily create + register) this thread's current-iter root provider.
fn get_or_init() -> Arc<CurrentIterRootProvider> {
    CURRENT_ITER_CELL.with(|cell| {
        cell.get_or_init(|| {
            let provider = Arc::new(CurrentIterRootProvider::new());
            register_root_provider(&(Arc::clone(&provider) as Arc<dyn RootProvider>));
            provider
        })
        .clone()
    })
}

/// Store the current MettaValue for this thread's current-iter root cell.
/// Lazy-initializes the cell + registers the RootProvider on first call.
#[inline]
pub fn store_current_iter(v: MettaValue) {
    get_or_init().store(v);
}

/// Clear this thread's current-iter root cell. No-op if the cell has never
/// been initialized (the cell only matters for threads that actually
/// evaluate MettaValues).
#[inline]
pub fn clear_current_iter() {
    CURRENT_ITER_CELL.with(|cell| {
        if let Some(provider) = cell.get() {
            provider.clear();
        }
    });
}

/// RAII scope that stores a MettaValue on entry and clears on drop.
///
/// Use at worker closure boundaries (parallel-dispatch / parallel-collapse
/// workers) to ensure the current-iter cell is cleared on panic-unwind as
/// well as normal exit. `!Send` because it's tied to thread-local state.
pub struct CurrentIterScope {
    _no_send: std::marker::PhantomData<*const ()>,
}

impl CurrentIterScope {
    pub fn enter(v: MettaValue) -> Self {
        store_current_iter(v);
        Self {
            _no_send: std::marker::PhantomData,
        }
    }
}

impl Drop for CurrentIterScope {
    fn drop(&mut self) {
        clear_current_iter();
    }
}
