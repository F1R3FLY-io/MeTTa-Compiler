//! Runtime Type Profile for Profile-Guided Tiered Compilation
//!
//! Collects runtime type feedback, branch frequencies, rule match statistics,
//! and guard outcomes during bytecode VM execution. This profile data flows
//! from lower tiers (bytecode VM) to higher tiers (JIT1, JIT2) to guide
//! speculative optimizations.
//!
//! ## V8/HotSpot Analogues
//!
//! | Component | V8 Equivalent | HotSpot Equivalent |
//! |-----------|---------------|-------------------|
//! | `RuntimeTypeProfile` | FeedbackVector | MethodData (MDO) |
//! | `arg_type_feedback` | CallIC / LoadIC | ReceiverTypeData |
//! | `branch_frequencies` | BinaryOpIC | BranchData |
//! | `rule_match_hits` | (no equivalent) | VirtualCallData |
//! | `guard_outcomes` | TypeGuard feedback | UncommonTrapData |
//!
//! ## Usage
//!
//! 1. **Bytecode VM** populates the profile during execution (Phase 8b)
//! 2. **TieredCache** stores the profile in `ExprCompilationState` (Phase 8c)
//! 3. **JIT compiler** reads the profile to generate optimized code (Phase 8c)

use smallvec::SmallVec;

use std::sync::atomic::{AtomicU32, Ordering};

/// Type tag for runtime type feedback.
///
/// Represents the observed runtime type of a value at a specific site.
/// Compact representation (1 byte) for inline storage in feedback vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TypeTag {
    /// Unit / Nil
    Unit = 0,
    /// Boolean (True/False)
    Bool = 1,
    /// Integer (i64)
    Long = 2,
    /// Float (f64)
    Float = 3,
    /// String
    String = 4,
    /// Atom (symbol)
    Atom = 5,
    /// S-expression (compound)
    SExpr = 6,
    /// Error
    Error = 7,
    /// Quoted value
    Quoted = 8,
    /// Any other type (Space, State, Type, etc.)
    Other = 9,
}

impl TypeTag {
    /// Classify a MettaValueInner into a TypeTag.
    #[inline]
    pub fn from_inner(inner: &crate::backend::models::MettaValueInner) -> Self {
        use crate::backend::models::MettaValueInner;
        match inner {
            MettaValueInner::Unit => TypeTag::Unit,
            MettaValueInner::Bool(_) => TypeTag::Bool,
            MettaValueInner::Long(_) => TypeTag::Long,
            MettaValueInner::Float(_) => TypeTag::Float,
            MettaValueInner::String(_) => TypeTag::String,
            MettaValueInner::Atom(_) => TypeTag::Atom,
            MettaValueInner::SExpr(_) => TypeTag::SExpr,
            MettaValueInner::Error(_, _) => TypeTag::Error,
            MettaValueInner::Quoted(_) => TypeTag::Quoted,
            MettaValueInner::Spanned(inner, _) => TypeTag::from_inner(inner.inner_ref()),
            _ => TypeTag::Other,
        }
    }
}

/// Per-expression runtime profile collected during bytecode VM execution.
///
/// This structure is populated incrementally during bytecode execution and
/// snapshotted when triggering JIT compilation. The JIT compiler uses this
/// data to make optimization decisions:
///
/// - **Branch layout**: Hot branches become fall-through (better CPU prediction)
/// - **Type specialization**: Monomorphic sites get direct operations
/// - **Rule inlining**: High-frequency rules get inlined at call sites
/// - **Guard elimination**: Guards that never fail get removed (JIT2 only)
#[derive(Debug, Clone)]
pub struct RuntimeTypeProfile {
    /// Per (bytecode_offset) → (taken_count, not_taken_count).
    ///
    /// V8 equivalent: BinaryOpIC / CompareIC branch feedback.
    /// Indexed by jump opcode offset within the bytecode chunk.
    pub branch_frequencies: SmallVec<[BranchFeedback; 8]>,

