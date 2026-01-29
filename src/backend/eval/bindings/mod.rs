//! Variable binding and unification operations for MeTTa evaluation.
//!
//! This module provides operations for binding variables to values:
//! - let: Basic variable binding with pattern matching
//! - let*: Sequential bindings
//! - unify: Pattern unification with success/failure branches
//! - sealed: Create locally scoped variables
//! - atom-subst: Variable substitution through pattern matching

mod let_forms;
mod unify;

#[cfg(test)]
mod tests;

// Re-export for use by parent eval module
pub(crate) use let_forms::{eval_let_star_step, eval_let_step};
// Keep deprecated versions accessible for internal use
#[allow(unused_imports)]
pub(crate) use let_forms::{eval_let, eval_let_star, pattern_mismatch_suggestion};
#[allow(unused_imports)]
pub(crate) use unify::eval_unify;
pub(crate) use unify::{eval_atom_subst, eval_sealed, eval_unify_step};
