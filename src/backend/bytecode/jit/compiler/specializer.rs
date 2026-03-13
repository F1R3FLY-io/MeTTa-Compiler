//! Profile-Guided Rule Specialization for JIT Stage 2
//!
//! Analyzes runtime type profiles collected during bytecode execution and
//! produces a `SpecializationPlan` that guides Cranelift IR generation.
//!
//! ## Optimization Categories
//!
//! 1. **Branch bias**: Biased branches (>90% one direction) get hot path as
//!    fall-through for better CPU branch prediction.
//! 2. **Monomorphic type specialization**: Call sites where a specific argument
//!    is always the same type skip runtime type tag checks.
//! 3. **Guard elimination**: Guards that never fail during profiling are removed,
//!    with a deoptimization trap as safety net (JIT2 only).
//! 4. **Hot rule hints**: Rule dispatch sites where one rule dominates are
//!    candidates for inlining the hot rule's pattern check.
//!
//! ## Integration
//!
//! The specializer is invoked during JIT Stage 2 compilation:
//! ```text
//! TieredCache::maybe_trigger_jit2()
//!   → snapshot RuntimeTypeProfile
//!   → analyze_profile(profile) → SpecializationPlan
//!   → JitCompiler::compile_with_plan(chunk, plan) → native code
//! ```

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::backend::bytecode::runtime_profile::{RuntimeTypeProfile, TypeTag};
use crate::backend::environment::rule_management::RULE_EPOCH;
use crate::backend::models::MettaValue;

/// A specialization plan produced by analyzing runtime profiles.
///
/// Contains per-offset optimization hints consumed during Cranelift IR generation.
/// The plan is immutable once created — the JIT compiler reads it during codegen.
#[derive(Debug, Default)]
pub struct SpecializationPlan {
    /// Branch bias hints: bytecode offset → which direction is hot.
    /// When the JIT encounters a branch at this offset, it should arrange
    /// blocks so the hot path is the fall-through (better branch prediction).
    pub branch_biases: HashMap<u32, BranchBias>,

    /// Monomorphic type hints: (bytecode offset, arg_index) → expected type.
    /// When the JIT encounters a type-checked operation at this site,
    /// it can skip the tag check and use the specialized operation directly,
    /// with a deoptimization trap if the type doesn't match.
    pub monomorphic_sites: Vec<MonomorphicSite>,

    /// Guards that can be eliminated (never failed during profiling).
    /// The JIT replaces these with a deoptimization trap (uncommon trap)
    /// that bails out to the interpreter if the guard fails.
    pub eliminable_guards: Vec<u16>,

    /// Hot rule dispatch hints: site_hash → (rule_index, match_fraction).
    /// Rules with >80% match frequency at a dispatch site are candidates
    /// for inlining the pattern check (guard + body) at the call site.
    pub hot_rules: Vec<HotRuleHint>,

    /// Detailed rule specialization plans for hot dispatch sites.
    /// Each entry contains the full rule data needed for inline code generation.
    /// Ordered by total_dispatches (hottest first).
    pub specialized_dispatch_sites: Vec<SpecializedDispatchSite>,

    /// Epoch of the RuleIndex when this plan was created.
    /// If the environment's RULE_EPOCH has advanced past this,
    /// the plan is stale and must be regenerated.
    pub rule_epoch: u64,

    /// Overall plan quality score [0.0, 1.0].
    /// Higher means more optimization opportunities found.
    /// Used by the JIT compiler to decide whether specialization is worthwhile.
    pub quality_score: f64,
}

/// A fully-resolved dispatch site ready for specialized code generation.
#[derive(Debug, Clone)]
pub struct SpecializedDispatchSite {
    /// Hash of the dispatch site (expression head + arity).
    pub site_hash: u64,

    /// Head symbol name.
    pub head: String,

    /// Expected arity.
    pub arity: u16,

    /// Rules to inline, ordered by match frequency (hottest first).
    /// The first rule in this list is the "fast path" — its pattern check
    /// is the fall-through in the generated code.
    pub inline_rules: Vec<SpecializedRuleInfo>,

    /// Total dispatches observed during profiling.
    pub total_dispatches: u32,

