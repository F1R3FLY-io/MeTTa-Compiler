//! List operations for MeTTa evaluation.
//!
//! This module handles list operations including:
//! - Generic operations: type-agnostic implementations for arena values

pub(crate) mod ops;
pub(crate) mod helpers;

// Re-export generic list operations
#[allow(unused_imports)]
pub(crate) use ops::{
    eval_car_atom_generic, eval_cdr_atom_generic, eval_cons_atom_generic, eval_decons_atom_generic,
    eval_index_atom_generic, eval_max_atom_generic, eval_min_atom_generic, eval_size_atom_generic,
};
pub(crate) use helpers::substitute_variable_generic;
