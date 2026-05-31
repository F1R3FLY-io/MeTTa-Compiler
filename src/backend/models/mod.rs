pub mod adaptive_pool;
pub mod bindings;
pub mod gc_allocator;
pub mod gc_cron;
pub mod gc_pool;
pub mod gc_thread;
pub mod generic_bindings;
pub mod memo_handle;
pub mod metta_state;
pub mod metta_value;
pub mod metta_value_trait;
pub mod space_handle;
pub mod task_scheduler;
pub mod work_pool;

pub use bindings::SmartBindings as Bindings;
pub use gc_allocator::{
    active_evaluator_count, alloc_count_snapshot, apply_backpressure_tier1,
    apply_backpressure_tier2, backpressure_level, collect_safepoint_roots,
    committed_bytes_snapshot, current_context_id, disable_gc, drop_eval_guard_for_safepoint,
    gc_cycle_in_flight, gc_requests_total, gc_sweep_epoch, global_allocator, global_factory,
    global_gc_cron, init_global_allocator, is_gc_disabled, is_gc_requested,
    maybe_process_gc_response, maybe_quiescent_gc, note_worker_spawned,
    reacquire_eval_guard_after_safepoint, register_temporary_roots, release_session, request_gc,
    set_backpressure_level, try_register_env_roots, worker_ever_spawned, EvalGuard, GcFactory,
    GcHoldGuard, SafepointRootHandle, SessionGuard, SlabAllocator, MAX_BACKPRESSURE,
};
// A5.5: the registry CORE re-exports are walled to slab — these symbols no longer
// compile in the index build (the index collector reads roots structurally via
// collect_machine_roots ∪ collect_safepoint_roots). KEPT in the common arm above:
// collect_safepoint_roots, register_temporary_roots, SafepointRootHandle,
// maybe_quiescent_gc, try_register_env_roots (the A5.4 driver-transport channel).
#[cfg(not(feature = "index-gc"))]
pub use gc_allocator::{collect_all_roots, register_root_provider, trigger_gc_cycle, RootProvider};
pub use gc_cron::{CronHandle, GcCronSingleton};
pub use generic_bindings::{
    allocate_scope_id, BindingName, BindingsWithClasses, ClassData, ClassId, ClassTable,
    GenericBindings, GenericBindingsFullIter, GenericBindingsIter, MergeConflict, ScopeId,
    UnifyMode, ROOT_SCOPE,
};
pub use memo_handle::MemoHandle;
pub use metta_state::MettaState;
pub use metta_value::{escape_json, serialize_tags};
pub use metta_value::{
    numeric_equal, numeric_equal_generic, numeric_not_equal, MettaValue, MettaValueInner, ValueView,
};
pub use metta_value_trait::{MettaValueFactory, MettaValueTrait};
pub use space_handle::{GenericMultiplicityMatch, SpaceHandle};
pub use task_scheduler::TaskSchedulerSingleton;
pub use work_pool::init_thread_pools;

use smallvec::SmallVec;

use crate::backend::environment::MettaEnvironment;

/// Result of evaluation: (result, new_environment)
/// Uses SmallVec<[MettaValue; 2]> to inline up to 2 elements, avoiding heap
/// allocation for the common single-result case.
pub type EvalResult = (SmallVec<[MettaValue; 2]>, MettaEnvironment);

// ============================================================================
// GC migration indirection seam (Increment 4, sub-step 1)
// ============================================================================
//
// These three items are the single place where the evaluator's value
// factory/store are selected. The whole `src/backend/eval/**` tree threads
// `ActiveFactory`/`ActiveStore`/`active_factory()` instead of the concrete
// slab types, so flipping the store-centric GC migration to the index arena
// is a localized one-place change here — re-point these aliases at
// `IndexFactory` / `IndexHeapStore` (`src/backend/eval/cesk/index_heap.rs`).
//
// In this sub-step the aliases resolve to the existing slab types, so the
// default build is byte-identical (no cargo feature flag yet).

/// The active value factory for the evaluator (Inc 4: the store-centric GC seam).
/// The slab `GcFactory` by default; under `--features index-gc` the index-arena
/// store's alloc interface `IndexFactory`. Compile-time store selection — NOT a
/// runtime mode flag inside the factory.
#[cfg(not(feature = "index-gc"))]
pub type ActiveFactory = GcFactory;
#[cfg(feature = "index-gc")]
pub type ActiveFactory = crate::backend::eval::cesk::index_heap::IndexFactory;

/// The active `Store` impl: `SlabStore` by default, `IndexHeapStore` (the store σ)
/// under `--features index-gc`.
#[cfg(not(feature = "index-gc"))]
pub type ActiveStore = crate::backend::eval::cesk::store::SlabStore;
#[cfg(feature = "index-gc")]
pub type ActiveStore = crate::backend::eval::cesk::index_heap::IndexHeapStore;

/// Get the active value factory instance. Delegates to `global_factory()`, which
/// is itself feature-selected (slab `GcFactory` vs index `IndexFactory`).
#[inline]
pub fn active_factory() -> ActiveFactory {
    global_factory()
}

/// The GC value-store compiled into this binary. The store is **compile-time
/// exclusive** (`ActiveStore`/`ActiveFactory` are `#[cfg]`-selected; the slab's
/// `&'static`-ptr value payload and the index arena's `Addr` payload cannot
/// coexist in one build), so this is fixed per build: `"index"` under
/// `--features index-gc`, otherwise `"slab"`.
#[inline]
pub fn compiled_gc_store() -> &'static str {
    if cfg!(feature = "index-gc") {
        "index"
    } else {
        "slab"
    }
}

/// Inc 3: resolve a `--gc` / `MTT_GC` request against the compile-time store.
///
/// Because the value model is compile-time exclusive, `--gc` is **not** a runtime
/// store switch — it is an ASSERTION that the running binary is the store the
/// caller intended, guarding against the confound of believing you tested
/// `index` while actually running a `slab` binary (or vice-versa). Returns the
/// compiled store name on success; on a concrete mismatch returns `Err(msg)` with
/// a rebuild hint (the caller hard-errors before evaluating). An `"auto"`, empty,
/// or absent request always succeeds (no assertion requested).
pub fn assert_gc_request(request: Option<&str>) -> Result<&'static str, String> {
    let compiled = compiled_gc_store();
    if let Some(raw) = request {
        let req = raw.trim().to_ascii_lowercase();
        match req.as_str() {
            "" | "auto" => {}
            r if r == compiled => {}
            "slab" | "index" => {
                let hint = if req == "index" {
                    "rebuild with `--features index-gc`"
                } else {
                    "rebuild with default features (without `index-gc`)"
                };
                return Err(format!(
                    "--gc={req} requested, but this binary was compiled with the '{compiled}' \
                     GC store. The store is compile-time exclusive — {hint}."
                ));
            }
            other => {
                return Err(format!(
                    "unknown --gc value '{other}'; expected one of: slab | index | auto"
                ));
            }
        }
    }
    Ok(compiled)
}
