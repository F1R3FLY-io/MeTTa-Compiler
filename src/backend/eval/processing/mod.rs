//! Result Processing
//!
//! This module handles processing of evaluation results, including
//! generic collection, combination processing, and no-match handling.

pub mod generic;
pub mod no_match;

#[allow(unused_imports)]
pub use generic::{
    cartesian_product_lazy_generic, process_collected_sexpr_generic,
    process_single_combination_generic, GenericCartesianProductIter,
    GenericCartesianProductResult, GenericProcessedSExpr,
};
pub use no_match::handle_no_rule_match;
