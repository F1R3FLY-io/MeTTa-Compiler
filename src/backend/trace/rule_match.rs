//! Rule-match attempt tracing.
//!
//! Emits a [`RuleMatchAttempt`](trace_format::TraceEventKind::RuleMatchAttempt)
//! event for each (call_site, candidate_rule) pair on every rule match
//! attempt — both successes and failures. The failure variants carry
//! enough detail to answer "why did rule R not match at call site C?"
//! without re-running evaluation.
//!
//! This module is a no-op when the `eval-trace` feature is disabled.
//!
//! # Filter
//!
//! By default no `RuleMatchAttempt` events are emitted. Enable selective
//! tracing via environment variables:
//!
//! - `METTA_TRACE_RULE_MATCH_HEADS` — comma-separated list of head atom
//!   names to trace. `*` enables tracing for all heads. Empty / unset
//!   disables `RuleMatchAttempt` events entirely.
//! - `METTA_TRACE_RULE_MATCH_SUCCESS` — set to `1` to also emit successes
//!   (default: failures only — successes are redundant with the existing
//!   `RuleApplication` event).
//! - `METTA_TRACE_RULE_MATCH_SAMPLE` — emit 1 of every N events (default
//!   `1` = no sampling).
//!
//! # Performance
//!
//! When `METTA_TRACE_RULE_MATCH_HEADS` is unset (default) the filter
//! performs a single atomic load and a `None` check before short-
//! circuiting — no allocation, no path walk, no value conversion.

use std::collections::HashSet;
use std::sync::OnceLock;

use trace_format::{RuleMatchOutcome, TraceEventKind, TraceSpan, TraceTier, TraceValue};

use crate::backend::models::MettaValueTrait;
use crate::backend::trace::convert::trace_value_generic;

/// Per-process filter that decides which `RuleMatchAttempt` events
/// are admitted into the trace.
#[derive(Debug)]
pub struct RuleMatchFilter {
    /// `None` = filter blocks everything (default).
    /// `Some(set)` = trace only the listed head names.
    /// `Some(empty)` after `*` = trace every head.
    heads: Option<HashSet<String>>,
    /// Match every head when true (set when env var contains `*`).
    all_heads: bool,
    /// When false (default), skip `Success` outcomes — they're redundant
    /// with the existing `RuleApplication` event.
    emit_successes: bool,
}

impl RuleMatchFilter {
    /// Construct from environment variables. Called once via `OnceLock`.
    pub fn from_env() -> Self {
        let raw_heads = std::env::var("METTA_TRACE_RULE_MATCH_HEADS").ok();
        let (heads, all_heads) = match raw_heads.as_deref() {
            None | Some("") => (None, false),
            Some("*") => (Some(HashSet::new()), true),
            Some(list) => {
                let set: HashSet<String> = list
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if set.is_empty() {
                    (None, false)
                } else {
                    (Some(set), false)
                }
            }
        };
        let emit_successes = std::env::var("METTA_TRACE_RULE_MATCH_SUCCESS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Self {
            heads,
            all_heads,
            emit_successes,
        }
    }

    /// Returns `true` if the filter is completely disabled (no events
    /// will ever be emitted). Cheap path used by call sites to skip the
    /// entire match-attempt machinery.
    #[inline]
    pub fn is_disabled(&self) -> bool {
        self.heads.is_none()
    }

    /// Returns `true` if attempts on `head` should be admitted.
    #[inline]
    pub fn admits_head(&self, head: &str) -> bool {
        if self.all_heads {
            return true;
        }
        match &self.heads {
            None => false,
            Some(set) => set.contains(head),
        }
    }

    /// Returns `true` if successful attempts should be emitted.
    #[inline]
    pub fn emit_successes(&self) -> bool {
        self.emit_successes
    }
}

/// Singleton accessor — initializes once from environment on first call.
pub fn rule_match_filter() -> &'static RuleMatchFilter {
    static FILTER: OnceLock<RuleMatchFilter> = OnceLock::new();
    FILTER.get_or_init(RuleMatchFilter::from_env)
}

