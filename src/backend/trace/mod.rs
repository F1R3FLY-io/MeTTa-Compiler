//! Evaluation trace system for MeTTaTron.
//!
//! When the `trace` feature is enabled, this module provides a
//! production-quality binary tracing system that records every rewrite,
//! type check, error, bailout, and optimization with full source location
//! provenance.
//!
//! The trace is written to a file via `--trace FILE` and can be analyzed
//! with the standalone `trace-analyzer` tool.
//!
//! When the feature is disabled, the `trace_emit!` macro compiles to
//! nothing — zero cost, no branches, no dead code.

pub mod collector;
pub mod convert;
pub mod format;
#[macro_use]
pub mod macros;
pub mod rule_match;
pub mod thread_local_sink;

#[cfg(test)]
mod tests;

pub use collector::TraceCollector;
pub use convert::{trace_bindings, trace_span, trace_value, trace_value_generic};
pub use format::{write_event, write_header, write_footer};
pub use rule_match::{emit_match_attempt, rule_match_filter, LiveOutcome, RuleMatchFilter};
pub use thread_local_sink::{set_thread_trace_collector, clear_thread_trace_collector, with_thread_trace_collector, set_thread_trace_collector_ref, with_trace_collector_ref};

// Re-export shared format types for convenience.
pub use trace_format::{
    RuleMatchOutcome, TraceEvent, TraceEventKind, TraceHeader, TraceSpan, TraceTier, TraceValue,
    TRACE_MAGIC, TRACE_FORMAT_VERSION,
};