    /// Fraction of dispatches covered by `inline_rules`.
    /// If < 1.0, a fallback to `jit_runtime_dispatch_rules` is needed.
    pub coverage: f64,
}

/// A rule extracted from the environment, ready for JIT specialization.
///
/// Contains structural matching checks and the RHS body, both in
/// forms suitable for Cranelift IR generation (no trait objects).
#[derive(Debug, Clone)]
pub struct SpecializedRuleInfo {
    /// Structural checks from the StructuralMatcher, translated to
    /// a form the Cranelift codegen can consume.
    pub checks: Vec<InlinableCheck>,

    /// Variable binding operations from the StructuralMatcher.
    pub var_bindings: Vec<InlinableVarOp>,

    /// The RHS body as a MettaValue (for FFI fallback evaluation).
    pub rhs: MettaValue,

    /// Whether the RHS contains variables (if false, skip apply_bindings).
    pub rhs_has_variables: bool,

    /// Hash of the LHS for staleness detection.
    pub lhs_hash: u64,

    /// Return type hint from the type system (for type-specialized codegen).
    pub rhs_type: Option<TypeTag>,
}

/// A structural check translated for Cranelift IR emission.
///
/// Each check is a predicate that must hold for the pattern to match.
/// The path describes navigation from the root S-expression to the
/// element being checked (e.g., `[1, 0]` = first child of second child).
#[derive(Debug, Clone)]
pub enum InlinableCheck {
    /// Check that the value at `path` is an S-expression with the given arity.
    Arity { path: Vec<u8>, expected: u16 },

