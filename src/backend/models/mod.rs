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
    apply_backpressure_tier2, backpressure_level, collect_all_roots, committed_bytes_snapshot,
    current_context_id, disable_gc, drop_eval_guard_for_safepoint, gc_cycle_in_flight,
    gc_sweep_epoch, global_allocator, global_factory, global_gc_cron, init_global_allocator,
    is_gc_disabled, is_gc_requested, maybe_process_gc_response, maybe_quiescent_gc,
    reacquire_eval_guard_after_safepoint, register_root_provider, register_temporary_roots,
    release_session, request_gc, set_backpressure_level, trigger_gc_cycle, try_register_env_roots,
    EvalGuard, GcFactory, GcHoldGuard, RootProvider, SafepointRootHandle, SessionGuard,
    SlabAllocator, MAX_BACKPRESSURE,
};
pub use gc_cron::{CronHandle, GcCronSingleton};
pub use generic_bindings::{
    allocate_scope_id, BindingName, GenericBindings, GenericBindingsFullIter, GenericBindingsIter,
    ScopeId, ROOT_SCOPE,
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
