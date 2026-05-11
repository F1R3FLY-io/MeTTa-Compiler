//! Shared utility functions for trace analysis.
//!
//! Provides:
//! - `hash_trace_value()` — recursive hashing for `TraceValue` (handles `f64` via `to_bits()`)
//! - `extract_head_symbol()` — extract the head symbol (first atom) from S-expressions
//! - `extract_operator_name()` — event-kind-aware operator name extraction (uses `TraceEventKind`
//!   to disambiguate structural events like forks/branches from real operators)
//! - `BoundedVec<T>` — reservoir-sampled, memory-bounded collection for percentile computation

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use trace_format::{TraceEventKind, TraceValue};

// ── TraceValue Hashing ─────────────────────────────────────────────────────

/// Recursively hash a `TraceValue`.
///
/// `TraceValue` does not derive `Hash` because it contains `f64`.
/// We use `f64::to_bits()` for deterministic hashing (NaN-aware).
pub fn hash_trace_value(value: &TraceValue) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_trace_value_into(value, &mut hasher);
    hasher.finish()
}

/// Hash a `TraceValue` into an existing hasher (recursive helper).
fn hash_trace_value_into(value: &TraceValue, hasher: &mut impl Hasher) {
    // Discriminant tag first
    std::mem::discriminant(value).hash(hasher);
    match value {
        TraceValue::Atom(s) => s.hash(hasher),
        TraceValue::Bool(b) => b.hash(hasher),
        TraceValue::Long(n) => n.hash(hasher),
        TraceValue::Float(f) => f.to_bits().hash(hasher),
        TraceValue::String(s) => s.hash(hasher),
        TraceValue::SExpr(items) => {
            items.len().hash(hasher);
            for item in items {
                hash_trace_value_into(item, hasher);
            }
        }
        TraceValue::Unit => {}
        TraceValue::Error(msg, details) => {
            msg.hash(hasher);
            hash_trace_value_into(details, hasher);
        }
        TraceValue::Type(inner) => hash_trace_value_into(inner, hasher),
        TraceValue::Empty => {}
        TraceValue::Quoted(inner) => hash_trace_value_into(inner, hasher),
    }
}

/// Hash a slice of `TraceValue` into a single u64.
pub fn hash_trace_values(values: &[TraceValue]) -> u64 {
    let mut hasher = DefaultHasher::new();
    values.len().hash(&mut hasher);
    for v in values {
        hash_trace_value_into(v, &mut hasher);
    }
    hasher.finish()
}

// ── Head Symbol Extraction ─────────────────────────────────────────────────

/// Extract the "head symbol" from a `TraceValue`.
///
/// For S-expressions `(f x y)`, the head is `"f"`.
/// For atoms, the head is the atom name itself.
/// For other value types, returns a descriptive placeholder.
pub fn extract_head_symbol(value: &TraceValue) -> &str {
    match value {
        TraceValue::SExpr(items) if !items.is_empty() => extract_head_symbol(&items[0]),
        TraceValue::Atom(s) => s.as_str(),
        TraceValue::Bool(_) => "<Bool>",
        TraceValue::Long(_) => "<Long>",
        TraceValue::Float(_) => "<Float>",
        TraceValue::String(_) => "<String>",
        TraceValue::SExpr(_) => "()",
        TraceValue::Unit => "()",
        TraceValue::Error(..) => "<Error>",
        TraceValue::Type(_) => "<Type>",
        TraceValue::Empty => "<Empty>",
        TraceValue::Quoted(_) => "<Quoted>",
    }
}

/// Extract a meaningful operator name from a trace event.
///
/// For structural/bookkeeping events (forks, branches, eval lifecycle, GC
/// safepoints), the `input` field is typically `TraceValue::Unit`, which
/// `extract_head_symbol()` maps to the uninformative `"()"`. This function
/// uses the `TraceEventKind` to produce descriptive pseudo-operator names
/// for these events, falling back to `extract_head_symbol()` for all others.
pub fn extract_operator_name(input: &TraceValue, kind: &TraceEventKind) -> String {
    match kind {
        TraceEventKind::NondeterministicFork { branch_count } => {
            format!("<fork:{branch_count}>")
        }
        TraceEventKind::BranchStart { .. } => "<branch-start>".to_string(),
        TraceEventKind::BranchEnd { .. } => "<branch-end>".to_string(),
        TraceEventKind::EvalStart => "<eval-start>".to_string(),
        TraceEventKind::EvalEnd { .. } => "<eval-end>".to_string(),
        TraceEventKind::GcSafepoint { .. } => "<gc-safepoint>".to_string(),
        _ => extract_head_symbol(input).to_string(),
    }
}

