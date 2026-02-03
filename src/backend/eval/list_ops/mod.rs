//! List operations for MeTTa evaluation.
//!
//! This module handles list operations including:
//! - Basic operations: car-atom, cdr-atom, cons-atom, decons-atom, size-atom, max-atom
//! - Higher-order operations: map-atom, filter-atom, foldl-atom
//! - Generic operations: type-agnostic implementations for both heap and arena values

// Note: The require_args_with_usage! macro is available from the parent module's
// #[macro_use] mod macros; declaration

mod basic;
pub(crate) mod generic;
pub(crate) mod helpers;
mod higher_order;

#[cfg(test)]
mod tests;

// Re-export all public functions
#[allow(unused_imports)]
pub(crate) use basic::{
    eval_car_atom, eval_cdr_atom, eval_cons_atom, eval_decons_atom, eval_max_atom, eval_size_atom,
};
#[allow(unused_imports)]
pub(crate) use generic::{
    eval_car_atom_generic, eval_cdr_atom_generic, eval_cons_atom_generic, eval_decons_atom_generic,
    eval_index_atom_generic, eval_max_atom_generic, eval_min_atom_generic, eval_size_atom_generic,
};
pub(crate) use helpers::substitute_variable_generic;
pub(crate) use higher_order::{eval_filter_atom_step, eval_foldl_atom_step, eval_map_atom_step};