    /// Check that the value at `path` is the atom with the given name.
    Atom { path: Vec<u8>, expected: &'static str },

    /// Check that the value at `path` is the given integer.
    Long { path: Vec<u8>, expected: i64 },

    /// Check that the value at `path` is the given boolean.
    Bool { path: Vec<u8>, expected: bool },

    /// Check that the value at `path` is the given float (bitwise).
    Float { path: Vec<u8>, expected_bits: u64 },

    /// Check that the value at `path` is the given string.
    Str { path: Vec<u8>, expected: &'static str },
}

/// A variable binding operation translated for Cranelift IR emission.
#[derive(Debug, Clone)]
pub enum InlinableVarOp {
    /// Bind: extract value at `path`, store in binding slot `slot_index`.
    Bind {
        path: Vec<u8>,
        name: &'static str,
        slot_index: u8,
    },

    /// EqualCheck: verify value at `path` equals the value in `bind_slot`.
    /// Used when the same variable appears multiple times in a pattern.
    EqualCheck { path: Vec<u8>, bind_slot: u8 },
}

/// Deoptimization state for a specialized function.
///
/// Stored alongside the NativeCode in ExprCompilationState.
/// Checked at JIT entry — if the epoch has changed, the specialized
/// code is invalidated and execution falls back to JIT Stage 1.
#[derive(Debug, Clone)]
pub struct DeoptimizationGuard {
    /// The RULE_EPOCH when this specialization was compiled.
    /// If the current epoch differs, the function must not be called.
    pub expected_epoch: u64,

    /// Hash of the bytecode chunk that was specialized.
    /// Guards against chunk replacement.
    pub chunk_hash: u64,
}

impl DeoptimizationGuard {
    /// Check if this guard is still valid.
    #[inline]
    pub fn is_valid(&self) -> bool {
        RULE_EPOCH.load(Ordering::Acquire) == self.expected_epoch
    }
}

/// Branch direction bias for a specific jump site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchBias {
    /// The branch is almost always taken (>90%).
    /// Arrange blocks so the taken path is the fall-through.
    MostlyTaken,
    /// The branch is almost always not taken (>90%).
    /// Arrange blocks so the not-taken path is the fall-through.
    MostlyNotTaken,
}

/// Monomorphic type specialization site.
#[derive(Debug, Clone)]
pub struct MonomorphicSite {
    /// Name of the function/operation head (e.g., "+", "==").
    pub head: String,
    /// Argument index (0-based).
    pub arg_index: u8,
    /// The single observed type at this site.
    pub expected_type: TypeTag,
    /// Number of observations (confidence).
    pub observation_count: u32,
}

/// Hot rule hint for dispatch site specialization.
#[derive(Debug, Clone)]
pub struct HotRuleHint {
    /// Hash of the dispatch site.
    pub site_hash: u64,
    /// Index of the hot rule.
    pub rule_index: u16,
    /// Fraction of dispatches that matched this rule [0.0, 1.0].
    pub match_fraction: f64,
    /// Total dispatch count at this site.
    pub total_dispatches: u32,
}

/// Analyze a runtime type profile and produce a specialization plan.
///
/// Returns `None` if the profile is immature (< min_samples at any site)
/// or if no optimization opportunities are found.
///
/// # Arguments
/// * `profile` - Snapshot of the runtime type profile
/// * `min_samples` - Minimum observations required for maturity (default: 50)
pub fn analyze_profile(profile: &RuntimeTypeProfile, min_samples: u32) -> Option<SpecializationPlan> {
    if !profile.is_mature(min_samples) {
        return None;
    }

    let mut plan = SpecializationPlan::default();
    let mut opportunities = 0u32;
    let mut total_sites = 0u32;

    // === 1. Branch bias analysis ===
    for bf in &profile.branch_frequencies {
        total_sites += 1;
        if bf.is_biased() {
            let taken = bf.taken_count();
            let not_taken = bf.not_taken_count();
            let bias = if taken > not_taken {
                BranchBias::MostlyTaken
            } else {
                BranchBias::MostlyNotTaken
            };
            plan.branch_biases.insert(bf.offset, bias);
            opportunities += 1;
        }
    }

    // === 2. Monomorphic type analysis ===
    for atf in &profile.arg_type_feedback {
        total_sites += 1;
        if atf.is_monomorphic() && atf.primary_count >= min_samples {
            plan.monomorphic_sites.push(MonomorphicSite {
                head: atf.head.clone(),
                arg_index: atf.arg_index,
                expected_type: atf.primary_type,
                observation_count: atf.primary_count,
            });
            opportunities += 1;
        }
    }

    // === 3. Guard elimination analysis ===
    for gf in &profile.guard_outcomes {
        total_sites += 1;
        if gf.never_failed() && gf.pass_count >= min_samples {
            plan.eliminable_guards.push(gf.offset);
            opportunities += 1;
        }
    }

    // === 4. Hot rule analysis ===
    // Group rule_match_hits by site_hash, find dominant rules
    let mut site_totals: HashMap<u64, u32> = HashMap::new();
    let mut site_max: HashMap<u64, (u16, u32)> = HashMap::new();

    for rmf in &profile.rule_match_hits {
        *site_totals.entry(rmf.site_hash).or_insert(0) += rmf.match_count;
        let entry = site_max.entry(rmf.site_hash).or_insert((rmf.rule_index, 0));
        if rmf.match_count > entry.1 {
            *entry = (rmf.rule_index, rmf.match_count);
        }
    }

    for (site_hash, total) in &site_totals {
        total_sites += 1;
        if *total >= min_samples {
            if let Some(&(rule_index, match_count)) = site_max.get(site_hash) {
                let fraction = match_count as f64 / *total as f64;
                // Only hint if the dominant rule handles >80% of dispatches
                if fraction > 0.80 {
                    plan.hot_rules.push(HotRuleHint {
                        site_hash: *site_hash,
                        rule_index,
                        match_fraction: fraction,
                        total_dispatches: *total,
                    });
                    opportunities += 1;
                }
            }
        }
    }

    // === 5. Dispatch site analysis (from detailed profiling) ===
    // These are populated by the profiling runtime and contain richer
    // data than rule_match_hits. They identify hot dispatch sites where
    // pattern matching + body evaluation can be inlined.
    for site in &profile.dispatch_sites {
        total_sites += 1;
        if site.total_dispatches >= min_samples {
            if site.dominant_rule().is_some() {
                opportunities += 1;
                // Note: actual SpecializedDispatchSite entries are populated
                // separately by extract_rule_data_for_specialization() which
                // has access to the environment's rule index.
            }
        }
    }

    // === Capture current rule epoch for deoptimization ===
    plan.rule_epoch = RULE_EPOCH.load(Ordering::Acquire);

    // === Compute quality score ===
    plan.quality_score = if total_sites > 0 {
        (opportunities as f64 / total_sites as f64).min(1.0)
    } else {
        0.0
    };

    // Only return plan if there are actual opportunities
    if opportunities > 0 {
        Some(plan)
    } else {
        None
    }
}

impl SpecializationPlan {
    /// Check if a branch at the given bytecode offset has a bias hint.
    #[inline]
    pub fn get_branch_bias(&self, offset: u32) -> Option<BranchBias> {
        self.branch_biases.get(&offset).copied()
    }

