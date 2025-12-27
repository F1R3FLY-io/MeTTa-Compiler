//! Result Processing
//!
//! This module handles processing of evaluation results, including
//! collection of S-expression results, combination processing, and
//! handling of no-match cases.

mod collect;
mod combination;
pub mod no_match;

pub use collect::process_collected_sexpr;
pub use combination::process_single_combination;
pub use no_match::handle_no_rule_match;
