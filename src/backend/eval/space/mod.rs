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
pub(crate) use match_ops::eval_match;
pub(crate) use memoization::{
    eval_clear_memo, eval_memo, eval_memo_first, eval_memo_stats, eval_new_memo,
};
pub(crate) use nondeterminism::{
    eval_amb, eval_backtrack, eval_collapse, eval_collapse_bind, eval_commit, eval_get_atoms,
    eval_guard, eval_superpose,
};
pub(crate) use rules::eval_add;
pub(crate) use space_management::{eval_add_atom, eval_new_space, eval_remove_atom};
pub(crate) use state::{eval_change_state, eval_get_state, eval_new_state};
