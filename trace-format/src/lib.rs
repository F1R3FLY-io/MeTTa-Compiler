//! Shared binary trace format types for MeTTaTron evaluation tracing.
//!
//! This crate defines the data model for MeTTaTron evaluation traces.
//! It has no dependency on mettatron itself, allowing the trace-analyzer
//! tool to read trace files without linking the full evaluator.
//!
//! All types derive `serde::{Serialize, Deserialize}` and are serialized
//! with bincode 2 for fast, compact binary encoding.

pub use postcard;
use serde::{Deserialize, Serialize};

/// Magic bytes identifying a MeTTaTron trace file (format v3).
pub const TRACE_MAGIC: [u8; 8] = *b"MTRACE\x00\x03";

/// Magic bytes for format v1 (accepted by reader for backward compatibility).
pub const TRACE_MAGIC_V1: [u8; 8] = *b"MTRACE\x00\x01";

/// Magic bytes for format v2 (accepted by reader for backward compatibility).
pub const TRACE_MAGIC_V2: [u8; 8] = *b"MTRACE\x00\x02";

/// Current trace format version.
pub const TRACE_FORMAT_VERSION: u32 = 3;

/// Serialize a value to postcard bytes.
pub fn serialize<T: serde::Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).expect("trace serialization should not fail")
}

/// Deserialize a value from postcard bytes.
pub fn deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

/// Compact source span for serialization (no pointer, no lifetime).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceSpan {
    /// Index into `TraceHeader::file_table`.
    pub file_id: u16,
    pub start_row: u32,
    pub start_col: u32,
    pub end_row: u32,
    pub end_col: u32,
}

/// Owned snapshot of a MeTTa value for trace serialization.
///
/// This is a fully-owned deep copy — no slab references, no lifetimes.
/// Created at trace-emission time via `trace_value()` so that the GC
/// can freely reclaim the original `MettaValue` afterwards.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TraceValue {
    Atom(String),
    Bool(bool),
    Long(i64),
    Float(f64),
    String(String),
    SExpr(Vec<TraceValue>),
    Unit,
    Error(String, Box<TraceValue>),
    Type(Box<TraceValue>),
    Empty,
    Quoted(Box<TraceValue>),
}

/// Kind of tabling decision for the TablingDecision trace event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TablingDecisionKind {
    /// True cycle detected (expression on its own call stack).
    CycleDetected,
    /// Cache hit — returning previously computed Complete results.
    CacheHit,
    /// Cache miss — first evaluation, marking active.
    CacheMiss,
    /// Evaluation complete — storing results in cache.
    CompleteStore,
}

impl std::fmt::Display for TablingDecisionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CycleDetected => write!(f, "cycle-detected"),
            Self::CacheHit => write!(f, "cache-hit"),
            Self::CacheMiss => write!(f, "cache-miss"),
            Self::CompleteStore => write!(f, "complete-store"),
        }
    }
}

impl std::fmt::Display for TraceValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceValue::Atom(s) => write!(f, "{s}"),
            TraceValue::Bool(b) => {
                if *b {
                    write!(f, "True")
                } else {
                    write!(f, "False")
                }
            }
            TraceValue::Long(n) => write!(f, "{n}"),
            TraceValue::Float(v) => write!(f, "{v}"),
            TraceValue::String(s) => write!(f, "\"{s}\""),
            TraceValue::SExpr(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, ")")
            }
            TraceValue::Unit => write!(f, "()"),
            TraceValue::Error(msg, details) => write!(f, "(Error {msg} {details})"),
            TraceValue::Type(inner) => write!(f, "(: {inner})"),
            TraceValue::Empty => write!(f, "%void%"),
            TraceValue::Quoted(inner) => write!(f, "(quote {inner})"),
        }
    }
}

/// Which evaluation tier produced the event.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TraceTier {
    TreeWalker = 0,
    BytecodeVM = 1,
    JitStage1 = 2,
    JitStage2 = 3,
}

impl std::fmt::Display for TraceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceTier::TreeWalker => write!(f, "TreeWalker"),
            TraceTier::BytecodeVM => write!(f, "BytecodeVM"),
            TraceTier::JitStage1 => write!(f, "JitStage1"),
            TraceTier::JitStage2 => write!(f, "JitStage2"),
        }
    }
}