// ── BoundedVec for Reservoir Sampling ──────────────────────────────────────

/// A memory-bounded collection that keeps up to `capacity` elements.
///
/// When full, incoming values are accepted with probability `capacity / n`
/// (reservoir sampling), ensuring uniform random sampling from an arbitrarily
/// large stream. After `finalize()`, the stored values can be sorted for
/// percentile computation.
pub struct BoundedVec<T> {
    data: Vec<T>,
    capacity: usize,
    /// Total items offered (for reservoir sampling probability).
    count: u64,
}

impl<T> BoundedVec<T> {
    /// Create a new `BoundedVec` with the given maximum capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            capacity,
            count: 0,
        }
    }

    /// Push a value. If at capacity, use reservoir sampling to decide
    /// whether to replace an existing entry.
    pub fn push(&mut self, value: T) {
        self.count += 1;
        if self.data.len() < self.capacity {
            self.data.push(value);
        } else {
            // Reservoir sampling: accept with probability capacity/count
            let idx = fast_random(self.count);
            if idx < self.capacity as u64 {
                self.data[idx as usize] = value;
            }
        }
    }

    /// Total number of items offered (not necessarily stored).
    pub fn total_count(&self) -> u64 {
        self.count
    }

    /// Number of items currently stored.
    pub fn stored_count(&self) -> usize {
        self.data.len()
    }

    /// Consume and return the stored data.
    pub fn into_vec(self) -> Vec<T> {
        self.data
    }

    /// Get a reference to the stored data.
    pub fn as_slice(&self) -> &[T] {
        &self.data
    }
}

impl<T: Ord> BoundedVec<T> {
    /// Sort the stored data and compute the value at a given percentile (0.0–1.0).
    ///
    /// Returns `None` if no data is stored.
    pub fn percentile(&mut self, p: f64) -> Option<&T> {
        if self.data.is_empty() {
            return None;
        }
        self.data.sort_unstable();
        let idx = ((self.data.len() as f64 * p) as usize).min(self.data.len() - 1);
        Some(&self.data[idx])
    }
}

/// Fast pseudo-random number in [0, n) using a thread-local xorshift64.
///
/// Not cryptographically secure, but fast and sufficient for reservoir sampling.
fn fast_random(n: u64) -> u64 {
    thread_local! {
        static STATE: std::cell::Cell<u64> = std::cell::Cell::new(0x12345678_9abcdef0);
    }
    STATE.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x % n
    })
}

// ── Formatting Helpers ─────────────────────────────────────────────────────

/// Format nanoseconds as human-readable duration.
pub fn format_duration_ns(ns: u64) -> String {
    if ns < 1_000 {
        format!("{ns}ns")
    } else if ns < 1_000_000 {
        format!("{:.1}\u{00b5}s", ns as f64 / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.2}ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.3}s", ns as f64 / 1_000_000_000.0)
    }
}

