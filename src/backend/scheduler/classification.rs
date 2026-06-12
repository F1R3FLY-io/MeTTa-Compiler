//! Expression classification: MeTTa expressions → CostClass.
//!
//! Implements the two-level lookup table for O(1) classification:
//!
//! ```text
//! Level 1: head_hash (16-bit) → Option<L2Index>   (65536 entries, ~512KB)
//! Level 2: (arity, flags) → CostClass             (sparse per head symbol)
//! ```
//!
//! Two cache-line loads per classification. Falls back to heuristic classification
//! when the table doesn't contain an entry for the expression.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

use crate::backend::models::adaptive_pool::Ema;
use crate::backend::models::metta_value::{MettaValue, ValueView};

use super::cost_class::{
    descriptor_flags, AffinityHint, CostClass, SchedulingAction, TaskDescriptor,
};

// ══════════════════════════════════════════════════════════════════════════════
// Known head symbol sets for heuristic classification
// ══════════════════════════════════════════════════════════════════════════════

/// Grounded arithmetic/comparison operators.
const GROUNDED_ARITH_OPS: &[&str] = &[
    "+", "-", "*", "/", "%", "<", ">", "<=", ">=", "==", "!=", "and", "or", "not", "xor", "nand",
    "min-atom", "max-atom", "abs", "mod", "pow", "sqrt", "log",
];

/// Phase 10.D (2026-05-17) — split of the historical `IMPURE_HEADS`
/// into two semantically distinct classes:
///
/// 1. **State-mutating** heads — operations whose execution order can
///    produce different evaluator state and therefore can break
///    HE-bisim correctness under parallel dispatch (e.g.,
///    `add-atom`/`remove-atom` racing on the same space, `bind!`
///    racing on the same name). These MUST keep the body-impurity
///    veto on parallel dispatch.
///
/// 2. **I/O** heads — operations whose execution order affects
///    OBSERVABLE output (`println!`, `print!`) but does NOT affect
///    evaluator state. HE-bisim treats parallel I/O as unordered
///    (any interleaving is correct), so these are safe to parallel-
///    dispatch by default. Users who depend on print ordering can
///    set `METTATRON_STRICT_PRINT_ORDER=1` to restore the wider veto.
///
/// `IMPURE_HEADS` remains the union — used by the cost-class
/// scheduler (`classify_heuristic` → `CostClass::ImpureSequential`)
/// where any impurity, including I/O, is a sequential-affinity hint
/// regardless of bisim correctness.
const STATE_MUTATING_HEADS: &[&str] = &[
    "add-atom",
    "remove-atom",
    "change-state!",
    "compare-and-swap-state!",
    "get-state",
    "sealed",
    "import!",
    "include",
    "bind!",
    "pragma!",
    "new-space",
    "new-state",
    // Phase I (2026-05-20): concurrency primitives are state-mutating.
    "spawn!",
    "await!",
    "await-barrier!",
    "loop-until-state",
    "new-das!",
    "new-distributed-space",
    "das-barrier!",
    "add-observer!",
    "snapshot!",
    "partition-space",
    // Random operations advance or create generator state; reordering them can
    // change observed values even when the expression shape is otherwise pure.
    "new-random-generator",
    "set-random-seed",
    "random-int",
    "random-float",
];

const IO_HEADS: &[&str] = &[
    "println!",
    "print!",
    "eprintln!",
    "eprint!",
    "trace!",
    "format",
];

/// Known impure (side-effecting) head symbols — the union of
/// `STATE_MUTATING_HEADS` and `IO_HEADS`. Retained for the cost-class
/// scheduler and for callers that want the historical wide check.
const IMPURE_HEADS: &[&str] = &[
    "add-atom",
    "remove-atom",
    "change-state!",
    "compare-and-swap-state!",
    "get-state",
    "println!",
    "print!",
    "eprintln!",
    "eprint!",
    "trace!",
    "format",
    "sealed",
    "import!",
    "include",
    "bind!",
    "pragma!",
    "new-space",
    "new-state",
    "nop",
    // Phase I concurrency primitives.
    "spawn!",
    "await!",
    "await-barrier!",
    "loop-until-state",
    "new-das!",
    "new-distributed-space",
    "das-barrier!",
    "add-observer!",
    "snapshot!",
    "partition-space",
    "new-random-generator",
    "set-random-seed",
    "random-int",
    "random-float",
];

