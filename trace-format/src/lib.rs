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

/// Magic bytes identifying a MeTTaTron trace file.
pub const TRACE_MAGIC: [u8; 8] = *b"MTRACE\x00\x01";

/// Current trace format version.
pub const TRACE_FORMAT_VERSION: u32 = 1;

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
}

/// A single trace event — the fundamental unit of the trace log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TraceEvent {
    /// Monotonic sequence number (per-thread).
    pub seq: u64,
    /// Thread that produced this event.
    pub thread_id: u32,
    /// Nanoseconds since trace start.
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
}
