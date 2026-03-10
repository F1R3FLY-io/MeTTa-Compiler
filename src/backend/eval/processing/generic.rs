//! Generic S-Expression Processing
//!
//! This module provides generic versions of S-expression processing functions
//! that work with any value type implementing `MettaValueTrait`. This enables
//! zero-conversion evaluation for both heap and arena allocation modes.

use smallvec::{SmallVec, smallvec};

use crate::backend::environment::GenericEnvironment;
use crate::backend::grounded::{execute_generic_grounded_op, has_generic_grounded_op, GenericGroundedState, GenericGroundedWork};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

#[allow(unused_imports)]
use super::super::trampoline::{
    apply_bindings_generic, try_match_all_rules_generic,
};
// NOTE: pattern_specificity_generic was removed — MeTTa HE has no specificity filter.
use super::super::helpers::needs_special_form_redispatch;

// ============================================================================
// Generic Processing Results
// ============================================================================

/// Result of processing a generic S-expression after argument evaluation.
///
/// Parameterized over value type V and factory type F.
/// Uses GenericEnvironment<V, F> as the environment type.
pub enum GenericProcessedSExpr<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static, F: MettaValueFactory<V> + Clone = crate::backend::models::GcFactory> {
    /// Evaluation complete - return results
    Done((SmallVec<[V; 2]>, GenericEnvironment<V, F>)),

    /// Rule matches found - need to evaluate RHS
    EvalRuleMatches {
        matches: Vec<(V, GenericBindings<V>)>,
        env: GenericEnvironment<V, F>,
        depth: usize,
        base_results: SmallVec<[V; 2]>,
    },

    /// Multiple combinations - need lazy processing
    EvalCombinations {
        combinations: GenericCartesianProductIter<V>,
        env: GenericEnvironment<V, F>,
        depth: usize,
    },

    /// Special form needs redispatch through eval
    RedispatchSExpr {
        items: Vec<V>,
        env: GenericEnvironment<V, F>,
        depth: usize,
    },
}

// ============================================================================
// Generic Cartesian Product
// ============================================================================

/// Generic Cartesian product iterator for nondeterministic evaluation.
#[derive(Debug, Clone)]
pub struct GenericCartesianProductIter<V> {
    /// The input vectors to compute Cartesian product of
    inputs: Vec<Vec<V>>,
    /// Current indices into each input vector
    indices: Vec<usize>,
    /// Whether we've exhausted all combinations
    exhausted: bool,
}

impl<V: Clone> GenericCartesianProductIter<V> {
    /// Get a reference to the input vectors (for GC root collection).
    #[inline]
    pub fn inputs(&self) -> &[Vec<V>] {
        &self.inputs
    }

    /// Create a new Cartesian product iterator.
    pub fn new(inputs: Vec<Vec<V>>) -> Self {
        let exhausted = inputs.iter().any(|v| v.is_empty());
        let indices = vec![0; inputs.len()];
        Self {
            inputs,
            indices,
            exhausted,
        }
    }
}

impl<V: Clone> Iterator for GenericCartesianProductIter<V> {
    type Item = SmallVec<[V; 8]>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }

        // Build current combination
        let combo: SmallVec<[V; 8]> = self
            .inputs
            .iter()
            .zip(self.indices.iter())
            .map(|(vec, &idx)| vec[idx].clone())
            .collect();

        // Advance indices (like incrementing a mixed-radix number)
        let mut i = self.indices.len();
        while i > 0 {
            i -= 1;
            self.indices[i] += 1;
            if self.indices[i] < self.inputs[i].len() {
                break;
            }
            self.indices[i] = 0;
            if i == 0 {
                self.exhausted = true;
            }
        }

        Some(combo)
    }
}

/// Result of lazy Cartesian product generation.
pub enum GenericCartesianProductResult<V: MettaValueTrait> {
    /// No combinations possible (empty input)
    Empty,
    /// Single combination (fast path)
    Single(SmallVec<[V; 8]>),
    /// Multiple combinations (lazy iterator)
    Lazy(GenericCartesianProductIter<V>),
}