/// Known pure head symbols (control flow and data manipulation).
const PURE_HEADS: &[&str] = &[
    "if",
    "case",
    "switch",
    "let",
    "let*",
    "quote",
    "eval",
    "chain",
    "cons-atom",
    "decons-atom",
    "car-atom",
    "cdr-atom",
    "collapse",
    "superpose",
    "unique",
    "get-type",
    "get-metatype",
    "check-type",
    "match",
    "unify",
    "empty",
    "Error",
    "error",
    "catch",
    "is-error",
    "assertEqual",
    "assertEqualToResult",
    "size-atom",
    "index-atom",
    "subtraction",
    "intersection",
    "union",
    "tuple-count",
    "tuple-concat",
    "flip",
    "id",
    "sort-strings",
];

/// Check if a head symbol is a known arithmetic/comparison operator.
#[inline]
fn is_arithmetic_head(head: &str) -> bool {
    GROUNDED_ARITH_OPS.iter().any(|&op| op == head)
}

/// Check if a head symbol is known to be impure (state-mutating OR I/O).
#[inline]
fn is_impure_head(head: &str) -> bool {
    IMPURE_HEADS.iter().any(|&op| op == head)
}

/// Check if a head symbol mutates evaluator state (HE-bisim
/// correctness blocker under parallel dispatch). Subset of
/// `IMPURE_HEADS` per Phase 10.D split.
#[inline]
fn is_state_mutating_head(head: &str) -> bool {
    STATE_MUTATING_HEADS.iter().any(|&op| op == head)
}

/// Check if a head symbol is an I/O head whose order affects observable
/// output but not evaluator state. Subset of `IMPURE_HEADS`.
#[inline]
fn is_io_head(head: &str) -> bool {
    IO_HEADS.iter().any(|&op| op == head)
}

/// Check if a head symbol is known to be pure.
#[inline]
fn is_pure_head(head: &str) -> bool {
    PURE_HEADS.iter().any(|&op| op == head)
}

/// Check if a head symbol is known (pure, impure, or arithmetic).
#[inline]
fn is_known_head(head: &str) -> bool {
    is_pure_head(head) || is_impure_head(head) || is_arithmetic_head(head)
}

/// **H2 (2026-05-05)**: Detect whether a body expression transitively contains
/// any side-effecting head, for branch-serialization decisions at parallel
/// dispatch gates.
///
/// Returns `true` if the body — walked to `max_depth` (default 8) — contains
/// any S-expression whose head is in `IMPURE_HEADS` (`add-atom`, `remove-atom`,
/// `change-state!`, `bind!`, `import!`, `pragma!`, `println!`, etc.).
///
/// Optimistic: only returns `true` when the walk PROVES impurity by finding
/// an `IMPURE_HEADS` symbol. Unknown user-defined heads are treated as pure
/// (false negative is acceptable for performance; mmverify's
/// `assign_f_hyp_to_var` body is `(unify ... (let () (remove-atom &kb ...) ...))`
/// which the walker DOES find at depth ≥ 3).
///
/// PLN/Robot pure-functional rule bodies (no `add-atom`/`remove-atom` in their
/// transitive call structure) remain parallel-eligible. Bodies that do
/// transitively call IMPURE_HEADS within `max_depth` levels become serial,
/// preserving HE bisimilarity per spec §5.6.1 [N, sub-profile ST].
///
/// Used by the 3 parallel-dispatch gates in `eval_loop.rs` (rule-match,
/// ProcessLet body dispatch, StartAmb/superpose).
pub fn body_contains_impure(body: &MettaValue, max_depth: u32) -> bool {
    if max_depth == 0 {
        return false;
    }
    if let Some(items) = body.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            if is_impure_head(head) {
                return true;
            }
        }
        for child in items {
            if body_contains_impure(child, max_depth - 1) {
                return true;
            }
        }
    }
    false
}

