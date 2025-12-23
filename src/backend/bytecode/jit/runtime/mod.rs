//! JIT Runtime Support Functions
//!
//! This module provides runtime helper functions that can be called from
//! JIT-compiled code. These handle operations that are too complex to inline
//! or require access to Rust runtime features.
//!
//! # Calling Convention
//!
//! All runtime functions use the C ABI (`extern "C"`) for stable calling
//! from generated machine code. They take raw pointers and return raw values.
//!
//! # Module Organization
//!
//! The runtime is organized into focused submodules:
//! - [`helpers`]: Core NaN-boxing and conversion helpers
//! - [`error_handling`]: Error signaling FFI functions
//! - [`arithmetic`]: Math and trigonometric operations
//! - [`stack_ops`]: Stack manipulation and debugging
//! - [`expression_ops`]: Expression manipulation (index, min, max)
//! - [`type_predicates`]: Type checking predicates (is_long, is_bool, etc.)
//! - [`type_ops`]: Type operations (get_type, check_type, assert_type)
//! - [`value_creation`]: Value creation (make_sexpr, cons_atom, make_list, make_quote)
//! - [`sexpr_ops`]: S-expression operations (get_head, get_tail, get_arity, get_element)
//! - [`nondeterminism`]: Choice points, fork/yield/collect, and dispatcher loop
//! - [`call_support`]: Function call operations (call, tail_call)
//! - [`bindings`]: Variable binding management
//! - [`pattern_matching`]: Pattern matching and unification
//! - [`space_ops`]: Space operations (add, remove, match)
//! - [`rule_dispatch`]: Rule dispatch and management
//! - [`special_forms`]: Special form evaluation (if, let, match, etc.)
//! - [`advanced_calls`]: Advanced call operations (native, external, cached)
//! - [`advanced_nondet`]: Advanced nondeterminism (cut, guard, amb)
//! - [`mork_ops`]: MORK bridge operations
//! - [`debug_meta`]: Debug and meta-level operations
//! - [`multi_value`]: Multi-value return operations
//! - [`global_ops`]: Global and closure operations
//! - [`higher_order`]: Higher-order operations (map, filter, fold)
//! - [`state_ops`]: State and heap tracking operations

// =============================================================================
// Submodules
// =============================================================================

pub mod helpers;
pub mod error_handling;
pub mod arithmetic;
pub mod stack_ops;
pub mod expression_ops;
pub mod type_predicates;
pub mod type_ops;
pub mod value_creation;
pub mod sexpr_ops;
pub mod nondeterminism;
pub mod call_support;
pub mod bindings;
pub mod pattern_matching;
pub mod space_ops;
pub mod rule_dispatch;
pub mod special_forms;
pub mod advanced_calls;
pub mod advanced_nondet;
pub mod mork_ops;
pub mod debug_meta;
pub mod multi_value;
pub mod global_ops;
pub mod higher_order;
pub mod state_ops;

#[cfg(test)]
mod tests;

// =============================================================================
// Re-exports for backward compatibility
// =============================================================================

// Error handling
pub use error_handling::{
    jit_runtime_type_error, jit_runtime_div_by_zero,
    jit_runtime_stack_overflow, jit_runtime_stack_underflow,
};

// Arithmetic
pub use arithmetic::{
    jit_runtime_pow, jit_runtime_abs, jit_runtime_signum,
    jit_runtime_sqrt, jit_runtime_log, jit_runtime_trunc,
    jit_runtime_ceil, jit_runtime_floor_math, jit_runtime_round,
    jit_runtime_sin, jit_runtime_cos, jit_runtime_tan,
    jit_runtime_asin, jit_runtime_acos, jit_runtime_atan,
    jit_runtime_isnan, jit_runtime_isinf,
};

// Stack operations
pub use stack_ops::{
    jit_runtime_push, jit_runtime_pop, jit_runtime_get_sp, jit_runtime_set_sp,
    jit_runtime_load_constant, jit_runtime_debug_print, jit_runtime_debug_stack,
};

// Expression operations
pub use expression_ops::{
    jit_runtime_index_atom, jit_runtime_min_atom, jit_runtime_max_atom,
};

// Type predicates
pub use type_predicates::{
    jit_runtime_is_long, jit_runtime_is_bool, jit_runtime_is_nil, jit_runtime_get_tag,
};

// Type operations
pub use type_ops::{
    jit_runtime_get_type, jit_runtime_check_type, jit_runtime_assert_type,
};

// Value creation
pub use value_creation::{
    jit_runtime_make_sexpr, jit_runtime_cons_atom, jit_runtime_push_uri,
    jit_runtime_make_list, jit_runtime_make_quote,
};

// S-expression operations
pub use sexpr_ops::{
    jit_runtime_push_empty, jit_runtime_get_head, jit_runtime_get_tail,
    jit_runtime_get_arity, jit_runtime_get_element,
};

