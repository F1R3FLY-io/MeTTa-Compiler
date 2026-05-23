//! Step-based Evaluation
//!
//! This module contains the functions and types for performing single evaluation
//! steps in the trampoline-based evaluator.
//!
//! ## Generic Step Functions
//!
//! The `step` and `sexpr` modules provide generic versions of the
//! step evaluation functions that work with any value type implementing `MettaValueTrait`.

pub(crate) mod grounded;
mod sexpr;
mod step;
mod types;

#[allow(unused_imports)]
pub use grounded::find_grounded_arg_indices_generic;
#[allow(unused_imports)]
pub use grounded::{extract_arg_types, is_meta_type};
#[allow(unused_imports)]
pub use grounded::{
    extract_return_type, find_typed_arg_indices_generic, is_arrow_type, is_declared_value_type,
    is_meta_type as is_meta_type_value, should_pre_eval_by_type, validate_grounded_arg_types,
};
#[allow(unused_imports)]
pub use sexpr::eval_sexpr_step_generic;
pub use step::eval_step_generic;
#[allow(unused_imports)]
pub use types::{GenericEvalStep, MemoOpType};
