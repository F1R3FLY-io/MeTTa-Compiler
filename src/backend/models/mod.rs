pub mod bindings;
pub mod gc_allocator;
pub mod gc_cron;
pub mod gc_thread;
pub mod generic_bindings;
pub mod memo_handle;
pub mod metta_state;
pub mod metta_value;
pub mod metta_value_trait;
pub mod space_handle;

pub use metta_value::{numeric_equal, numeric_equal_generic, numeric_not_equal, MettaValue, MettaValueInner};
pub use gc_allocator::{
    active_evaluator_count, alloc_count_snapshot,
    apply_backpressure_tier1, apply_backpressure_tier2,
    backpressure_level, collect_all_roots, committed_bytes_snapshot,
    current_context_id, disable_gc,
    drop_eval_guard_for_safepoint, gc_cycle_in_flight,
    global_allocator, global_factory, global_gc_cron, global_gc_thread,
    init_global_allocator, is_gc_disabled, is_gc_requested,
    maybe_process_gc_response, maybe_quiescent_gc,
    reacquire_eval_guard_after_safepoint,
    register_root_provider, register_temporary_roots, release_session,
    request_gc, set_backpressure_level,
    trigger_gc_cycle, try_register_env_roots,
    EvalGuard, GcFactory, RootProvider, SafepointRootHandle,
    SessionGuard, SlabAllocator, MAX_BACKPRESSURE,
};
pub use gc_cron::{GcCronSingleton, CronHandle};
pub use bindings::SmartBindings as Bindings;
pub use generic_bindings::{GenericBindings, GenericBindingsIter};
pub use memo_handle::MemoHandle;
pub use metta_state::MettaState;
pub use metta_value::{escape_json, serialize_tags};
pub use metta_value_trait::{MettaValueTrait, MettaValueFactory};
pub use space_handle::{GenericMultiplicityMatch, SpaceHandle};

use crate::backend::environment::MettaEnvironment;

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, MettaEnvironment);