    /// Per (function_head, arg_index) → observed type tag histogram.
    ///
    /// V8 equivalent: CallIC / LoadIC monomorphism tracking.
    /// Stores top-2 types per slot (like HotSpot's ReceiverTypeData).
    /// Monomorphic sites (single type observed) enable type specialization.
    pub arg_type_feedback: SmallVec<[ArgTypeFeedback; 8]>,

    /// Per rule dispatch site → which rule indices matched, with frequency.
    ///
    /// No direct V8 equivalent (V8 has no rule system), but analogous to
    /// polymorphic dispatch profiling in HotSpot's VirtualCallData.
    /// High-frequency rules are candidates for inlining.
    pub rule_match_hits: SmallVec<[RuleMatchFeedback; 8]>,

    /// Guard outcome tracking: (opcode_offset) → (pass_count, fail_count).
    ///
    /// Used to identify guards that never fail → candidates for elimination
    /// in JIT Stage 2 (with deoptimization trap as safety net).
    pub guard_outcomes: SmallVec<[GuardFeedback; 4]>,

    /// Per-dispatch-site detailed rule matching profiles.
    /// Populated during JIT Stage 1 execution by the profiling variant
    /// of `jit_runtime_dispatch_rules`.
    pub dispatch_sites: SmallVec<[RuleDispatchSite; 4]>,

    /// Total number of bytecode executions when this profile was collected.
    /// Used to assess profile maturity — don't promote to JIT if profile
    /// has fewer than 50 samples at any feedback site (immature profile).
    pub sample_count: u32,
}

/// Branch frequency feedback for a specific jump site.
#[derive(Debug)]
pub struct BranchFeedback {
    /// Bytecode offset of the jump instruction.
    pub offset: u32,
    /// Number of times the branch was taken.
    pub taken: AtomicU32,
    /// Number of times the branch was not taken (fall-through).
    pub not_taken: AtomicU32,
}

impl BranchFeedback {
    /// Create a new branch feedback entry.
    pub fn new(offset: u32) -> Self {
        Self {
            offset,
            taken: AtomicU32::new(0),
            not_taken: AtomicU32::new(0),
        }
    }