    /// Check if a guard at the given offset can be eliminated.
    #[inline]
    pub fn is_guard_eliminable(&self, offset: u16) -> bool {
        self.eliminable_guards.contains(&offset)
    }

    /// Get monomorphic type hints for a given function head.
    pub fn get_monomorphic_hints(&self, head: &str) -> Vec<&MonomorphicSite> {
        self.monomorphic_sites
            .iter()
            .filter(|s| s.head == head)
            .collect()
    }

    /// Get the hot rule hint for a dispatch site, if any.
    pub fn get_hot_rule(&self, site_hash: u64) -> Option<&HotRuleHint> {
        self.hot_rules.iter().find(|h| h.site_hash == site_hash)
    }

    /// Get the specialized dispatch site for a given site hash, if any.
    pub fn get_specialized_site(&self, site_hash: u64) -> Option<&SpecializedDispatchSite> {
        self.specialized_dispatch_sites
            .iter()
            .find(|s| s.site_hash == site_hash)
    }

    /// Check if this plan's deoptimization guard is still valid.
    /// Returns false if the rule epoch has changed since the plan was created.
    #[inline]
    pub fn is_epoch_valid(&self) -> bool {
        RULE_EPOCH.load(Ordering::Acquire) == self.rule_epoch
    }

    /// Returns true if this plan has enough optimization opportunities
    /// to justify specialized compilation (vs. generic JIT1 code).
    pub fn is_worthwhile(&self) -> bool {
        self.quality_score > 0.1
            || !self.branch_biases.is_empty()
            || !self.monomorphic_sites.is_empty()
            || !self.hot_rules.is_empty()
            || !self.specialized_dispatch_sites.is_empty()
    }