/// Phase 10.D (2026-05-17): bisim-correctness-only impurity check used
/// at the 3 parallel-dispatch gates (rule-match, ProcessLet body,
/// StartAmb/superpose). Strictly tighter than `body_contains_impure`
/// — it returns `true` only for `STATE_MUTATING_HEADS` (the subset
/// that can race on evaluator state and break HE-bisim).
///
/// `IO_HEADS` (println!/print!/trace!) are NOT detected by this
/// function, so PLN bodies like `(progn (println! ...) (PLN.Derive ...))`
/// remain parallel-eligible. Users who depend on exact print order can
/// set `METTATRON_STRICT_PRINT_ORDER=1` — see
/// `body_blocks_parallel_dispatch` below.
pub fn body_contains_state_mutation(body: &MettaValue, max_depth: u32) -> bool {
    if max_depth == 0 {
        return false;
    }
    if let Some(items) = body.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            if is_state_mutating_head(head) {
                return true;
            }
        }
        for child in items {
            if body_contains_state_mutation(child, max_depth - 1) {
                return true;
            }
        }
    }
    false
}

/// Phase 10.D (2026-05-17): I/O-head detector for the strict-print
/// order opt-in.
pub fn body_contains_io(body: &MettaValue, max_depth: u32) -> bool {
    if max_depth == 0 {
        return false;
    }
    if let Some(items) = body.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            if is_io_head(head) {
                return true;
            }
        }
        for child in items {
            if body_contains_io(child, max_depth - 1) {
                return true;
            }
        }
    }
    false
}

/// Phase 10.D (2026-05-17): unified parallel-dispatch veto used by
/// `dispatch_rule_matches`, `ProcessLet` body fan-out, and
/// `StartAmb`/superpose. Returns `true` if the body contains a head
/// that would make parallel dispatch incorrect or that the user has
/// opted into treating as ordered.
///
/// Default: state-mutation only (HE-bisim correctness). With
/// `METTATRON_STRICT_PRINT_ORDER=1`, ALSO returns `true` for bodies
/// containing I/O heads (restores the historical
/// `body_contains_impure` behavior). The env var is sampled once
/// per process via `OnceLock`.
pub fn body_blocks_parallel_dispatch(body: &MettaValue, max_depth: u32) -> bool {
    if body_contains_state_mutation(body, max_depth) {
        return true;
    }
    if strict_print_order() && body_contains_io(body, max_depth) {
        return true;
    }
    false
}

/// Cached value of `METTATRON_STRICT_PRINT_ORDER` — when truthy, the
/// parallel-dispatch veto also treats I/O heads as ordering-relevant.
fn strict_print_order() -> bool {
    use std::sync::OnceLock;
    static STRICT: OnceLock<bool> = OnceLock::new();
    *STRICT.get_or_init(|| {
        std::env::var("METTATRON_STRICT_PRINT_ORDER")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false)
    })
}

/// Recursively check if an expression's children contain calls to unknown
/// user-defined functions (heads not in any known-head list). Bounded to
/// `max_depth` levels to prevent O(n) blowup on deep expressions.
///
/// Returns `true` if any child S-expression has an unknown head, indicating
/// the expression will trigger user-defined rule matching (potentially expensive).
fn has_expensive_children(items: &[MettaValue], max_depth: u32) -> bool {
    if max_depth == 0 {
        return false;
    }
    for child in &items[1..] {
        if let Some(child_items) = child.as_sexpr() {
            if !child_items.is_empty() {
                if let Some(head) = child_items[0].as_atom() {
                    if !is_known_head(head) {
                        return true; // Unknown user function → expensive
                    }
                }
                // Recurse into children of known-head expressions
                if has_expensive_children(child_items, max_depth - 1) {
                    return true;
                }
            }
        }
    }
    false
}

// ══════════════════════════════════════════════════════════════════════════════
// L2 Entry — sparse per-head classification
// ══════════════════════════════════════════════════════════════════════════════

/// Level-2 classification entry: maps (arity, flags) pattern to a cost class.
#[derive(Debug, Clone, Copy)]
struct L2Entry {
    /// 4-bit arity (0..15).
    arity: u8,
    /// 8-bit flags mask to match against.
    flags_mask: u8,
    /// Expected flags value (after masking).
    flags_value: u8,
    /// Resulting cost class.
    cost_class: CostClass,
}

