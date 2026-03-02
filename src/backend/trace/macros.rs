//! Zero-cost trace emission macros.
//!
//! `trace_emit!` compiles to nothing when the `eval-trace` feature is
//! disabled, ensuring zero overhead in production builds.

/// Emit a trace event if a `TraceCollector` is available on the context.
///
/// When `eval-trace` is disabled, this macro expands to nothing.
///
/// # Arguments
///
/// * `$collector` - An `Option<&TraceCollector>` (typically from `ctx.trace_collector()`)
/// * `$tier` - A `TraceTier` variant
/// * `$depth` - Trampoline eval depth (`u32`)
/// * `$input` - The expression before rewrite (impl `Into<TraceValue>` or `&MettaValue`)
/// * `$outputs` - The expression(s) after rewrite (`Vec<TraceValue>`)
/// * `$expr_span` - Source span of the expression (`Option<TraceSpan>`)
/// * `$kind` - A `TraceEventKind` variant describing the rewrite
#[macro_export]
macro_rules! trace_emit {
    ($collector:expr, $tier:expr, $depth:expr, $input:expr, $outputs:expr, $expr_span:expr, $kind:expr) => {
        if let Some(tc) = $collector {
            tc.emit($tier, $depth, $input, $outputs, $expr_span, $kind);
        }
    };
}

/// Convenience: emit only if the feature is enabled and a collector exists.
/// This variant takes an EvalContext and extracts the collector automatically.
#[macro_export]
macro_rules! trace_emit_ctx {
    ($ctx:expr, $tier:expr, $depth:expr, $input:expr, $outputs:expr, $expr_span:expr, $kind:expr) => {
        #[cfg(feature = "eval-trace")]
        {
            if let Some(tc) = $ctx.trace_collector() {
                tc.emit($tier, $depth, $input, $outputs, $expr_span, $kind);
            }
        }
    };
}

/// Zero-cost timed emission macro (duration + span ID).
///
/// Emits a timed trace event with explicit start timestamp, duration,
/// and optional span correlation ID. Compiles to nothing when the
/// `eval-trace` feature is disabled.
///
/// # Arguments
///
/// * `$ctx`         - An `EvalContext` (has `trace_collector()`)
/// * `$tier`        - A `TraceTier` variant
/// * `$depth`       - Trampoline eval depth (`u32`)
/// * `$input`       - Pre-converted `TraceValue`
/// * `$outputs`     - `Vec<TraceValue>`
/// * `$expr_span`   - `Option<TraceSpan>`
/// * `$kind`        - A `TraceEventKind` variant
/// * `$start_ns`    - Start timestamp (from `tc.elapsed_ns()`)
/// * `$duration_ns` - `Option<u64>` duration
/// * `$span_id`     - `Option<u64>` span correlation ID
#[macro_export]
macro_rules! trace_emit_timed_ctx {
    ($ctx:expr, $tier:expr, $depth:expr, $input:expr, $outputs:expr, $expr_span:expr, $kind:expr, $start_ns:expr, $duration_ns:expr, $span_id:expr) => {
        #[cfg(feature = "eval-trace")]
        {
            if let Some(tc) = $ctx.trace_collector() {
                tc.emit_timed(
                    $tier,
                    $depth,
                    $input,
                    $outputs,
                    $expr_span,
                    $kind,
                    $start_ns,
                    $duration_ns,
                    $span_id,
                );
            }
        }
    };
}