/// Describes what kind of rewrite or action happened.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TraceEventKind {
    // ---- Rule Application ----
    /// A user-defined rule `(= lhs rhs)` was applied.
    RuleApplication {
        rule_lhs: TraceValue,
        rule_rhs: TraceValue,
        bindings: Vec<(String, TraceValue)>,
        /// Source location where the rule was defined.
        rule_span: Option<TraceSpan>,
    },

    // ---- Grounded Operations ----
    /// A grounded (built-in) operation was executed.
    GroundedOp {
        op_name: String,
        args: Vec<TraceValue>,
    },

    // ---- Special Forms ----
    /// A special form (`if`, `let`, `chain`, `match`, `eval`, `quote`, etc.)
    /// was dispatched or progressed through a phase.
    SpecialForm {
        form_name: String,
        /// "dispatch", "condition-eval", "then-branch", "else-branch", etc.
        phase: String,
    },

    // ---- Pattern Matching ----
    /// A pattern match was attempted against a value.
    PatternMatch {
        pattern: TraceValue,
        value: TraceValue,
        success: bool,
        bindings: Vec<(String, TraceValue)>,
    },

    /// All matching rules found for an expression.
    RuleMatchSet {
        match_count: u32,
        /// Each entry: (rule LHS, optional definition span).
        matches: Vec<(TraceValue, Option<TraceSpan>)>,
    },

    // ---- Type System ----
    /// A type system operation was performed.
    TypeOperation {
        /// "get-type", "check-type", "infer", "validate-grounded-arg"
        op: String,
        subject: TraceValue,
        result_type: Option<TraceValue>,
    },

    /// Applicative pre-evaluation of arguments before rule matching.
    ApplicativePreEval {
        operator: String,
        arg_indices: Vec<u16>,
        /// "type-driven" or "bloom-filter"
        source: String,
    },

    /// Type-driven branch pruning after rule matching.
    BranchPrune {
        expected_type: TraceValue,
        pruned_count: u32,
        surviving_count: u32,
        /// The rhs_type of each pruned match (`None` if the match had no rhs_type).
        pruned_types: Vec<Option<TraceValue>>,
    },

    // ---- Error/Exception Handling ----
    /// An error value was created.
    ErrorCreated {
        message: String,
        details: TraceValue,
    },

    /// An error was caught by an error handler.
    ErrorCaught {
        error: TraceValue,
        /// "catch", "is-error check", "grounded-op fallback"
        handler: String,
        default_used: Option<TraceValue>,
    },

    /// An error propagated through a higher-order operation.
    ErrorPropagated {
        error: TraceValue,
        /// "map-atom", "filter-atom", "foldl-atom", etc.
        context: String,
    },

    /// A grounded operation failed.
    GroundedOpError {
        op_name: String,
        /// "NoReduce", "Runtime", "Arithmetic", "IncorrectArgument"
        error_kind: String,
        message: String,
        args: Vec<TraceValue>,
    },

    // ---- Nondeterminism ----
    /// A nondeterministic fork occurred (multiple rule matches).
    NondeterministicFork { branch_count: u32 },

    /// A nondeterministic branch started evaluation.
    BranchStart {
        branch_index: u32,
        total_branches: u32,
    },

    /// A nondeterministic branch completed.
    BranchEnd {
        branch_index: u32,
        result_count: u32,
    },

    // ---- Tier Transitions ----
    /// An expression was dispatched to a specific evaluation tier.
    TierDispatch {
        expression_hash: u64,
        selected_tier: TraceTier,
        execution_count: u32,
    },

    /// A bytecode chunk was compiled for an expression.
    BytecodeCompilation {
        expression_hash: u64,
        execution_count: u32,
    },

    /// A JIT compilation was triggered for an expression.
    JitCompilation {
        expression_hash: u64,
        stage: u8,
        execution_count: u32,
    },

    // ---- JIT Bailout ----
    /// JIT execution bailed out to a lower tier.
    JitBailout {
        bailout_ip: u32,
        reason: String,
        /// "bytecode" or "tree-walker"
        fallback_tier: String,
    },

    /// Bytecode VM halted, falling back to tree-walker.
    BytecodeHalt { ip: u32, reason: String },

    // ---- Evaluation Lifecycle ----
    /// Top-level evaluation started.
    EvalStart,

    /// Top-level evaluation completed.
    EvalEnd { result_count: u32 },

    // ---- GC ----
    /// A GC safepoint was reached during evaluation.
    GcSafepoint {
        root_count: u32,
        allocation_delta_bytes: u64,
    },

    // ---- Type Inference ----
    /// Type inference determined the type(s) of an expression.
    TypeInference {
        /// The expression whose type was inferred.
        expression: TraceValue,
        /// All inferred types (nondeterministic — may have multiple).
        inferred_types: Vec<TraceValue>,
        /// Which code path produced the result (e.g., "literal-bool",
        /// "builtin-signature:{op}", "env-declared-arrow:{op}",
        /// "phase-10-inferred-arrow:{op}", "fallback-undefined", etc.).
        source: String,
    },

    /// A type match comparison was performed.
    TypeMatch {
        /// The actual type (e.g., from a rule's rhs_type).
        actual: TraceValue,
        /// The expected type (e.g., Bool from if-condition).
        expected: TraceValue,
        /// Whether the match succeeded.
        result: bool,
        /// Which matching rule decided the outcome (e.g., "expected-undefined",
        /// "exact-atom-match", "sexpr-structural-match", "no-match", etc.).
        reason: String,
    },

    /// RHS type was computed for a rule entry at insertion time.
    RhsTypeComputed {
        /// The rule's head symbol (e.g., "fact", "|-").
        head: String,
        /// The rule's arity.
        arity: u32,
        /// The LHS pattern.
        lhs: TraceValue,
        /// The RHS template.
        rhs: TraceValue,
        /// The computed rhs_type (None = %Undefined% → filtered out).
        rhs_type: Option<TraceValue>,
    },

    /// A new inferred type was registered (Phase 10.1 or 10.4).
    InferredTypeRegistered {
        /// Function name.
        function_name: String,
        /// The registered type.
        registered_type: TraceValue,
        /// "phase-10.1-rhs" or "phase-10.4-arrow".
        source: String,
    },

    // ---- Worker Pool ----
    /// A task was added to the work pool's priority queue.
    WorkPoolTaskEnqueued {
        /// "eval", "compile", or "detached"
        task_kind: String,
        /// Task priority value (5=NORMAL, 10=BACKGROUND_COMPILE)
        priority: u32,
        /// Queue depth AFTER the task was enqueued
        queue_depth: u32,
        /// Number of currently active (non-parked) workers
        active_workers: u32,
        /// Maximum configured workers
        max_workers: u32,
    },

    /// A compile task was dropped due to backpressure (queue full).
    WorkPoolTaskDropped {
        /// Always "compile"
        task_kind: String,
        /// Queue depth at drop time
        queue_depth: u32,
        /// Number of active workers
        active_workers: u32,
    },

    /// A worker finished executing a task.
    WorkPoolTaskCompleted {
        /// "eval", "compile", or "detached"
        task_kind: String,
        /// Runtime in nanoseconds
        runtime_nanos: u64,
        /// Queue depth after task completion
        queue_depth: u32,
        /// Number of active workers
        active_workers: u32,
    },

    /// Scaling monitor tick — records every decision including Hold.
    WorkPoolScaleEvent {
        // --- existing fields ---
        /// "unpark", "park", "hold", or "graduated_park"
        action: String,
        /// Number of active workers AFTER the action
        active_workers_after: u32,
        /// Min worker bound
        min_workers: u32,
        /// Max worker bound
        max_workers: u32,
        /// Current queue depth (instantaneous, not EMA)
        queue_depth: u32,
        /// EMA-smoothed throughput signal
        ema_throughput: f64,
        /// EMA-smoothed queue depth signal
        ema_queue_depth: f64,
        /// EMA-smoothed slab memory pressure signal
        ema_slab_pressure: f64,
        /// EMA-smoothed RSS memory pressure signal
        ema_rss_pressure: f64,
        /// Composite objective value J(N) from hill climber
        objective: f64,
        /// Whether this was an emergency override (bp_level >= 2)
        emergency: bool,

        // --- hill climber internals ---
        /// +1 (exploring unpark) or -1 (exploring park)
        hc_direction: i32,
        /// Ticks remaining in cooldown (0 = ready to act)
        hc_cooldown_remaining: u32,
        /// Previous objective value (baseline for comparison)
        hc_prev_objective: f64,
        /// improvement = prev_objective - objective (positive = got better)
        hc_improvement: f64,

        // --- instantaneous (pre-EMA) signals ---
        /// Raw throughput: delta_evals / elapsed_seconds
        raw_throughput: f64,
        /// Raw slab pressure: backpressure_level() as f64 (0.0-3.0)
        raw_slab_pressure: f64,
        /// Raw RSS pressure (0.0-3.0)
        raw_rss_pressure: f64,
        /// Backpressure level (0-3)
        bp_level: u32,

        // --- objective decomposition ---
        /// slab_amplifier: 1.0 (bp 0-1), 2.0 (bp 2), 4.0 (bp 3)
        slab_amplifier: f64,
        /// -THROUGHPUT_WEIGHT * ema_throughput
        term_throughput: f64,
        /// QUEUE_DEPTH_WEIGHT * queue_depth_instant
        term_queue_depth: f64,
        /// MEMORY_PRESSURE_WEIGHT * slab_amplifier * ema_slab_pressure
        term_slab_pressure: f64,
        /// RSS_PRESSURE_WEIGHT * ema_rss_pressure
        term_rss_pressure: f64,

        // --- phase 2/3 state ---
        /// Number of workers detected as blocked (Phase 2)
        blocked_worker_count: u32,
        /// Current overflow worker count
        overflow_count: u32,
        /// Which phase made the decision: "phase1_emergency", "phase4_hill_climber"
        decision_phase: String,

        // --- raw throughput inputs ---
        /// Number of evals completed since last tick
        delta_evals: u64,
        /// Seconds elapsed since last tick
        elapsed_seconds: f64,
    },

    /// A worker thread entered the parked state.
    WorkPoolWorkerParked {
        /// Worker thread index
        worker_id: u32,
        /// Queue depth when this worker parked
        queue_depth: u32,
    },

    /// A worker thread resumed from the parked state.
    WorkPoolWorkerResumed {
        /// Worker thread index
        worker_id: u32,
        /// Queue depth when this worker resumed
        queue_depth: u32,
        /// Active workers after resume
        active_workers: u32,
    },

    /// Blocked workers detected by the scaling monitor (Phase 2).
    WorkPoolBlockedWorkersDetected {
        /// Number of blocked workers
        blocked_count: u32,
        /// Number of active (non-parked) workers
        active_workers: u32,
        /// Per-worker blocked indices (only workers detected as blocked)
        blocked_indices: Vec<u32>,
    },

    /// Compensatory activation fired (Phase 3).
    WorkPoolCompensatoryAction {
        /// Workers unparked from core pool
        core_unparked: u32,
        /// Overflow workers spawned
        overflow_spawned: u32,
        /// Overflow workers drained
        overflow_drained: u32,
        /// Target active count (from hill climber)
        target: u32,
        /// Deficit = target - total_unblocked
        deficit: u32,
        /// Whether RSS vetoed overflow spawning
        rss_veto: bool,
    },

    /// Lightweight event emitted at the START of every monitor tick,
    /// carrying the raw sample values before EMA/decision processing.
    WorkPoolMonitorTick {
        /// Current global eval count
        current_eval_count: u64,
        /// Nanoseconds elapsed since last tick
        elapsed_ns: u64,
        /// Current queue length
        queue_len: u32,
        /// Current backpressure level (0-3)
        bp_level: u32,
        /// Current RSS in bytes (0 if unavailable)
        rss_bytes: u64,
    },

    /// A specific worker transitioned from unblocked to blocked (Phase 2).
    WorkPoolWorkerBlocked {
        /// Worker thread index
        worker_id: u32,
        /// CPU utilization ratio at detection time (0.0-1.0)
        cpu_ratio: f64,
    },

    /// A specific worker transitioned from blocked to unblocked (Phase 2).
    WorkPoolWorkerUnblocked {
        /// Worker thread index
        worker_id: u32,
    },

    // ---- Binding & Memoization Diagnostics ----

    /// Per-binding event in let/let* showing the pattern match result.
    LetBindingStep {
        /// The pattern being matched (e.g., `$x`, `($a $b)`).
        pattern: TraceValue,
        /// The evaluated value being matched against.
        evaluated_value: TraceValue,
        /// Whether the pattern match succeeded.
        success: bool,
        /// Resulting bindings from the pattern match.
        bindings: Vec<(String, TraceValue)>,
        /// "let" or "let*"
        form: String,
        /// For let*, which binding pair index (0-based).
        pair_index: Option<u32>,
    },

    /// Result of pre-evaluating a grounded argument (Step 2 / CollectGroundedArg).
    ArgumentPreEvalResult {
        /// Index of the argument in the parent S-expression (0-based).
        arg_index: u16,
        /// The unevaluated argument expression.
        before: TraceValue,
        /// The evaluated result.
        after: TraceValue,
        /// Whether the argument changed (false = fixpoint, returned unchanged).
        changed: bool,
    },

    /// Subgoal tabling system decision.
    TablingDecision {
        /// Hash of the expression.
        expr_hash: u64,
        /// What happened: CycleDetected, CacheHit, CacheMiss, CompleteStore
        decision: TablingDecisionKind,
        /// Number of cached results (for CacheHit and CompleteStore).
        result_count: Option<u32>,
    },

    /// When `apply_bindings` instantiates a template with bindings.
    BindingsApplied {
        /// The template before substitution.
        template: TraceValue,
        /// The bindings being applied.
        bindings: Vec<(String, TraceValue)>,
        /// The result after substitution.
        result: TraceValue,
    },

    /// When dispatching from matched rules, which rule was selected.
    RuleSelected {
        /// The selected rule's RHS.
        selected_rhs: TraceValue,
        /// Index of the selected rule among matches (0-based).
        selected_index: u32,
        /// Total number of matching rules.
        total_matches: u32,
        /// Source span of the selected rule definition.
        rule_span: Option<TraceSpan>,
    },
}