    /// Returns the total number of optimization opportunities in this plan.
    pub fn opportunity_count(&self) -> usize {
        self.branch_biases.len()
            + self.monomorphic_sites.len()
            + self.eliminable_guards.len()
            + self.hot_rules.len()
            + self.specialized_dispatch_sites.len()
    }
}

/// Extract rule data from the environment for the given dispatch sites.
///
/// Called in `maybe_trigger_jit2` while the main thread still has access to
/// the environment. Snapshots all rule patterns and bodies needed by the
/// specializer, so the background JIT2 compilation thread can work without
/// holding the environment lock.
///
/// For each dispatch site, finds the matching rules in the RuleIndex,
/// verifies identity via LHS hash, extracts the StructuralMatcher checks
/// and variable operations, and packages them as `SpecializedDispatchSite`.
///
/// # Arguments
/// * `dispatch_sites` - Hot dispatch sites from the RuntimeTypeProfile
/// * `env` - The MeTTa environment (read lock acquired internally)
/// * `rule_epoch` - Current RULE_EPOCH for staleness detection
///
/// # Returns
/// Vec of `SpecializedDispatchSite` with all rule data resolved,
/// ordered by total_dispatches (hottest first). Empty if no
/// specialization opportunities exist.
pub fn extract_rule_data_for_specialization(
    dispatch_sites: &[crate::backend::bytecode::runtime_profile::RuleDispatchSite],
    env: &crate::backend::eval::trampoline::MettaEnvironment,
    _rule_epoch: u64,
) -> Vec<SpecializedDispatchSite> {
    let rule_index = env.shared.rule_index.read();
    let mut result = Vec::with_capacity(dispatch_sites.len());

    for site in dispatch_sites {
        if site.total_dispatches < 50 {
            continue; // Not enough data
        }

        // Get candidate rules from the RuleIndex for this (head, arity)
        let candidates: Vec<_> = rule_index
            .get_candidates(&site.head, site.arity as usize, None)
            .collect();

        if candidates.is_empty() {
            continue;
        }

        let mut inline_rules = Vec::new();
        let mut covered_dispatches: u32 = 0;

        // Sort hits by frequency for specialization priority
        let mut sorted_hits = site.rule_hits.clone();
        sorted_hits.sort_by(|a, b| b.match_count.cmp(&a.match_count));

        for hit in &sorted_hits {
            if hit.rule_index as usize >= candidates.len() {
                continue; // Index out of bounds (stale profile)
            }

            let entry = &candidates[hit.rule_index as usize];

            // Verify rule identity via LHS hash
            // Guards against stale profiles from add-atom/remove-atom
            let current_lhs_hash = hash_metta_value(&entry.lhs);
            if current_lhs_hash != hit.lhs_hash {
                continue; // Stale — rule has changed
            }

            // Extract structural matcher checks if available
            if let Some(ref matcher) = entry.structural_matcher {
                let (checks, var_bindings) = matcher.translate_for_jit();
                let rhs_type = entry.rhs_type.as_ref().map(|t| TypeTag::from_inner(t.inner_ref()));

                inline_rules.push(SpecializedRuleInfo {
                    checks,
                    var_bindings,
                    rhs: entry.rhs.clone(),
                    rhs_has_variables: entry.rhs_has_variables,
                    lhs_hash: current_lhs_hash,
                    rhs_type,
                });
                covered_dispatches += hit.match_count;
            }
        }

        if !inline_rules.is_empty() {
            let coverage = if site.total_dispatches > 0 {
                covered_dispatches as f64 / site.total_dispatches as f64
            } else {
                0.0
            };
            result.push(SpecializedDispatchSite {
                site_hash: site.site_hash,
                head: site.head.clone(),
                arity: site.arity,
                inline_rules,
                total_dispatches: site.total_dispatches,
                coverage,
            });
        }
    }

    // Sort by total dispatches (hottest first)
    result.sort_by(|a, b| b.total_dispatches.cmp(&a.total_dispatches));
    result
}

/// Hash a MettaValue for identity tracking.
///
/// Used by `extract_rule_data_for_specialization` to verify that the rule
/// at a given index hasn't changed since the profile was collected.
fn hash_metta_value(val: &MettaValue) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    val.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::runtime_profile::{
        ArgTypeFeedback, BranchFeedback, GuardFeedback, RuleMatchFeedback,
    };
    use std::sync::atomic::AtomicU32;

    fn make_profile(
        branches: Vec<(u32, u32, u32)>,      // (offset, taken, not_taken)
        args: Vec<(&str, u8, TypeTag, u32)>,  // (head, arg_idx, type, count)
        guards: Vec<(u16, u32, u32)>,         // (offset, pass, fail)
        rules: Vec<(u64, u16, u32)>,          // (site_hash, rule_idx, count)
    ) -> RuntimeTypeProfile {
        RuntimeTypeProfile {
            branch_frequencies: branches
                .into_iter()
                .map(|(offset, taken, not_taken)| BranchFeedback {
                    offset,
                    taken: AtomicU32::new(taken),
                    not_taken: AtomicU32::new(not_taken),
                })
                .collect(),
            arg_type_feedback: args
                .into_iter()
                .map(|(head, arg_index, primary_type, primary_count)| ArgTypeFeedback {
                    head: head.to_string(),
                    arg_index,
                    primary_type,
                    primary_count,
                    secondary_type: None,
                    secondary_count: 0,
                })
                .collect(),
            guard_outcomes: guards
                .into_iter()
                .map(|(offset, pass_count, fail_count)| GuardFeedback {
                    offset,
                    pass_count,
                    fail_count,
                })
                .collect(),
            rule_match_hits: rules
                .into_iter()
                .map(|(site_hash, rule_index, match_count)| RuleMatchFeedback {
                    site_hash,
                    rule_index,
                    match_count,
                })
                .collect(),
            dispatch_sites: smallvec::smallvec![],
            sample_count: 200,
        }
    }

    #[test]
    fn test_analyze_immature_profile() {
        let profile = RuntimeTypeProfile::new();
        assert!(analyze_profile(&profile, 50).is_none());
    }