impl L2Entry {
    /// Check if this entry matches the given arity and flags.
    #[inline]
    fn matches(&self, arity: u8, flags: u8) -> bool {
        self.arity == arity && (flags & self.flags_mask) == self.flags_value
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// SchedulerAutomaton — The compiled WFST scheduler
// ══════════════════════════════════════════════════════════════════════════════

/// Size of the Level-1 classification table (one entry per 16-bit head hash).
const L1_TABLE_SIZE: usize = 65536;

/// The compiled WFST scheduler automaton.
///
/// Contains the three-layer pipeline:
/// - Layer 1 (WTA): expression → CostClass via two-level table
/// - Layer 2 (WFST): CostClass → SchedulingAction via flat array
/// - Layer 3 (WPDS): context weight refinement (in context_weights module)
///
/// Thread-safe: the classification table is immutable after construction,
/// and weight EMAs use DashMap for lock-free concurrent updates.
pub struct SchedulerAutomaton {
    // ── Layer 1: Expression → CostClass ──────────────────────────────────
    /// Level-1 table: head_hash → index into `l2_entries`.
    /// `None` means no entry for this head hash (use heuristic fallback).
    l1_table: Vec<Option<u32>>,

    /// Level-2 entries: packed `(start_idx, count)` pairs.
    /// For head hash `h`, entries are at `l2_entries[l1_table[h]..l1_table[h]+count]`.
    l2_entries: Vec<L2Entry>,

    /// Level-2 counts: number of L2 entries per L1 slot.
    l2_counts: Vec<u16>,

    // ── Layer 2: CostClass → SchedulingAction ────────────────────────────
    /// Transduction table: one SchedulingAction per CostClass.
    transduction_table: [SchedulingAction; CostClass::COUNT],

    // ── Online weight refinement ─────────────────────────────────────────
    /// Per-(head_hash, arity) EMA weight tracker.
    /// Updated after each task completion with actual runtime.
    weight_emas: DashMap<u32, Ema>,

    /// Epoch counter: incremented when AAM re-analysis rebuilds the automaton.
    epoch: AtomicU64,

    // ── Layer 3: Context weights (WPDS) ──────────────────────────────────
    /// Precomputed context weights: hash(top_k_continuations) → multiplier.
    /// Populated by WPDS poststar computation per scheduling epoch.
    context_weights: DashMap<u64, f32>,
}

impl SchedulerAutomaton {
    /// Create a new scheduler automaton with default transduction table.
    pub fn new() -> Self {
        Self {
            l1_table: vec![None; L1_TABLE_SIZE],
            l2_entries: Vec::new(),
            l2_counts: vec![0; L1_TABLE_SIZE],
            transduction_table: Self::default_transduction_table(),
            weight_emas: DashMap::new(),
            epoch: AtomicU64::new(0),
            context_weights: DashMap::new(),
        }
    }

    /// Default transduction table mapping CostClass → SchedulingAction.
    ///
    /// | CostClass         | Priority | ParDegree | Affinity | Memo  |
    /// |--------------------|----------|-----------|----------|-------|
    /// | GroundCheap        | 0        | 1         | Any      | false |
    /// | GroundArith        | 0        | 1         | Any      | true  |
    /// | SymbolicCheap      | 2        | 1         | Sticky   | true  |
    /// | SymbolicModerate   | 5        | 4         | Any      | false |
    /// | RecursiveBounded   | 8        | 1         | Sticky   | true  |
    /// | RecursiveUnbounded | 10       | 1         | Sticky   | false |
    /// | ParallelPure       | 3        | 8         | Any      | true  |
    /// | ImpureSequential   | 5        | 1         | Sticky   | false |
    fn default_transduction_table() -> [SchedulingAction; CostClass::COUNT] {
        [
            // GroundCheap: immediate, single-threaded
            SchedulingAction::new(0, 1, AffinityHint::Any, false),
            // GroundArith: immediate, memoizable
            SchedulingAction::new(0, 1, AffinityHint::Any, true),
            // SymbolicCheap: low priority, sticky for cache locality
            SchedulingAction::new(2, 1, AffinityHint::Sticky, true),
            // SymbolicModerate: normal priority, fan out to 4 workers
            SchedulingAction::new(5, 4, AffinityHint::Any, false),
            // RecursiveBounded: higher priority (finish recursive chains), sticky
            SchedulingAction::new(8, 1, AffinityHint::Sticky, true),
            // RecursiveUnbounded: deprioritized (may diverge), sticky
            SchedulingAction::new(10, 1, AffinityHint::Sticky, false),
            // ParallelPure: moderate priority, high parallelism
            SchedulingAction::new(3, 8, AffinityHint::Any, true),
            // ImpureSequential: normal priority, sequential, sticky
            SchedulingAction::new(5, 1, AffinityHint::Sticky, false),
        ]
    }

    // ── Classification (Layer 1) ─────────────────────────────────────────

    /// Classify a MeTTa expression into a CostClass.
    ///
    /// Uses the two-level table for known expressions, falls back to heuristic
    /// classification for unknown expressions. O(1) amortized.
    #[inline]
    pub fn classify(&self, expr: &MettaValue) -> CostClass {
        match expr.view() {
            // Ground literals → GroundCheap
            ValueView::Long(_)
            | ValueView::Float(_)
            | ValueView::Bool(_)
            | ValueView::Unit
            | ValueView::Empty
            | ValueView::NotReducible => CostClass::GroundCheap,

            ValueView::String(_) => CostClass::GroundCheap,

            ValueView::Atom(s) => {
                // Variables are not ground
                if s.starts_with('$') || s == "_" || s.starts_with('\'') {
                    CostClass::GroundCheap // Variables are cheap to evaluate (just lookup)
                } else {
                    CostClass::GroundCheap
                }
            }

            ValueView::SExpr(items) => {
                if items.is_empty() {
                    return CostClass::GroundCheap;
                }
                self.classify_sexpr(items)
            }

            ValueView::Error(..) => CostClass::GroundCheap,
            ValueView::Quoted(_) => CostClass::GroundCheap,
            ValueView::Type(_) => CostClass::GroundCheap,
            ValueView::Conjunction(_) => CostClass::SymbolicModerate,
            ValueView::Space(_) => CostClass::ImpureSequential,
            ValueView::State(_) => CostClass::ImpureSequential,
            ValueView::Memo(_) => CostClass::GroundCheap,
            // PT-canonical Lazy is data — never reduces, treat as ground.
            ValueView::Lazy(_) => CostClass::GroundCheap,
        }
    }

    /// Classify an S-expression given its items.
    fn classify_sexpr(&self, items: &[MettaValue]) -> CostClass {
        // Extract head symbol
        let head_str = match items[0].view() {
            ValueView::Atom(s) => s,
            _ => return CostClass::SymbolicModerate, // non-atom head = dynamic dispatch
        };

        let arity = (items.len() - 1).min(15) as u8;
        let head_hash = TaskDescriptor::hash_head_symbol(head_str);

        // Build flags from head symbol knowledge
        let mut flags: u8 = 0;
        if is_pure_head(head_str) || is_arithmetic_head(head_str) {
            flags |= descriptor_flags::PURE;
        }
        if is_arithmetic_head(head_str) && !items[1..].iter().any(|v| v.has_variables_fast()) {
            flags |= descriptor_flags::GROUND;
        }

        // Try two-level table lookup first
        let l1_idx = head_hash as usize;
        if let Some(l2_start) = self.l1_table[l1_idx] {
            let start = l2_start as usize;
            let count = self.l2_counts.get(l1_idx).copied().unwrap_or(0) as usize;
            for i in start..start + count {
                if let Some(entry) = self.l2_entries.get(i) {
                    if entry.matches(arity, flags) {
                        return entry.cost_class;
                    }
                }
            }
        }

        // Heuristic fallback
        let class = self.classify_heuristic(head_str, arity, flags);

        // Deep inspection: if the heuristic says SymbolicCheap but children
        // contain calls to unknown user-defined functions, escalate to
        // SymbolicModerate. Pure control flow heads (let*, if, case, etc.)
        // can contain arbitrarily expensive nested computation.
        if class == CostClass::SymbolicCheap && is_pure_head(head_str) {
            if has_expensive_children(items, 3) {
                return CostClass::SymbolicModerate;
            }
        }

        class
    }

    /// Heuristic classification when table lookup misses.
    ///
    /// Unknown user-defined functions default to SymbolicCheap (sequential,
    /// degree=1). Only escalate to SymbolicModerate when the L2 table has
    /// evidence of multi-rule matching. This prevents over-parallelization
    /// of trivial recursive functions like PLN's BestCandidate, TupleConcat, etc.
    fn classify_heuristic(&self, head: &str, _arity: u8, flags: u8) -> CostClass {
        if is_arithmetic_head(head) {
            if flags & descriptor_flags::GROUND != 0 {
                CostClass::GroundArith
            } else {
                CostClass::SymbolicCheap
            }
        } else if is_impure_head(head) {
            CostClass::ImpureSequential
        } else if is_pure_head(head) {
            CostClass::SymbolicCheap
        } else {
            // Unknown user-defined function → assume cheap (sequential).
            // The L2 table overrides this for known expensive patterns.
            CostClass::SymbolicCheap
        }
    }

    // ── Transduction (Layer 2) ───────────────────────────────────────────

    /// Transduce a CostClass into a SchedulingAction. O(1) array index.
    #[inline]
    pub fn transduce(&self, class: CostClass) -> SchedulingAction {
        self.transduction_table[class as usize]
    }

    /// Classify and transduce in one call.
    #[inline]
    pub fn classify_and_transduce(&self, expr: &MettaValue) -> (CostClass, SchedulingAction) {
        let class = self.classify(expr);
        let action = self.transduce(class);
        (class, action)
    }

    // ── Context weight lookup (Layer 3) ──────────────────────────────────

    /// Look up the context weight multiplier for a continuation stack hash.
    ///
    /// Returns the precomputed WPDS weight for the given context, or 1.0
    /// if no entry exists (neutral multiplier).
    #[inline]
    pub fn context_weight(&self, context_hash: u64) -> f32 {
        self.context_weights
            .get(&context_hash)
            .map(|v| *v)
            .unwrap_or(1.0)
    }

    /// Compute the effective priority for a task given its cost class and
    /// continuation context.
    ///
    /// ```text
    /// effective_priority = base_priority(class) × context_weight(continuation_hash)
    /// ```
    #[inline]
    pub fn effective_priority(&self, class: CostClass, context_hash: u64) -> u32 {
        let base = self.transduction_table[class as usize].priority_class as f32;
        let ctx = self.context_weight(context_hash);
        (base * ctx).round().min(255.0).max(0.0) as u32
    }

    // ── Online weight update ─────────────────────────────────────────────

    /// Update the EMA weight for a (head_hash, arity) pair after task completion.
    ///
    /// Called from the worker loop after each task executes.
    pub fn update_weight(&self, descriptor: TaskDescriptor, actual_runtime_ns: u64) {
        let key = descriptor.head_arity_key();
        self.weight_emas
            .entry(key)
            .or_insert_with(|| Ema::new(0.15))
            .update(actual_runtime_ns as f64);
    }

    /// Get the current EMA-estimated runtime for a (head_hash, arity) pair.
    pub fn estimated_runtime(&self, descriptor: TaskDescriptor) -> Option<f64> {
        let key = descriptor.head_arity_key();
        self.weight_emas.get(&key).map(|ref_| {
            let ema: &Ema = ref_.value();
            ema.value()
        })
    }

    // ── Epoch management ─────────────────────────────────────────────────

    /// Get the current epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Relaxed)
    }

    /// Increment the epoch (called when AAM re-analysis completes).
    pub fn advance_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::Relaxed) + 1
    }

    // ── Table construction ───────────────────────────────────────────────

    /// Insert a classification entry into the two-level table.
    pub fn insert_classification(
        &mut self,
        head_hash: u16,
        arity: u8,
        flags_mask: u8,
        flags_value: u8,
        cost_class: CostClass,
    ) {
        let l1_idx = head_hash as usize;

        let entry = L2Entry {
            arity,
            flags_mask,
            flags_value,
            cost_class,
        };

        if self.l2_counts.len() < L1_TABLE_SIZE {
            self.l2_counts.resize(L1_TABLE_SIZE, 0);
        }

        if let Some(start) = self.l1_table[l1_idx] {
            // Append to existing L2 entries
            let start = start as usize;
            let count = self.l2_counts[l1_idx] as usize;
            // Check for duplicate
            for i in start..start + count {
                if self.l2_entries[i].arity == arity
                    && self.l2_entries[i].flags_mask == flags_mask
                    && self.l2_entries[i].flags_value == flags_value
                {
                    // Update existing entry
                    self.l2_entries[i].cost_class = cost_class;
                    return;
                }
            }
            // Insert at the end of this head's contiguous L2 range. Later
            // ranges shift right by one slot, preserving the lookup invariant
            // l2_entries[start..start+count] for every occupied L1 slot.
            let insert_at = start + count;
            self.l2_entries.insert(insert_at, entry);
            self.l2_counts[l1_idx] += 1;
            for (idx, maybe_start) in self.l1_table.iter_mut().enumerate() {
                if idx == l1_idx {
                    continue;
                }
                if let Some(other_start) = maybe_start {
                    if *other_start as usize >= insert_at {
                        *other_start += 1;
                    }
                }
            }
        } else {
            // First entry for this head hash
            let start = self.l2_entries.len() as u32;
            self.l1_table[l1_idx] = Some(start);
            self.l2_entries.push(entry);
            self.l2_counts[l1_idx] = 1;
        }
    }

    /// Insert a context weight entry.
    pub fn insert_context_weight(&self, context_hash: u64, weight: f32) {
        self.context_weights.insert(context_hash, weight);
    }

    /// Set a custom transduction table entry.
    pub fn set_transduction(&mut self, class: CostClass, action: SchedulingAction) {
        self.transduction_table[class as usize] = action;
    }
}

