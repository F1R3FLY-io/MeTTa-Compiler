//! List operations for MeTTa evaluation.
//!
//! This module handles list operations including:
//! - Basic operations: car-atom, cdr-atom, cons-atom, decons-atom, size-atom, max-atom
//! - Higher-order operations: map-atom, filter-atom, foldl-atom

// Note: The require_args_with_usage! macro is available from the parent module's
// #[macro_use] mod macros; declaration

mod basic;
mod helpers;
mod higher_order;

#[cfg(test)]
mod tests;

// Re-export all public functions
pub(crate) use basic::{
    eval_car_atom, eval_cdr_atom, eval_cons_atom, eval_decons_atom, eval_max_atom, eval_size_atom,
};
pub(crate) use higher_order::{eval_filter_atom, eval_foldl_atom, eval_map_atom};