// Nondeterminism
pub use nondeterminism::{
    // Choice point core
    jit_runtime_push_choice_point, jit_runtime_fail, jit_runtime_get_current_alternative,
    jit_runtime_get_results_count, jit_runtime_get_choice_point_count,
    // Fork/Yield/Collect
    jit_runtime_fork, jit_runtime_yield, jit_runtime_collect,
    // Native nondeterminism
    jit_runtime_save_stack, jit_runtime_restore_stack, jit_runtime_fork_native,
    jit_runtime_yield_native, jit_runtime_fail_native, jit_runtime_collect_native,
    jit_runtime_has_alternatives, jit_runtime_get_resume_ip,
    // Dispatcher
    JitNativeFn, execute_with_dispatcher, collect_results, execute_once,
};

// Call support
pub use call_support::{
    jit_runtime_call, jit_runtime_tail_call,
    jit_runtime_call_n, jit_runtime_tail_call_n,
};

// Bindings
pub use bindings::{
    jit_runtime_load_binding, jit_runtime_store_binding, jit_runtime_has_binding,
    jit_runtime_clear_bindings, jit_runtime_push_binding_frame, jit_runtime_pop_binding_frame,
    jit_runtime_fork_bindings, jit_runtime_restore_bindings,
    jit_runtime_free_saved_bindings, jit_runtime_saved_bindings_size,
    JitSavedBindings,
};

// Pattern matching
pub use pattern_matching::{
    jit_runtime_pattern_match, jit_runtime_pattern_match_bind,
    jit_runtime_match_arity, jit_runtime_match_head,
    jit_runtime_unify, jit_runtime_unify_bind,
};
pub(crate) use pattern_matching::pattern_matches_impl;

// Space operations
pub use space_ops::{
    jit_runtime_space_add, jit_runtime_space_remove, jit_runtime_space_get_atoms,
    jit_runtime_space_match, jit_runtime_space_match_nondet,
    jit_runtime_resume_space_match, jit_runtime_free_space_match_alternatives,
};

// Rule dispatch
pub use rule_dispatch::{
    jit_runtime_dispatch_rules, jit_runtime_try_rule, jit_runtime_next_rule,
    jit_runtime_commit_rule, jit_runtime_fail_rule, jit_runtime_lookup_rules,
    jit_runtime_apply_subst, jit_runtime_define_rule,
};
pub(crate) use rule_dispatch::{hash_string, collect_bindings_from_ctx};

// Special forms
pub use special_forms::{
    jit_runtime_eval_if, jit_runtime_eval_let, jit_runtime_eval_let_star,
    jit_runtime_eval_match, jit_runtime_eval_case, jit_runtime_eval_chain,
    jit_runtime_eval_quote, jit_runtime_eval_unquote, jit_runtime_eval_eval,
    jit_runtime_eval_bind, jit_runtime_eval_new, jit_runtime_eval_collapse,
    jit_runtime_eval_superpose, jit_runtime_eval_memo, jit_runtime_eval_memo_first,
    jit_runtime_eval_pragma, jit_runtime_eval_function, jit_runtime_eval_lambda,
    jit_runtime_eval_apply,
};

// Advanced calls
pub use advanced_calls::{
    jit_runtime_call_native, jit_runtime_call_external, jit_runtime_call_cached,
};

// Advanced nondeterminism
pub use advanced_nondet::{
    jit_runtime_cut, jit_runtime_enter_cut_scope, jit_runtime_exit_cut_scope,
    jit_runtime_guard, jit_runtime_amb, jit_runtime_commit, jit_runtime_backtrack,
    jit_runtime_begin_nondet, jit_runtime_end_nondet,
};

// MORK operations
pub use mork_ops::{
    jit_runtime_mork_lookup, jit_runtime_mork_match,
    jit_runtime_mork_insert, jit_runtime_mork_delete,
};

// Debug and meta operations
pub use debug_meta::{
    jit_runtime_trace, jit_runtime_breakpoint,
    jit_runtime_get_metatype, jit_runtime_bloom_check,
};

// Multi-value return
pub use multi_value::{
    jit_runtime_return_multi, jit_runtime_collect_n,
};

// Global and closure operations
pub use global_ops::{
    jit_runtime_load_global, jit_runtime_store_global,
    jit_runtime_load_space, jit_runtime_load_grounded_space,
    jit_runtime_load_upvalue,
    grounded_space_index, is_grounded_ref,
};

// Higher-order operations
pub use higher_order::{
    jit_runtime_decon_atom, jit_runtime_repr,
    jit_runtime_map_atom, jit_runtime_filter_atom, jit_runtime_foldl_atom,
};

// State and heap operations
pub use state_ops::{
    jit_runtime_track_heap, jit_runtime_cleanup_heap, jit_runtime_heap_count,
    jit_runtime_new_state, jit_runtime_get_state, jit_runtime_change_state,
};

// =============================================================================
// Re-export helpers for internal use
// =============================================================================

pub(crate) use helpers::{
    extract_long_signed, box_long, metta_to_jit, metta_to_jit_tracked,
    make_jit_error, make_jit_error_with_details,
};

// Re-export constants for submodules
pub(crate) use super::types::MAX_ALTERNATIVES_INLINE;