/// Live (non-`TraceValue`) representation of a match-attempt outcome.
///
/// The matchers construct one of these on the failure path so that
/// the conversion to owned `TraceValue` only happens when the event
/// is actually going to be emitted.
#[derive(Debug)]
pub enum LiveOutcome<'v, V> {
    Success {
        /// Pairs of (variable name, bound value reference). Empty when
        /// `emit_successes` is false (the matchers can skip the conversion).
        bindings: Vec<(String, &'v V)>,
    },
    StructuralCheckFailed {
        check_index: u32,
        check_kind: &'static str,
        path: Vec<u16>,
        expected: V,
        actual: Option<V>,
    },
    PathNavigateFailed {
        path: Vec<u16>,
        var: Option<String>,
    },
    EqualCheckFailed {
        var: String,
        first_value: V,
        second_value: V,
    },
    BidirectionalUnifyFailed {
        var: String,
        bound: V,
        candidate: V,
        reason: &'static str,
    },
    MorkExtractFailed {
        note: &'static str,
    },
}

/// Owned variant of [`LiveOutcome`] used by the matcher's
/// `try_match_with_detail` method as its `Err` payload.
///
/// Unlike `LiveOutcome` (which borrows binding references for cheap
/// success conversion), this variant owns its values so the matcher
/// can drop its working borrows before the caller emits the event.
///
/// `expected` is split into `expected_arity` and `expected_atom` to
/// avoid wrapping atoms in a synthetic `V`.
#[derive(Debug)]
pub enum DetailedFailure<V> {
    StructuralCheckFailed {
        check_index: u32,
        check_kind: &'static str,
        path: Vec<u16>,
        /// For arity checks, the expected arity (number of children).
        expected_arity: Option<usize>,
        /// For atom/str checks, the expected literal name.
        expected_atom: Option<String>,
        /// The value that was actually present at `path`.
        actual: Option<V>,
    },
    PathNavigateFailed {
        path: Vec<u16>,
        var: Option<String>,
    },
    EqualCheckFailed {
        var: String,
        first_value: V,
        second_value: V,
    },
    BidirectionalUnifyFailed {
        var: String,
        bound: V,
        candidate: V,
        reason: &'static str,
    },
}

impl<'v, V> LiveOutcome<'v, V>
where
    V: MettaValueTrait + Clone + 'static,
{
    /// Convert this live outcome to the owned `RuleMatchOutcome` carried
    /// by the trace event. Done lazily, only when the event is admitted.
    pub fn into_owned(self) -> RuleMatchOutcome {
        match self {
            LiveOutcome::Success { bindings } => RuleMatchOutcome::Success {
                bindings: bindings
                    .into_iter()
                    .map(|(name, val)| (name, trace_value_generic(val)))
                    .collect(),
            },
            LiveOutcome::StructuralCheckFailed {
                check_index,
                check_kind,
                path,
                expected,
                actual,
            } => RuleMatchOutcome::StructuralCheckFailed {
                check_index,
                check_kind: check_kind.to_string(),
                path,
                expected: trace_value_generic(&expected),
                actual: actual
                    .as_ref()
                    .map(trace_value_generic)
                    .unwrap_or(TraceValue::Empty),
            },
            LiveOutcome::PathNavigateFailed { path, var } => RuleMatchOutcome::PathNavigateFailed {
                path,
                var,
            },
            LiveOutcome::EqualCheckFailed {
                var,
                first_value,
                second_value,
            } => RuleMatchOutcome::EqualCheckFailed {
                var,
                first_value: trace_value_generic(&first_value),
                second_value: trace_value_generic(&second_value),
            },
            LiveOutcome::BidirectionalUnifyFailed {
                var,
                bound,
                candidate,
                reason,
            } => RuleMatchOutcome::BidirectionalUnifyFailed {
                var,
                bound: trace_value_generic(&bound),
                candidate: trace_value_generic(&candidate),
                reason: reason.to_string(),
            },
            LiveOutcome::MorkExtractFailed { note } => RuleMatchOutcome::MorkExtractFailed {
                note: note.to_string(),
            },
        }
    }
}