    #[test]
    fn test_branch_bias_detection() {
        let profile = make_profile(
            vec![(10, 190, 10)], // 95% taken at offset 10
            vec![],
            vec![],
            vec![],
        );
        let plan = analyze_profile(&profile, 50).expect("should produce plan");
        assert_eq!(
            plan.get_branch_bias(10),
            Some(BranchBias::MostlyTaken)
        );
        assert!(plan.get_branch_bias(20).is_none());
    }

    #[test]
    fn test_branch_bias_not_taken() {
        let profile = make_profile(
            vec![(5, 5, 195)], // 97.5% not taken
            vec![],
            vec![],
            vec![],
        );
        let plan = analyze_profile(&profile, 50).expect("should produce plan");
        assert_eq!(
            plan.get_branch_bias(5),
            Some(BranchBias::MostlyNotTaken)
        );
    }

    #[test]
    fn test_monomorphic_site_detection() {
        let profile = make_profile(
            vec![],
            vec![("+", 0, TypeTag::Long, 150)], // Always Long at arg 0
            vec![],
            vec![],
        );
        let plan = analyze_profile(&profile, 50).expect("should produce plan");
        assert_eq!(plan.monomorphic_sites.len(), 1);
        assert_eq!(plan.monomorphic_sites[0].expected_type, TypeTag::Long);
        assert_eq!(plan.monomorphic_sites[0].head, "+");
    }

    #[test]
    fn test_guard_elimination() {
        let profile = make_profile(
            vec![],
            vec![],
            vec![(42, 200, 0)], // Guard at offset 42 never failed
            vec![],
        );
        let plan = analyze_profile(&profile, 50).expect("should produce plan");
        assert!(plan.is_guard_eliminable(42));
        assert!(!plan.is_guard_eliminable(99));
    }

    #[test]
    fn test_guard_not_eliminated_if_failed() {
        let profile = make_profile(
            vec![],
            vec![],
            vec![(42, 180, 20)], // Guard failed 10% of the time
            vec![],
        );
        let plan = analyze_profile(&profile, 50);
        // Plan may still exist (from sample_count maturity) but guard not eliminable
        if let Some(plan) = plan {
            assert!(!plan.is_guard_eliminable(42));
        }
    }

    #[test]
    fn test_hot_rule_detection() {
        let profile = make_profile(
            vec![],
            vec![],
            vec![],
            vec![
                (0xABCD, 0, 170), // Rule 0 matched 85%
                (0xABCD, 1, 30),  // Rule 1 matched 15%
            ],
        );
        let plan = analyze_profile(&profile, 50).expect("should produce plan");
        assert_eq!(plan.hot_rules.len(), 1);
        assert_eq!(plan.hot_rules[0].rule_index, 0);
        assert!(plan.hot_rules[0].match_fraction > 0.80);
    }

    #[test]
    fn test_no_hot_rule_if_evenly_split() {
        let profile = make_profile(
            vec![],
            vec![],
            vec![],
            vec![
                (0xABCD, 0, 100), // 50/50 split
                (0xABCD, 1, 100),
            ],
        );
        let plan = analyze_profile(&profile, 50);
        if let Some(plan) = plan {
            assert!(plan.hot_rules.is_empty());
        }
    }

    #[test]
    fn test_quality_score() {
        let profile = make_profile(
            vec![(10, 190, 10), (20, 100, 100)], // 1 biased, 1 not
            vec![("+", 0, TypeTag::Long, 150)],   // 1 monomorphic
            vec![(42, 200, 0)],                    // 1 eliminable guard
            vec![],
        );
        let plan = analyze_profile(&profile, 50).expect("should produce plan");
        // 3 opportunities out of 4 sites = 0.75
        assert!(plan.quality_score > 0.5);
        assert!(plan.is_worthwhile());
    }

    #[test]
    fn test_no_opportunities() {
        // Profile is mature but no optimization opportunities
        let profile = make_profile(
            vec![(10, 100, 100)], // 50/50 branch
            vec![],
            vec![(42, 100, 100)], // Guard fails 50% of the time
            vec![],
        );
        let result = analyze_profile(&profile, 50);
        // Should be None since no opportunities found
        assert!(result.is_none());
    }
}