/// A single trace event — the fundamental unit of the trace log.
///
/// # Format v2 additions
///
/// `duration_ns` and `span_id` were added in format v2. For point-in-time
/// events both are `None`. For timed events, `duration_ns` carries the
/// operation's wall-clock duration. Paired begin/end events share the same
/// `span_id` for correlation.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TraceEvent {
    /// Monotonic sequence number (per-thread).
    pub seq: u64,
    /// Thread that produced this event.
    pub thread_id: u32,
    /// Nanoseconds since trace start. For timed events this is the
    /// operation's **start** time; the end time is `timestamp_ns + duration_ns`.
    pub timestamp_ns: u64,
    /// Which evaluation tier produced the event.
    pub tier: TraceTier,
    /// Trampoline evaluation depth.
    pub depth: u32,
    /// Expression BEFORE rewrite.
    pub input: TraceValue,
    /// Expression(s) AFTER rewrite.
    pub outputs: Vec<TraceValue>,
    /// Source location of the expression being rewritten.
    pub expr_span: Option<TraceSpan>,
    /// What performed the rewrite.
    pub kind: TraceEventKind,
    /// Duration in nanoseconds. `None` for point-in-time events.
    /// When `Some(d)`, the event's interval is `[timestamp_ns, timestamp_ns + d]`.
    pub duration_ns: Option<u64>,
    /// Span correlation ID for paired begin/end events.
    /// Generated by `TraceCollector::next_span_id()`.
    pub span_id: Option<u64>,
}

