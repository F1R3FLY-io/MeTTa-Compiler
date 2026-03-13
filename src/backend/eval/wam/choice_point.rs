//! WAM Choice Points: All-solutions nondeterministic branching.
//!
//! Unlike Prolog's depth-first single-solution model, MeTTa requires ALL matching
//! rules to fire and results to be accumulated as an unordered set. Choice points
//! in the MeTTa-WAM track remaining alternatives and accumulate results from
//! explored branches.
//!
//! # Difference from Classical WAM
//!
//! | Classical WAM (Prolog) | MeTTa-WAM |
//! |----------------------|-----------|
//! | First solution returned via `Proceed` | All solutions accumulated in `results` |
//! | Backtrack via `Fail` to find next | After each branch, always try next |
//! | `cut` (!) prunes remaining alternatives | No cut — all alternatives explored |
//! | Choice point removed on `TrustMe` | Choice point removed after all alts done |

use std::sync::Arc;

use crate::backend::eval::trampoline::MettaEnvironment;
use crate::backend::models::MettaValue;

use super::compiler::WamCode;

/// A single untried alternative in a choice point.
///
/// Represents a compiled rule that could potentially match the current expression.
/// The match is attempted lazily — the WamCode is executed when this alternative
/// is selected, not eagerly when the choice point is created.
#[derive(Clone, Debug)]
pub struct WamAlternative {
    /// Compiled WAM instruction sequence for this rule's LHS matching.
    pub match_code: Arc<WamCode>,
    /// RHS template to evaluate if the LHS matches.
    pub rhs_template: MettaValue,
    /// Whether the RHS template contains variables (for apply_bindings skip optimization).
    /// When `false`, the RHS can be used directly without substitution.
    pub rhs_has_variables: bool,
    /// Cached return type of the RHS (for expected_type pruning).
    pub rhs_type: Option<MettaValue>,
    /// Rule multiplicity (how many times this rule was defined).
    pub multiplicity: u64,
}

/// WAM choice point for nondeterministic branching.
///
/// Created by `TryMeElse` when multiple rules match an expression.
/// Each alternative is tried in sequence; after each, the trail is unwound
/// to the `trail_mark` and the binding frame is reset to `frame_slots` size.
///
/// Unlike Prolog, ALL alternatives are explored and results are accumulated
/// in the `results` vector. The choice point is removed only after the last
/// alternative (`TrustMe`) completes.
#[derive(Clone, Debug)]
pub struct WamChoicePoint {
    /// Trail position at choice point creation. On backtrack, the trail
    /// is unwound to this mark to restore bindings.
    pub trail_mark: usize,
    /// Number of binding frame slots at choice point creation.
    /// Used to trim the frame back to its state before this alternative.
    pub frame_slots: usize,
    /// Index of the next alternative to try (0-based into `alternatives`).
    pub next_alternative: usize,
    /// All remaining alternatives for this choice point.
    pub alternatives: Vec<WamAlternative>,
    /// Accumulated results from already-explored branches.
    pub results: Vec<MettaValue>,
    /// Environment at choice point creation (Arc clone = O(1)).
    pub env: MettaEnvironment,
    /// Evaluation depth at choice point creation.
    pub depth: usize,
    /// Parallel branching budget for this choice point.
    /// When > 0 and multiple alternatives remain, alternatives may be
    /// dispatched to the work pool for parallel evaluation.
    pub parallel_budget: u32,
}

impl WamChoicePoint {
    /// Create a new choice point with the given alternatives.
    pub fn new(
        trail_mark: usize,
        frame_slots: usize,
        alternatives: Vec<WamAlternative>,
        env: MettaEnvironment,
        depth: usize,
    ) -> Self {
        WamChoicePoint {
            trail_mark,
            frame_slots,
            next_alternative: 0,
            alternatives,
            results: Vec::new(),
            env,
            depth,
            parallel_budget: 0,
        }
    }

    /// Check if there are more alternatives to try.
    #[inline]
    pub fn has_more_alternatives(&self) -> bool {
        self.next_alternative < self.alternatives.len()
    }