/// Generate lazy Cartesian product of evaluation results.
pub fn cartesian_product_lazy_generic<V: MettaValueTrait + Clone>(
    eval_results: Vec<Vec<V>>,
) -> GenericCartesianProductResult<V> {
    // Check for empty inputs
    if eval_results.iter().any(|v| v.is_empty()) {
        return GenericCartesianProductResult::Empty;
    }

    // Check if this is the single-combination fast path
    let total_combinations: usize = eval_results
        .iter()
        .map(|v| v.len())
        .product();

    if total_combinations == 1 {
        // Fast path: single combination
        let combo: SmallVec<[V; 8]> = eval_results
            .into_iter()
            .map(|v| v.into_iter().next().expect("non-empty"))
            .collect();
        return GenericCartesianProductResult::Single(combo);
    }

    // Lazy path: create iterator
    GenericCartesianProductResult::Lazy(GenericCartesianProductIter::new(eval_results))
}

// ============================================================================
// Generic Processing Functions
// ============================================================================

/// Process collected S-expression evaluation results (generic version).
///
/// This is the zero-conversion version of `process_collected_sexpr` that works
/// with any value type implementing `MettaValueTrait`.
pub fn process_collected_sexpr_generic<V, F>(
    collected: Vec<(SmallVec<[V; 2]>, GenericEnvironment<V, F>)>,
    original_env: GenericEnvironment<V, F>,
    depth: usize,
    factory: &F,
) -> GenericProcessedSExpr<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    // Check for errors in sub-expression results
    for (results, new_env) in &collected {
        if let Some(first) = results.first() {
            if first.is_error() {
                return GenericProcessedSExpr::Done((smallvec![first.clone()], new_env.clone()));
            }
        }
    }

    // Split results and environments: convert SmallVec→Vec for Cartesian product
    // (Cartesian product works with Vec<Vec<V>> internally for multi-result cases)
    let (eval_results, envs): (Vec<Vec<V>>, Vec<_>) = collected
        .into_iter()
        .map(|(sv, env)| (sv.into_vec(), env))
        .unzip();

    // Union all environments using optimized batch method
    // This avoids N allocations in the common case where nothing was modified
    let unified_env = original_env.union_all(&envs);

    // Generate lazy Cartesian product of all sub-expression results
    match cartesian_product_lazy_generic(eval_results) {
        GenericCartesianProductResult::Empty => {
            // No combinations possible (empty result list)
            GenericProcessedSExpr::Done((SmallVec::new(), unified_env))
        }
        GenericCartesianProductResult::Single(evaled_items) => {
            // FAST PATH: Single combination (deterministic evaluation)
            process_single_combination_generic(evaled_items.into_vec(), unified_env, depth, factory)
        }
        GenericCartesianProductResult::Lazy(combinations) => {
            // LAZY PATH: Multiple combinations - process via continuation
            GenericProcessedSExpr::EvalCombinations {
                combinations,
                env: unified_env,
                depth,
            }
        }
    }
}