/// Trace file header, written once at the start of the file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TraceHeader {
    /// Source file that was evaluated.
    pub source_file: String,
    /// Nanosecond wall-clock timestamp of trace start.
    pub start_time_ns: u64,
    /// MeTTaTron version string.
    pub mettatron_version: String,
    /// Number of logical CPUs.
    pub cpu_count: u32,
    /// File path table — maps file_id (index) to file path.
    pub file_table: Vec<String>,
    /// Trace format version (self-describing). Added in v2.
    pub format_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal TraceEvent for testing.
    fn make_event(
        duration_ns: Option<u64>,
        span_id: Option<u64>,
        kind: TraceEventKind,
    ) -> TraceEvent {
        TraceEvent {
            seq: 42,
            thread_id: 7,
            timestamp_ns: 1_000_000,
            tier: TraceTier::TreeWalker,
            depth: 3,
            input: TraceValue::Atom("test-input".to_string()),
            outputs: vec![TraceValue::Long(99)],
            expr_span: Some(TraceSpan {
                file_id: 0,
                start_row: 10,
                start_col: 5,
                end_row: 10,
                end_col: 20,
            }),
            kind,
            duration_ns,
            span_id,
        }
    }

    #[test]
    fn roundtrip_event_point_in_time() {
        // Point-in-time event: no duration, no span
        let event = make_event(None, None, TraceEventKind::EvalStart);
        let bytes = serialize(&event);
        let decoded: TraceEvent = deserialize(&bytes).expect("deserialize should succeed");
        assert_eq!(event, decoded);
        assert!(decoded.duration_ns.is_none());
        assert!(decoded.span_id.is_none());
    }

    #[test]
    fn roundtrip_event_with_duration() {
        // Timed event: has duration, no span
        let event = make_event(
            Some(5_000),
            None,
            TraceEventKind::GroundedOp {
                op_name: "+".to_string(),
                args: vec![TraceValue::Long(1), TraceValue::Long(2)],
            },
        );
        let bytes = serialize(&event);
        let decoded: TraceEvent = deserialize(&bytes).expect("deserialize should succeed");
        assert_eq!(event, decoded);
        assert_eq!(decoded.duration_ns, Some(5_000));
        assert!(decoded.span_id.is_none());
    }

    #[test]
    fn roundtrip_event_with_span_id() {
        // Paired event: has span_id, may have duration
        let event = make_event(
            Some(100_000),
            Some(12345),
            TraceEventKind::EvalEnd { result_count: 3 },
        );
        let bytes = serialize(&event);
        let decoded: TraceEvent = deserialize(&bytes).expect("deserialize should succeed");
        assert_eq!(event, decoded);
        assert_eq!(decoded.duration_ns, Some(100_000));
        assert_eq!(decoded.span_id, Some(12345));
    }

    #[test]
    fn roundtrip_event_span_only() {
        // Begin event: has span_id but no duration
        let event = make_event(
            None,
            Some(9999),
            TraceEventKind::BranchStart {
                branch_index: 0,
                total_branches: 3,
            },
        );
        let bytes = serialize(&event);
        let decoded: TraceEvent = deserialize(&bytes).expect("deserialize should succeed");
        assert_eq!(event, decoded);
        assert!(decoded.duration_ns.is_none());
        assert_eq!(decoded.span_id, Some(9999));
    }

    #[test]
    fn roundtrip_header_v2() {
        let header = TraceHeader {
            source_file: "test.metta".to_string(),
            start_time_ns: 1_234_567_890,
            mettatron_version: "0.1.0-test".to_string(),
            cpu_count: 8,
            file_table: vec!["test.metta".to_string(), "lib.metta".to_string()],
            format_version: TRACE_FORMAT_VERSION,
        };
        let bytes = serialize(&header);
        let decoded: TraceHeader = deserialize(&bytes).expect("deserialize should succeed");
        assert_eq!(header, decoded);
        assert_eq!(decoded.format_version, 2);
    }

    #[test]
    fn point_event_none_fields_are_compact() {
        // Verify that None fields add minimal overhead (postcard encodes None as single byte)
        let event_no_extras = make_event(None, None, TraceEventKind::EvalStart);
        let event_with_extras = make_event(Some(100_000), Some(42), TraceEventKind::EvalStart);

        let bytes_no = serialize(&event_no_extras);
        let bytes_with = serialize(&event_with_extras);

        // The version with Some values should be larger (duration u64 + span u64 varint overhead)
        assert!(
            bytes_with.len() > bytes_no.len(),
            "Event with duration/span ({} bytes) should be larger than without ({} bytes)",
            bytes_with.len(),
            bytes_no.len()
        );

        // None values should add exactly 2 bytes (one per Option<u64>)
        // because postcard encodes None as a single 0x00 byte
        let overhead = bytes_with.len() - bytes_no.len();
        // The overhead is the varint-encoded Some(100_000) + Some(42) minus the two None bytes
        // We just verify None is compact — each None is 1 byte vs Some(varint) which is 1+varint
        assert!(
            bytes_no.len() < bytes_with.len(),
            "None fields should be smaller than Some fields"
        );
        // Rough check: overhead should be less than 20 bytes (2 varints + 2 Some tags)
        assert!(
            overhead < 20,
            "Overhead for duration+span should be < 20 bytes, got {overhead}"
        );
    }

    #[test]
    fn all_tiers_roundtrip() {
        for tier in [
            TraceTier::TreeWalker,
            TraceTier::BytecodeVM,
            TraceTier::JitStage1,
            TraceTier::JitStage2,
        ] {
            let event = TraceEvent {
                seq: 1,
                thread_id: 0,
                timestamp_ns: 0,
                tier,
                depth: 0,
                input: TraceValue::Unit,
                outputs: vec![],
                expr_span: None,
                kind: TraceEventKind::EvalStart,
                duration_ns: None,
                span_id: None,
            };
            let bytes = serialize(&event);
            let decoded: TraceEvent = deserialize(&bytes).expect("roundtrip tier");
            assert_eq!(decoded.tier, tier);
        }
    }

    #[test]
    fn complex_event_kinds_roundtrip() {
        // Test several complex event kinds with nested data
        let kinds = vec![
            TraceEventKind::RuleApplication {
                rule_lhs: TraceValue::SExpr(vec![
                    TraceValue::Atom("=".to_string()),
                    TraceValue::SExpr(vec![
                        TraceValue::Atom("fact".to_string()),
                        TraceValue::Atom("$n".to_string()),
                    ]),
                ]),
                rule_rhs: TraceValue::Atom("body".to_string()),
                bindings: vec![("$n".to_string(), TraceValue::Long(5))],
                rule_span: Some(TraceSpan {
                    file_id: 0,
                    start_row: 1,
                    start_col: 1,
                    end_row: 1,
                    end_col: 30,
                }),
            },
            TraceEventKind::GcSafepoint {
                root_count: 42,
                allocation_delta_bytes: 1_048_576,
            },
            TraceEventKind::WorkPoolScaleEvent {
                action: "unpark".to_string(),
                active_workers_after: 4,
                min_workers: 1,
                max_workers: 8,
                queue_depth: 10,
                ema_throughput: 0.75,
                ema_queue_depth: 5.5,
                ema_slab_pressure: 0.1,
                ema_rss_pressure: 0.3,
                objective: 0.85,
                emergency: false,
                hc_direction: 1,
                hc_cooldown_remaining: 0,
                hc_prev_objective: 1.0,
                hc_improvement: 0.15,
                raw_throughput: 100.0,
                raw_slab_pressure: 0.0,
                raw_rss_pressure: 0.0,
                bp_level: 0,
                slab_amplifier: 1.0,
                term_throughput: -0.75,
                term_queue_depth: 20.0,
                term_slab_pressure: 0.5,
                term_rss_pressure: 2.4,
                blocked_worker_count: 0,
                overflow_count: 0,
                decision_phase: "phase4_hill_climber".to_string(),
                delta_evals: 500,
                elapsed_seconds: 0.2,
            },
            TraceEventKind::NondeterministicFork { branch_count: 3 },
            TraceEventKind::BranchEnd {
                branch_index: 2,
                result_count: 1,
            },
        ];

        for kind in kinds {
            let event = make_event(Some(500), Some(77), kind.clone());
            let bytes = serialize(&event);
            let decoded: TraceEvent = deserialize(&bytes).expect("roundtrip complex kind");
            assert_eq!(decoded.kind, kind);
            assert_eq!(decoded.duration_ns, Some(500));
            assert_eq!(decoded.span_id, Some(77));
        }
    }

    #[test]
    fn magic_bytes_are_correct() {
        assert_eq!(&TRACE_MAGIC[..6], b"MTRACE");
        assert_eq!(TRACE_MAGIC[6], 0x00);
        assert_eq!(TRACE_MAGIC[7], 0x02); // v2

        assert_eq!(&TRACE_MAGIC_V1[..6], b"MTRACE");
        assert_eq!(TRACE_MAGIC_V1[6], 0x00);
        assert_eq!(TRACE_MAGIC_V1[7], 0x01); // v1
    }

    #[test]
    fn format_version_constant() {
        assert_eq!(TRACE_FORMAT_VERSION, 2);
    }
}
