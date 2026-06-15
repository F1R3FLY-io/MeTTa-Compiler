pub mod adaptive_pool;
pub mod bindings;
pub mod gc_allocator;
pub mod gc_cron;
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
// These items are the single place where the evaluator's value factory/store
// are named. The whole `src/backend/eval/**` tree threads `ActiveFactory` /
// `ActiveStore` / `active_factory()` instead of the concrete types, so the
// evaluator is store-agnostic at the type level. They resolve to the index
// arena store (`src/backend/eval/cesk/index_heap.rs`), the only store.

/// The active value factory for the evaluator: the index-arena store's alloc
/// interface `IndexFactory` (`src/backend/eval/cesk/index_heap.rs`).
pub type ActiveFactory = crate::backend::eval::cesk::index_heap::IndexFactory;

/// The active `Store` impl: the index heap store σ, `IndexHeapStore`.
pub type ActiveStore = crate::backend::eval::cesk::index_heap::IndexHeapStore;

/// Get the active value factory instance. Delegates to `global_factory()`
/// (the index `IndexFactory`).
#[inline]
pub fn active_factory() -> ActiveFactory {
    global_factory()
}

/// The GC value-store compiled into this binary. The slab store has been
/// decommissioned; the index arena store is the only store, so this is always
/// `"index"`. Retained as the single source of truth for the `--gc` reporter.
#[inline]
pub fn compiled_gc_store() -> &'static str {
    "index"
}

/// Resolve a `--gc` / `MTT_GC` request against the compiled store. The index
/// arena is the only store, so `--gc index` / `--gc auto` / empty / absent
/// always succeed; `--gc slab` hard-errors (the slab store is decommissioned);
/// any other value is rejected. Returns the compiled store name (`"index"`) on
/// success.
pub fn assert_gc_request(request: Option<&str>) -> Result<&'static str, String> {
    let compiled = compiled_gc_store();
    if let Some(raw) = request {
        let req = raw.trim().to_ascii_lowercase();
        match req.as_str() {
            "" | "auto" => {}
            r if r == compiled => {}
            "slab" => {
                return Err(format!(
                    "--gc=slab requested, but this binary was compiled with the \
                     '{compiled}' GC store; the slab store has been decommissioned — \
                     rebuild with default features to use index-gc, the only \
                     supported store."
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

#[cfg(test)]
mod gc_request_tests {
    use super::{assert_gc_request, compiled_gc_store};

    /// F2 (the --gc/MTT_GC assert + reporter): the resolved request must match
    /// the compiled store (`"index"`, the only store) or hard-error BEFORE
    /// evaluation. The tests derive expectations from `compiled_gc_store()`.
    #[test]
    fn matching_request_passes_and_reports_the_compiled_store() {
        let compiled = compiled_gc_store();
        assert_eq!(assert_gc_request(Some(compiled)), Ok(compiled));
    }

    #[test]
    fn auto_empty_and_absent_requests_always_pass() {
        let compiled = compiled_gc_store();
        assert_eq!(assert_gc_request(Some("auto")), Ok(compiled));
        assert_eq!(assert_gc_request(Some("")), Ok(compiled));
        assert_eq!(assert_gc_request(Some("  ")), Ok(compiled));
        assert_eq!(assert_gc_request(None), Ok(compiled));
    }

    #[test]
    fn request_is_case_insensitive_and_trimmed() {
        let compiled = compiled_gc_store();
        let shouty = compiled.to_ascii_uppercase();
        assert_eq!(assert_gc_request(Some(shouty.as_str())), Ok(compiled));
        let padded = format!("  {compiled}  ");
        assert_eq!(assert_gc_request(Some(padded.as_str())), Ok(compiled));
    }

    #[test]
    fn the_other_store_is_a_hard_error_with_a_rebuild_hint() {
        let compiled = compiled_gc_store();
        let other = "slab"; // the only non-matching value for the index-only store
        let err = assert_gc_request(Some(other)).expect_err("store mismatch must hard-error");
        assert!(
            err.contains(&format!("--gc={other} requested")),
            "names the rejected request: {err}"
        );
        assert!(
            err.contains(&format!("compiled with the '{compiled}' GC store")),
            "names the compiled store: {err}"
        );
        assert!(err.contains("rebuild with"), "carries the rebuild hint: {err}");
    }

    #[test]
    fn default_index_rejects_slab_with_decommission_hint() {
        let err = assert_gc_request(Some("slab"))
            .expect_err("default index binary must reject a slab assertion");
        assert!(
            err.contains("compiled with the 'index' GC store"),
            "names the compiled default-index store: {err}"
        );
        assert!(
            err.contains("the slab store has been decommissioned"),
            "tells slab callers the store is gone: {err}"
        );
        assert!(
            !err.contains("without `index-gc`"),
            "must not describe impossible subtractive Cargo features: {err}"
        );
    }

    #[test]
    fn unknown_values_are_rejected_with_the_expected_set() {
        let err = assert_gc_request(Some("bogus")).expect_err("unknown value must error");
        assert!(
            err.contains("unknown --gc value 'bogus'") && err.contains("slab | index | auto"),
            "names the value and the accepted set: {err}"
        );
    }
}