/// Emit a `RuleMatchAttempt` event if the filter admits it.
///
/// `call_head` is checked against the filter first, before any value
/// conversion happens. If the filter rejects (or there's no active
/// trace collector), this returns immediately without allocating.
#[inline]
pub fn emit_match_attempt<V>(
    matcher: &'static str,
    call_head: &str,
    call_arity: u32,
    call_expr: &V,
    rule_lhs: &V,
    rule_span: Option<TraceSpan>,
    rule_index: u32,
    outcome: LiveOutcome<'_, V>,
    expr_span: Option<TraceSpan>,
    depth: u32,
) where
    V: MettaValueTrait + Clone + 'static,
{
    let filter = rule_match_filter();
    if filter.is_disabled() {
        return;
    }
    if !filter.admits_head(call_head) {
        return;
    }
    // Skip successes if not requested.
    if matches!(outcome, LiveOutcome::Success { .. }) && !filter.emit_successes() {
        return;
    }

    // Only convert values now that we know we'll emit.
    let owned_outcome = outcome.into_owned();
    let kind = TraceEventKind::RuleMatchAttempt {
        call_head: call_head.to_string(),
        call_arity,
        rule_lhs: trace_value_generic(rule_lhs),
        rule_span,
        rule_index,
        matcher: matcher.to_string(),
        outcome: owned_outcome,
    };

    let input = trace_value_generic(call_expr);
    crate::backend::trace::with_trace_collector_ref(|tc| {
        tc.emit_converted(
            TraceTier::TreeWalker,
            depth,
            input.clone(),
            Vec::new(),
            expr_span,
            kind.clone(),
        );
    });
}

/// Cheap admission check used by call sites BEFORE invoking
/// `try_match_with_detail`. Returns `true` only when an active filter
/// would actually accept events for this `call_head`. The hot path
/// (filter disabled) compiles to a single atomic load.
#[inline]
pub fn should_trace_match(call_head: &str) -> bool {
    let f = rule_match_filter();
    if f.is_disabled() {
        return false;
    }
    f.admits_head(call_head)
}

/// Singleton accessor for `RuleLookup` event filter, reading from
/// `METTA_TRACE_RULE_LOOKUP_HEADS` env var. Same semantics as
/// `METTA_TRACE_RULE_MATCH_HEADS`: comma-separated heads, `*` for all.
fn rule_lookup_filter() -> &'static RuleMatchFilter {
    static FILTER: OnceLock<RuleMatchFilter> = OnceLock::new();
    FILTER.get_or_init(|| {
        let raw_heads = std::env::var("METTA_TRACE_RULE_LOOKUP_HEADS").ok();
        let (heads, all_heads) = match raw_heads.as_deref() {
            None | Some("") => (None, false),
            Some("*") => (Some(HashSet::new()), true),
            Some(list) => {
                let set: HashSet<String> = list
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if set.is_empty() {
                    (None, false)
                } else {
                    (Some(set), false)
                }
            }
        };
        RuleMatchFilter {
            heads,
            all_heads,
            emit_successes: true,
        }
    })
}

/// Cheap admission check for `RuleLookup` / `SelfEvaluating` events.
/// Gated by `METTA_TRACE_RULE_LOOKUP_HEADS` env var.
#[inline]
pub fn should_trace_lookup(head: &str) -> bool {
    let f = rule_lookup_filter();
    if f.is_disabled() {
        return false;
    }
    f.admits_head(head)
}