    /// Get the next alternative and advance the index.
    /// Returns `None` if all alternatives have been tried.
    #[inline]
    pub fn next_alternative(&mut self) -> Option<&WamAlternative> {
        if self.next_alternative < self.alternatives.len() {
            let idx = self.next_alternative;
            self.next_alternative += 1;
            Some(&self.alternatives[idx])
        } else {
            None
        }
    }

    /// Number of remaining alternatives (including the current one).
    #[inline]
    pub fn remaining_count(&self) -> usize {
        self.alternatives.len().saturating_sub(self.next_alternative)
    }

    /// Add a result from an explored branch.
    #[inline]
    pub fn push_result(&mut self, result: MettaValue) {
        self.results.push(result);
    }

    /// Add multiple results from an explored branch.
    pub fn extend_results(&mut self, results: impl IntoIterator<Item = MettaValue>) {
        self.results.extend(results);
    }

    /// Collect all MettaValues for GC root reporting.
    pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
        // Results
        out.extend(self.results.iter().copied());
        // Alternative RHS templates
        for alt in &self.alternatives {
            out.push(alt.rhs_template);
            if let Some(rhs_type) = alt.rhs_type {
                out.push(rhs_type);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::trampoline::new_env;
    use crate::backend::models::MettaValue;

    fn make_test_alternative(rhs: MettaValue) -> WamAlternative {
        WamAlternative {
            match_code: Arc::new(WamCode {
                instructions: Vec::new(),
                num_slots: 0,
                slot_names: Vec::new(),
                rhs_templates: Vec::new(),
                constants: Vec::new(),
            }),
            rhs_template: rhs,
            rhs_has_variables: false,
            rhs_type: None,
            multiplicity: 1,
        }
    }

    #[test]
    fn test_choice_point_creation() {
        let env = new_env();
        let cp = WamChoicePoint::new(
            0,
            2,
            vec![
                make_test_alternative(MettaValue::Long(1)),
                make_test_alternative(MettaValue::Long(2)),
                make_test_alternative(MettaValue::Long(3)),
            ],
            env,
            0,
        );
        assert_eq!(cp.remaining_count(), 3);
        assert!(cp.has_more_alternatives());
        assert!(cp.results.is_empty());
    }

    #[test]
    fn test_choice_point_iteration() {
        let env = new_env();
        let mut cp = WamChoicePoint::new(
            0,
            0,
            vec![
                make_test_alternative(MettaValue::Long(10)),
                make_test_alternative(MettaValue::Long(20)),
            ],
            env,
            0,
        );

        let alt1 = cp.next_alternative().expect("should have alt 1");
        assert_eq!(alt1.rhs_template, MettaValue::Long(10));
        assert_eq!(cp.remaining_count(), 1);

        let alt2 = cp.next_alternative().expect("should have alt 2");
        assert_eq!(alt2.rhs_template, MettaValue::Long(20));
        assert_eq!(cp.remaining_count(), 0);

        assert!(cp.next_alternative().is_none());
        assert!(!cp.has_more_alternatives());
    }

    #[test]
    fn test_choice_point_result_accumulation() {
        let env = new_env();
        let mut cp = WamChoicePoint::new(0, 0, Vec::new(), env, 0);

        cp.push_result(MettaValue::Long(1));
        cp.push_result(MettaValue::Long(2));
        cp.extend_results(vec![MettaValue::Long(3), MettaValue::Long(4)]);

        assert_eq!(cp.results.len(), 4);
        assert_eq!(cp.results[0], MettaValue::Long(1));
        assert_eq!(cp.results[3], MettaValue::Long(4));
    }

    #[test]
    fn test_choice_point_gc_roots() {
        let env = new_env();
        let mut cp = WamChoicePoint::new(
            0,
            0,
            vec![make_test_alternative(MettaValue::Long(100))],
            env,
            0,
        );
        cp.push_result(MettaValue::Long(42));

        let mut roots = Vec::new();
        cp.collect_gc_roots(&mut roots);
        // Should include: 1 result + 1 alternative rhs_template
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&MettaValue::Long(42)));
        assert!(roots.contains(&MettaValue::Long(100)));
    }
}
