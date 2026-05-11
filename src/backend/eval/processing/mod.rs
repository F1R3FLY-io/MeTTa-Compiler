//! Result Processing
//!
//! This module handles processing of evaluation results, including
//! generic collection and combination processing.

pub mod ops;

#[allow(unused_imports)]
pub use ops::{
    cartesian_product_lazy_generic, process_collected_sexpr_generic,
    process_single_combination_generic, GenericCartesianProductIter, GenericCartesianProductResult,
    GenericProcessedSExpr,
};