/// Format a percentage with one decimal place.
pub fn format_pct(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_trace_value_deterministic() {
        let v = TraceValue::SExpr(vec![
            TraceValue::Atom("fact".to_string()),
            TraceValue::Long(5),
        ]);
        let h1 = hash_trace_value(&v);
        let h2 = hash_trace_value(&v);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_trace_value_distinguishes() {
        let v1 = TraceValue::Long(1);
        let v2 = TraceValue::Long(2);
        assert_ne!(hash_trace_value(&v1), hash_trace_value(&v2));
    }

    #[test]
    fn test_hash_float() {
        let v1 = TraceValue::Float(1.0);
        let v2 = TraceValue::Float(1.0);
        assert_eq!(hash_trace_value(&v1), hash_trace_value(&v2));
    }

    #[test]
    fn test_extract_head_symbol_sexpr() {
        let v = TraceValue::SExpr(vec![
            TraceValue::Atom("fact".to_string()),
            TraceValue::Long(5),
        ]);
        assert_eq!(extract_head_symbol(&v), "fact");
    }

    #[test]
    fn test_extract_head_symbol_atom() {
        let v = TraceValue::Atom("hello".to_string());
        assert_eq!(extract_head_symbol(&v), "hello");
    }

    #[test]
    fn test_extract_head_symbol_nested() {
        let v = TraceValue::SExpr(vec![
            TraceValue::SExpr(vec![TraceValue::Atom("inner".to_string())]),
            TraceValue::Long(1),
        ]);
        assert_eq!(extract_head_symbol(&v), "inner");
    }

    #[test]
    fn test_extract_operator_name_fork() {
        let v = TraceValue::Unit;
        let kind = TraceEventKind::NondeterministicFork { branch_count: 3 };
        assert_eq!(extract_operator_name(&v, &kind), "<fork:3>");
    }

    #[test]
    fn test_extract_operator_name_branch_start() {
        let v = TraceValue::Unit;
        let kind = TraceEventKind::BranchStart {
            branch_index: 0,
            total_branches: 3,
        };
        assert_eq!(extract_operator_name(&v, &kind), "<branch-start>");
    }

    #[test]
    fn test_extract_operator_name_branch_end() {
        let v = TraceValue::Unit;
        let kind = TraceEventKind::BranchEnd {
            branch_index: 1,
            result_count: 2,
        };
        assert_eq!(extract_operator_name(&v, &kind), "<branch-end>");
    }

    #[test]
    fn test_extract_operator_name_eval_lifecycle() {
        assert_eq!(
            extract_operator_name(&TraceValue::Unit, &TraceEventKind::EvalStart),
            "<eval-start>"
        );
        assert_eq!(
            extract_operator_name(
                &TraceValue::Unit,
                &TraceEventKind::EvalEnd { result_count: 1 }
            ),
            "<eval-end>"
        );
    }

    #[test]
    fn test_extract_operator_name_gc_safepoint() {
        let kind = TraceEventKind::GcSafepoint {
            root_count: 42,
            allocation_delta_bytes: 1024,
        };
        assert_eq!(
            extract_operator_name(&TraceValue::Unit, &kind),
            "<gc-safepoint>"
        );
    }

    #[test]
    fn test_extract_operator_name_falls_through_to_head_symbol() {
        // For non-structural events, should use extract_head_symbol
        let v = TraceValue::SExpr(vec![
            TraceValue::Atom("fact".to_string()),
            TraceValue::Long(5),
        ]);
        let kind = TraceEventKind::RuleApplication {
            rule_lhs: TraceValue::Unit,
            rule_rhs: TraceValue::Unit,
            bindings: vec![],
            rule_span: None,
        };
        assert_eq!(extract_operator_name(&v, &kind), "fact");
    }

    #[test]
    fn test_bounded_vec_within_capacity() {
        let mut bv = BoundedVec::new(10);
        for i in 0..5 {
            bv.push(i);
        }
        assert_eq!(bv.total_count(), 5);
        assert_eq!(bv.stored_count(), 5);
    }

    #[test]
    fn test_bounded_vec_at_capacity() {
        let mut bv = BoundedVec::new(5);
        for i in 0..100 {
            bv.push(i);
        }
        assert_eq!(bv.total_count(), 100);
        assert!(bv.stored_count() <= 5);
    }

    #[test]
    fn test_bounded_vec_percentile() {
        let mut bv = BoundedVec::new(100);
        for i in 0..100 {
            bv.push(i);
        }
        assert_eq!(*bv.percentile(0.5).expect("should have data"), 50);
        assert_eq!(*bv.percentile(0.95).expect("should have data"), 95);
    }

    #[test]
    fn test_format_duration_ns() {
        assert_eq!(format_duration_ns(500), "500ns");
        assert_eq!(format_duration_ns(1_500), "1.5\u{00b5}s");
        assert_eq!(format_duration_ns(1_500_000), "1.50ms");
        assert_eq!(format_duration_ns(1_500_000_000), "1.500s");
    }
}
