//! Space operations for MeTTa evaluation.
//!
//! This module handles operations on spaces including:
//! - Rule definition (`=`)
//! - Pattern matching (`match`)
//! - Space management (new-space, add-atom, remove-atom)
//! - Nondeterminism (collapse, superpose, amb, guard, etc.)
//! - State operations (new-state, get-state, change-state!)
//! - Memoization (new-memo, memo, memo-first, etc.)

// Note: The require_args_with_usage! macro is available from the parent module's
// #[macro_use] mod macros; declaration

mod helpers;
mod match_ops;
mod memoization;
mod nondeterminism;
mod rules;
mod space_management;
mod state;

#[cfg(test)]
mod tests;

// Re-export all public functions
pub(crate) use match_ops::eval_match_step;
pub(crate) use memoization::{
    eval_clear_memo_step, eval_memo_first_step, eval_memo_stats_step, eval_memo_step,
    eval_new_memo_step,
};
pub(crate) use nondeterminism::{
    eval_amb_step, eval_backtrack, eval_collapse_bind_step, eval_collapse_step, eval_commit,
    eval_get_atoms_step, eval_guard_step, eval_superpose,
};
pub(crate) use rules::eval_add;
pub(crate) use space_management::{eval_add_atom_step, eval_new_space, eval_remove_atom_step};
pub(crate) use state::{eval_change_state_step, eval_get_state_step, eval_new_state_step};