impl Default for SchedulerAutomaton {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SchedulerAutomaton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let l2_count = self.l2_entries.len();
        let l1_occupied = self.l1_table.iter().filter(|e| e.is_some()).count();
        let ctx_count = self.context_weights.len();
        let ema_count = self.weight_emas.len();
        write!(
            f,
            "SchedulerAutomaton {{ l1_occupied: {}, l2_entries: {}, ctx_weights: {}, emas: {}, epoch: {} }}",
            l1_occupied,
            l2_count,
            ctx_count,
            ema_count,
            self.epoch(),
        )
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Global scheduler automaton
// ══════════════════════════════════════════════════════════════════════════════

use std::sync::OnceLock;

/// Global scheduler automaton instance.
///
/// Initialized lazily on first use. Rebuilt when AAM analysis completes.
static GLOBAL_SCHEDULER: OnceLock<SchedulerAutomaton> = OnceLock::new();

/// Get or initialize the global scheduler automaton.
///
/// Returns the installed automaton, or creates a default one on first call.
pub fn global_scheduler() -> &'static SchedulerAutomaton {
    GLOBAL_SCHEDULER.get_or_init(SchedulerAutomaton::new)
}

/// Install a new scheduler automaton as the global instance.
///
/// Returns `Ok(())` if installed successfully, `Err(automaton)` if one was
/// already installed (OnceLock semantics — first writer wins).
///
/// For epoch-based updates after AAM re-analysis, use `global_scheduler()`
/// and update the DashMap weights in-place rather than replacing the automaton.
pub fn install_scheduler(automaton: SchedulerAutomaton) -> Result<(), SchedulerAutomaton> {
    GLOBAL_SCHEDULER.set(automaton)
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_transduction_table() {
        let automaton = SchedulerAutomaton::new();

        let ground = automaton.transduce(CostClass::GroundCheap);
        assert_eq!(ground.priority_class, 0);
        assert_eq!(ground.parallelism_degree, 1);
        assert!(!ground.memoizable);

        let arith = automaton.transduce(CostClass::GroundArith);
        assert_eq!(arith.priority_class, 0);
        assert!(arith.memoizable);

        let impure = automaton.transduce(CostClass::ImpureSequential);
        assert_eq!(impure.parallelism_degree, 1);
        assert_eq!(impure.affinity_hint, AffinityHint::Sticky);
        assert!(!impure.memoizable);

        let parallel = automaton.transduce(CostClass::ParallelPure);
        assert!(parallel.parallelism_degree > 1);
        assert!(parallel.memoizable);
    }

    #[test]
    fn test_heuristic_classification() {
        let automaton = SchedulerAutomaton::new();

        // Arithmetic with ground flag
        let class = automaton.classify_heuristic("+", 2, descriptor_flags::GROUND);
        assert_eq!(class, CostClass::GroundArith);

        // Arithmetic without ground flag
        let class = automaton.classify_heuristic("+", 2, 0);
        assert_eq!(class, CostClass::SymbolicCheap);

        // Impure
        let class = automaton.classify_heuristic("add-atom", 2, 0);
        assert_eq!(class, CostClass::ImpureSequential);

        // Pure control flow — classified as SymbolicCheap (sequential)
        // to avoid over-parallelizing trivial branches
        let class = automaton.classify_heuristic("if", 3, descriptor_flags::PURE);
        assert_eq!(class, CostClass::SymbolicCheap);

        // Unknown user function — assumed cheap until L2 table proves otherwise
        let class = automaton.classify_heuristic("my-custom-fn", 2, 0);
        assert_eq!(class, CostClass::SymbolicCheap);
    }

    #[test]
    fn test_table_insert_and_lookup() {
        let mut automaton = SchedulerAutomaton::new();
        let head_hash = TaskDescriptor::hash_head_symbol("my-fn");

        automaton.insert_classification(
            head_hash,
            2,
            descriptor_flags::PURE,
            descriptor_flags::PURE,
            CostClass::ParallelPure,
        );

        // Direct L2 lookup simulation
        let l1_idx = head_hash as usize;
        assert!(automaton.l1_table[l1_idx].is_some());
    }

    #[test]
    fn test_interleaved_table_insert_keeps_l2_ranges_contiguous() {
        let mut automaton = SchedulerAutomaton::new();
        let head_a = "formal-a";
        let head_b = "formal-b";
        let hash_a = TaskDescriptor::hash_head_symbol(head_a);
        let hash_b = TaskDescriptor::hash_head_symbol(head_b);

        automaton.insert_classification(hash_a, 1, 0, 0, CostClass::RecursiveBounded);
        automaton.insert_classification(hash_b, 1, 0, 0, CostClass::ImpureSequential);
        automaton.insert_classification(hash_a, 2, 0, 0, CostClass::ParallelPure);

        let expr_a_2 = MettaValue::SExpr(vec![
            MettaValue::Atom(head_a),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let expr_b_1 = MettaValue::SExpr(vec![MettaValue::Atom(head_b), MettaValue::Long(1)]);

        assert_eq!(automaton.classify(&expr_a_2), CostClass::ParallelPure);
        assert_eq!(automaton.classify(&expr_b_1), CostClass::ImpureSequential);
    }

    #[test]
    fn test_random_heads_are_sequential_and_non_memoizable() {
        let automaton = SchedulerAutomaton::new();
        for head in [
            "new-random-generator",
            "set-random-seed",
            "random-int",
            "random-float",
        ] {
            let class = automaton.classify_heuristic(head, 3, 0);
            let action = automaton.transduce(class);
            assert_eq!(class, CostClass::ImpureSequential, "{head}");
            assert_eq!(action.parallelism_degree, 1, "{head}");
            assert!(!action.memoizable, "{head}");
        }

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("random-int"),
            MettaValue::Atom("&rng"),
            MettaValue::Long(0),
            MettaValue::Long(10),
        ]);
        assert_eq!(automaton.classify(&expr), CostClass::ImpureSequential);
        assert!(body_blocks_parallel_dispatch(&expr, 8));
    }

    #[test]
    fn test_context_weight_default() {
        let automaton = SchedulerAutomaton::new();
        // No entries → default multiplier of 1.0
        assert_eq!(automaton.context_weight(42), 1.0);
    }

    #[test]
    fn test_context_weight_lookup() {
        let automaton = SchedulerAutomaton::new();
        automaton.insert_context_weight(42, 2.5);
        assert_eq!(automaton.context_weight(42), 2.5);
    }

    #[test]
    fn test_effective_priority() {
        let automaton = SchedulerAutomaton::new();
        // GroundCheap has base priority 0
        assert_eq!(automaton.effective_priority(CostClass::GroundCheap, 0), 0);

        // SymbolicModerate has base priority 5, with 2.0x context weight
        automaton.insert_context_weight(99, 2.0);
        assert_eq!(
            automaton.effective_priority(CostClass::SymbolicModerate, 99),
            10
        );
    }

    #[test]
    fn test_epoch() {
        let automaton = SchedulerAutomaton::new();
        assert_eq!(automaton.epoch(), 0);
        assert_eq!(automaton.advance_epoch(), 1);
        assert_eq!(automaton.epoch(), 1);
    }

    #[test]
    fn test_weight_update() {
        let automaton = SchedulerAutomaton::new();
        let desc = TaskDescriptor::pack(0x1234, 2, 0, 0);

        assert!(automaton.estimated_runtime(desc).is_none());

        automaton.update_weight(desc, 1000);
        let est = automaton
            .estimated_runtime(desc)
            .expect("should have estimate");
        assert_eq!(est, 1000.0); // First sample = exact value

        automaton.update_weight(desc, 2000);
        let est2 = automaton
            .estimated_runtime(desc)
            .expect("should have estimate");
        // EMA: 0.15 * 2000 + 0.85 * 1000 = 1150
        assert!((est2 - 1150.0).abs() < 1.0);
    }
}