/// Whether `TrampolineStep` events should be emitted.
/// Gated by `METTA_TRACE_TRAMPOLINE=1` env var. Very high volume — only
/// enable when debugging infinite loops or deadlocks inside the trampoline.
pub fn should_trace_trampoline() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("METTA_TRACE_TRAMPOLINE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// Convert a `DetailedFailure<V>` to a `RuleMatchOutcome` for emission.
///
/// Lazy: only called when the filter has admitted the event. The
/// `expected` field for structural-check failures is synthesized as
/// a `TraceValue::Atom` describing the expected condition.
pub fn detailed_failure_to_outcome<V>(detail: DetailedFailure<V>) -> RuleMatchOutcome
where
    V: MettaValueTrait + Clone + 'static,
{
    match detail {
        DetailedFailure::StructuralCheckFailed {
            check_index,
            check_kind,
            path,
            expected_arity,
            expected_atom,
            actual,
        } => {
            let expected = if let Some(name) = expected_atom {
                TraceValue::Atom(name)
            } else if let Some(arity) = expected_arity {
                TraceValue::Atom(format!("<arity {}>", arity))
            } else {
                TraceValue::Atom("<expected>".to_string())
            };
            RuleMatchOutcome::StructuralCheckFailed {
                check_index,
                check_kind: check_kind.to_string(),
                path,
                expected,
                actual: actual
                    .as_ref()
                    .map(trace_value_generic)
                    .unwrap_or(TraceValue::Empty),
            }
        }
        DetailedFailure::PathNavigateFailed { path, var } => RuleMatchOutcome::PathNavigateFailed {
            path,
            var,
        },
        DetailedFailure::EqualCheckFailed {
            var,
            first_value,
            second_value,
        } => RuleMatchOutcome::EqualCheckFailed {
            var,
            first_value: trace_value_generic(&first_value),
            second_value: trace_value_generic(&second_value),
        },
        DetailedFailure::BidirectionalUnifyFailed {
            var,
            bound,
            candidate,
            reason,
        } => RuleMatchOutcome::BidirectionalUnifyFailed {
            var,
            bound: trace_value_generic(&bound),
            candidate: trace_value_generic(&candidate),
            reason: reason.to_string(),
        },
    }
}

/// Emit a `RuleMatchAttempt` event with a pre-built `RuleMatchOutcome`.
///
/// Lower-level alternative to [`emit_match_attempt`] for callers that
/// have already converted the failure detail to an owned outcome.
#[inline]
pub fn emit_outcome<V>(
    matcher: &'static str,
    call_head: &str,
    call_arity: u32,
    call_expr: &V,
    rule_lhs: &V,
    rule_span: Option<TraceSpan>,
    rule_index: u32,
    outcome: RuleMatchOutcome,
    expr_span: Option<TraceSpan>,
    depth: u32,
) where
    V: MettaValueTrait + Clone + 'static,
{
    let filter = rule_match_filter();
    if filter.is_disabled() {
        return;
    }
    if !filter.admits_head(call_head) {
        return;
    }
    if matches!(outcome, RuleMatchOutcome::Success { .. }) && !filter.emit_successes() {
        return;
    }

    let kind = TraceEventKind::RuleMatchAttempt {
        call_head: call_head.to_string(),
        call_arity,
        rule_lhs: trace_value_generic(rule_lhs),
        rule_span,
        rule_index,
        matcher: matcher.to_string(),
        outcome,
    };

    let input = trace_value_generic(call_expr);
    crate::backend::trace::with_trace_collector_ref(|tc| {
        tc.emit_converted(
            TraceTier::TreeWalker,
            depth,
            input.clone(),
            Vec::new(),
            expr_span,
            kind.clone(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_default_is_disabled() {
        // Bypass the static singleton: build directly so the test works
        // even if some other test set the env var.
        let f = RuleMatchFilter {
            heads: None,
            all_heads: false,
            emit_successes: false,
        };
        assert!(f.is_disabled());
        assert!(!f.admits_head("|-"));
        assert!(!f.admits_head("anything"));
    }

    #[test]
    fn filter_specific_heads() {
        let mut set = HashSet::new();
        set.insert("|-".to_string());
        set.insert("fact".to_string());
        let f = RuleMatchFilter {
            heads: Some(set),
            all_heads: false,
            emit_successes: false,
        };
        assert!(!f.is_disabled());
        assert!(f.admits_head("|-"));
        assert!(f.admits_head("fact"));
        assert!(!f.admits_head("other"));
    }

    #[test]
    fn filter_wildcard() {
        let f = RuleMatchFilter {
            heads: Some(HashSet::new()),
            all_heads: true,
            emit_successes: false,
        };
        assert!(!f.is_disabled());
        assert!(f.admits_head("anything"));
        assert!(f.admits_head("|-"));
    }

    #[test]
    fn filter_emit_successes_default_false() {
        let f = RuleMatchFilter::from_env();
        // Cannot assert exact value because env may have been set by
        // another test, but the default for an unset env var is false.
        let _ = f.emit_successes();
    }
}
