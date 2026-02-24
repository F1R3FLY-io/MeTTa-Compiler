//! Step-based Evaluation
//!
//! This module contains the functions and types for performing single evaluation
//! steps in the trampoline-based evaluator.
//!
//! ## Generic Step Functions
//!
//! The `generic_step` and `generic_sexpr` modules provide generic versions of the
//! step evaluation functions that work with any value type implementing `MettaValueTrait`.

mod generic_sexpr;
mod generic_step;
mod generic_types;
mod grounded;

#[allow(unused_imports)]
pub use generic_sexpr::eval_sexpr_step_generic;
pub use generic_step::eval_step_generic;
#[allow(unused_imports)]
pub use generic_types::{GenericEvalStep, MemoOpType};
#[allow(unused_imports)]
pub use grounded::find_grounded_arg_indices_generic;
#[allow(unused_imports)]
pub use grounded::{extract_arg_types, is_meta_type};