/// Process a single combination (generic version).
///
/// This is the zero-conversion version of `process_single_combination` that
/// checks for grounded operations and rule matches without converting values.
pub fn process_single_combination_generic<V, F>(
    evaled_items: Vec<V>,
    mut unified_env: GenericEnvironment<V, F>,
    depth: usize,
    factory: &F,
) -> GenericProcessedSExpr<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    // Check if this is a grounded operation or special form
    if let Some(first) = evaled_items.first() {
        if let Some(op) = first.as_atom() {
            // First, check for grounded operations using generic registry
            if has_generic_grounded_op(op) {
                let args: Vec<V> = evaled_items[1..].to_vec();
                let mut state = GenericGroundedState::new(op.to_string(), args);

                if let Some(work) = execute_generic_grounded_op(op, &mut state, factory) {
                    match work {
                        GenericGroundedWork::Done(results) => {
                            let values: SmallVec<[V; 2]> = results.into_iter().map(|(v, _)| v).collect();
                            return GenericProcessedSExpr::Done((values, unified_env));
                        }
                        GenericGroundedWork::EvalArg { .. } => {
                            // Grounded op needs argument evaluation - shouldn't happen here
                            // as args are already evaluated. Return as-is for now.
                        }
                        GenericGroundedWork::Error(e) => {
                            let err = factory.error(&format!("{:?}", e), factory.atom("GroundedError"));
                            return GenericProcessedSExpr::Done((smallvec![err], unified_env));
                        }
                    }
                }
            }

            // Re-dispatch special forms through eval_sexpr_step
            if needs_special_form_redispatch(op) {
                return GenericProcessedSExpr::RedispatchSExpr {
                    items: evaled_items,
                    env: unified_env,
                    depth,
                };
            }
        }
    }

    // Try rule matching using generic rule matching (zero-conversion)
    let sexpr = factory.sexpr(evaled_items.clone());
    let all_matches_with_types = try_match_all_rules_generic(&sexpr, &unified_env, *factory);

    if !all_matches_with_types.is_empty() {
        // Rules match with evaluated arguments - evaluate the rule RHS
        // Strip rhs_type from 3-tuples → 2-tuples
        return GenericProcessedSExpr::EvalRuleMatches {
            matches: all_matches_with_types.into_iter()
                .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                .collect(),
            env: unified_env,
            depth,
            base_results: SmallVec::new(),
        };
    }

    // No rules matched - add to space at top level and return as data constructor
    let result = handle_no_rule_match_generic(evaled_items, factory, &mut unified_env, depth);
    GenericProcessedSExpr::Done((smallvec![result], unified_env))
}

/// Handle no rule match (generic version).
///
/// **ADD Mode Semantics**: Only top-level expressions (depth == 0) are added to space.
/// Nested sub-expressions evaluated as arguments are NOT added to space.
/// This matches the heap path behavior in `handle_no_rule_match`.
fn handle_no_rule_match_generic<V, F>(
    evaled_items: Vec<V>,
    factory: &F,
    env: &mut GenericEnvironment<V, F>,
    depth: usize,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let sexpr = factory.sexpr(evaled_items);
    // ADD mode: add to space and return unreduced s-expression
    // In official MeTTa's default ADD mode, bare expressions are automatically added to &self
    // Only add top-level expressions (depth == 0) to space, not nested sub-expressions
    if depth == 0 {
        env.add_to_space(&sexpr);
    }
    sexpr
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_cartesian_product_empty() {
        let inputs: Vec<Vec<MettaValue>> = vec![vec![], vec![MettaValue::Long(1)]];
        match cartesian_product_lazy_generic(inputs) {
            GenericCartesianProductResult::Empty => (),
            _ => panic!("Expected Empty"),
        }
    }

    #[test]
    fn test_cartesian_product_single() {
        let inputs: Vec<Vec<MettaValue>> = vec![
            vec![MettaValue::Long(1)],
            vec![MettaValue::Long(2)],
        ];
        match cartesian_product_lazy_generic(inputs) {
            GenericCartesianProductResult::Single(combo) => {
                assert_eq!(combo.len(), 2);
                assert_eq!(combo[0].as_long(), Some(1));
                assert_eq!(combo[1].as_long(), Some(2));
            }
            _ => panic!("Expected Single"),
        }
    }

    #[test]
    fn test_cartesian_product_lazy() {
        let inputs: Vec<Vec<MettaValue>> = vec![
            vec![MettaValue::Long(1), MettaValue::Long(2)],
            vec![MettaValue::Long(3)],
        ];
        match cartesian_product_lazy_generic(inputs) {
            GenericCartesianProductResult::Lazy(mut iter) => {
                let combo1 = iter.next().unwrap();
                assert_eq!(combo1[0].as_long(), Some(1));
                assert_eq!(combo1[1].as_long(), Some(3));

                let combo2 = iter.next().unwrap();
                assert_eq!(combo2[0].as_long(), Some(2));
                assert_eq!(combo2[1].as_long(), Some(3));

                assert!(iter.next().is_none());
            }
            _ => panic!("Expected Lazy"),
        }
    }
}