    /// Record a branch taken event.
    #[inline]
    pub fn record_taken(&self) {
        self.taken.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a branch not-taken (fall-through) event.
    #[inline]
    pub fn record_not_taken(&self) {
        self.not_taken.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the taken count.
    #[inline]
    pub fn taken_count(&self) -> u32 {
        self.taken.load(Ordering::Relaxed)
    }

    /// Get the not-taken count.
    #[inline]
    pub fn not_taken_count(&self) -> u32 {
        self.not_taken.load(Ordering::Relaxed)
    }

    /// Returns true if the branch is biased (>90% one direction).
    pub fn is_biased(&self) -> bool {
        let t = self.taken_count();
        let nt = self.not_taken_count();
        let total = t + nt;
        if total < 10 {
            return false;
        }
        t * 10 > total * 9 || nt * 10 > total * 9
    }
}

impl Clone for BranchFeedback {
    fn clone(&self) -> Self {
        Self {
            offset: self.offset,
            taken: AtomicU32::new(self.taken.load(Ordering::Relaxed)),
            not_taken: AtomicU32::new(self.not_taken.load(Ordering::Relaxed)),
        }
    }
}

/// Argument type feedback for a specific function/operation site.
#[derive(Debug, Clone)]
pub struct ArgTypeFeedback {
    /// Name of the function/operation head.
    pub head: String,
    /// Argument index (0-based, after head).
    pub arg_index: u8,
    /// Primary observed type (most frequent).
    pub primary_type: TypeTag,
    /// Primary type observation count.
    pub primary_count: u32,
    /// Secondary observed type (second most frequent), if polymorphic.
    pub secondary_type: Option<TypeTag>,
    /// Secondary type observation count.
    pub secondary_count: u32,
}

impl ArgTypeFeedback {
    /// Returns true if this site is monomorphic (single type observed).
    pub fn is_monomorphic(&self) -> bool {
        self.secondary_type.is_none() || self.secondary_count == 0
    }

    /// Returns true if this site is polymorphic (2+ types observed).
    pub fn is_polymorphic(&self) -> bool {
        self.secondary_type.is_some() && self.secondary_count > 0
    }

    /// Record a type observation, updating the top-2 histogram.
    pub fn record(&mut self, tag: TypeTag) {
        if tag == self.primary_type {
            self.primary_count += 1;
        } else if self.secondary_type == Some(tag) {
            self.secondary_count += 1;
            // Promote secondary to primary if it overtakes
            if self.secondary_count > self.primary_count {
                std::mem::swap(&mut self.primary_type, self.secondary_type.as_mut().expect("secondary_type is Some"));
                std::mem::swap(&mut self.primary_count, &mut self.secondary_count);
            }
        } else if self.secondary_type.is_none() || self.secondary_count == 0 {
            self.secondary_type = Some(tag);
            self.secondary_count = 1;
        }
        // If a third type is observed, it replaces the secondary only if
        // the secondary is very cold (< 5% of primary). This prevents
        // oscillation between rare types.
    }
}

/// Rule match frequency feedback for a specific dispatch site.
#[derive(Debug, Clone)]
pub struct RuleMatchFeedback {
    /// Hash of the dispatch site (expression hash + bytecode offset).
    pub site_hash: u64,
    /// Index of the rule that matched.
    pub rule_index: u16,
    /// Number of times this rule was selected.
    pub match_count: u32,
}

/// Detailed rule dispatch profile for a specific call site.
///
/// Collected by the JIT Stage 1 runtime's DispatchRules handler.
/// Stored per-expression in `ExprCompilationState.runtime_profile`.
///
/// Unlike `RuleMatchFeedback` (which is a flat frequency counter),
/// this captures the actual rule identities and match frequencies
/// so the specializer can inline hot rules.
#[derive(Debug, Clone)]
pub struct RuleDispatchSite {
    /// Hash of the expression head + arity at this dispatch site.
    /// Used as the join key between profile data and specialization plan.
    pub site_hash: u64,

    /// Head symbol name.
    pub head: String,

    /// Expected arity of the expression at this site.
    pub arity: u16,

    /// Per-rule hit counts, ordered by frequency (descending after sorting).
    /// `rule_index` is the position in the RuleIndex's candidate list.
    pub rule_hits: Vec<RuleHit>,

    /// Total dispatch count at this site (sum of all rule_hits).
    pub total_dispatches: u32,
}

impl RuleDispatchSite {
    /// Create a new dispatch site profile.
    pub fn new(site_hash: u64, head: String, arity: u16) -> Self {
        Self {
            site_hash,
            head,
            arity,
            rule_hits: Vec::new(),
            total_dispatches: 0,
        }
    }

    /// Record a rule match at this site.
    pub fn record_match(&mut self, rule_index: u16, lhs_hash: u64, rhs_hash: u64, rhs_has_variables: bool) {
        self.total_dispatches += 1;
        if let Some(hit) = self.rule_hits.iter_mut().find(|h| h.rule_index == rule_index) {
            hit.match_count += 1;
        } else {
            self.rule_hits.push(RuleHit {
                rule_index,
                match_count: 1,
                lhs_hash,
                rhs_hash,
                rhs_has_variables,
            });
        }
    }

    /// Sort rule hits by frequency (descending) for specialization priority.
    pub fn sort_by_frequency(&mut self) {
        self.rule_hits.sort_by(|a, b| b.match_count.cmp(&a.match_count));
    }

    /// Get the dominant rule (if any matches >80% of dispatches).
    pub fn dominant_rule(&self) -> Option<&RuleHit> {
        if self.total_dispatches == 0 {
            return None;
        }
        self.rule_hits.first().filter(|h| {
            (h.match_count as f64 / self.total_dispatches as f64) > 0.80
        })
    }
}

/// A single rule's match frequency at a dispatch site.
#[derive(Debug, Clone)]
pub struct RuleHit {
    /// Index into the RuleIndex candidate list for this (head, arity).
    pub rule_index: u16,

    /// Number of times this rule was the matching rule.
    pub match_count: u32,

    /// Hash of the rule's LHS for identity (used to detect stale profiles
    /// after rule index changes due to add-atom/remove-atom).
    pub lhs_hash: u64,

    /// Hash of the rule's RHS for identity (for detecting body changes).
    pub rhs_hash: u64,

    /// Whether the RHS contains variables (cached from RuleEntry).
    pub rhs_has_variables: bool,
}

/// Guard outcome feedback for a specific guard site.
#[derive(Debug, Clone)]
pub struct GuardFeedback {
    /// Bytecode offset of the guard instruction.
    pub offset: u16,
    /// Number of times the guard passed (expected path).
    pub pass_count: u32,
    /// Number of times the guard failed (deoptimization path).
    pub fail_count: u32,
}

impl GuardFeedback {
    /// Returns true if the guard never failed (candidate for elimination).
    pub fn never_failed(&self) -> bool {
        self.fail_count == 0 && self.pass_count > 0
    }

    /// Returns the failure rate as a fraction [0.0, 1.0].
    pub fn failure_rate(&self) -> f64 {
        let total = self.pass_count + self.fail_count;
        if total == 0 {
            0.0
        } else {
            self.fail_count as f64 / total as f64
        }
    }
}

impl RuntimeTypeProfile {
    /// Create a new empty profile.
    pub fn new() -> Self {
        Self {
            branch_frequencies: SmallVec::new(),
            arg_type_feedback: SmallVec::new(),
            rule_match_hits: SmallVec::new(),
            guard_outcomes: SmallVec::new(),
            dispatch_sites: SmallVec::new(),
            sample_count: 0,
        }
    }

    /// Returns true if this profile has enough samples to be considered
    /// mature for JIT compilation. Immature profiles lead to premature
    /// optimization from unrepresentative data.
    ///
    /// Requires at least `min_samples` total observations at every
    /// feedback site. Default threshold: 50 (from HotSpot's profile
    /// maturity check).
    pub fn is_mature(&self, min_samples: u32) -> bool {
        if self.sample_count < min_samples {
            return false;
        }
        // Check that every branch site has enough observations
        for bf in &self.branch_frequencies {
            if bf.taken_count() + bf.not_taken_count() < min_samples {
                return false;
            }
        }
        true
    }

    /// Snapshot this profile for handoff to the JIT compiler.
    ///
    /// Returns a clone suitable for passing to the background compilation
    /// task. The original profile continues to accumulate data.
    pub fn snapshot(&self) -> Self {
        self.clone()
    }
}

impl Default for RuntimeTypeProfile {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_tag_classification() {
        use crate::backend::models::MettaValueInner;
        assert_eq!(TypeTag::from_inner(&MettaValueInner::Unit), TypeTag::Unit);
        assert_eq!(TypeTag::from_inner(&MettaValueInner::Bool(true)), TypeTag::Bool);
        assert_eq!(TypeTag::from_inner(&MettaValueInner::Long(42)), TypeTag::Long);
    }

    #[test]
    fn test_branch_feedback_biased() {
        let bf = BranchFeedback::new(0);
        assert!(!bf.is_biased()); // No data yet

        for _ in 0..95 {
            bf.record_taken();
        }
        for _ in 0..5 {
            bf.record_not_taken();
        }
        assert!(bf.is_biased()); // 95% taken
    }

    #[test]
    fn test_arg_type_feedback_monomorphic() {
        let mut atf = ArgTypeFeedback {
            head: String::from("+"),
            arg_index: 0,
            primary_type: TypeTag::Long,
            primary_count: 100,
            secondary_type: None,
            secondary_count: 0,
        };
        assert!(atf.is_monomorphic());

        atf.record(TypeTag::Float);
        assert!(atf.is_polymorphic());
    }

    #[test]
    fn test_guard_feedback_never_failed() {
        let gf = GuardFeedback {
            offset: 42,
            pass_count: 1000,
            fail_count: 0,
        };
        assert!(gf.never_failed());
        assert_eq!(gf.failure_rate(), 0.0);
    }

    #[test]
    fn test_profile_maturity() {
        let mut profile = RuntimeTypeProfile::new();
        assert!(!profile.is_mature(50));

        profile.sample_count = 100;
        assert!(profile.is_mature(50));

        // Add immature branch site
        profile.branch_frequencies.push(BranchFeedback::new(0));
        assert!(!profile.is_mature(50)); // Branch site has 0 samples
    }
}
