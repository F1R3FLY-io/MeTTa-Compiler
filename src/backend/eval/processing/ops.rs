//! Generic S-Expression Processing
//!
//! This module provides generic versions of S-expression processing functions
//! that work with any value type implementing `MettaValueTrait`. This enables
//! zero-conversion evaluation for both heap and arena allocation modes.

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use smallvec::{smallvec, SmallVec};

use crate::backend::environment::GenericEnvironment;
use crate::backend::grounded::{execute_grounded_op, has_grounded_op, GroundedState, GroundedWork};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

use super::super::trampoline::try_match_all_rules;
use crate::backend::eval::bindings::compose_outer_inner_generic;
use crate::backend::eval::trampoline::types::{bv_with, BoundValue};
use crate::backend::models::{GcFactory, MettaValue};
// NOTE: pattern_specificity_generic was removed — MeTTa HE has no specificity filter.
use super::super::helpers::needs_special_form_redispatch;

// ============================================================================
// Generic Processing Results
// ============================================================================

/// Result of processing a generic S-expression after argument evaluation.
///
/// Parameterized over value type V and factory type F.
/// Uses GenericEnvironment<V, F> as the environment type.
pub enum GenericProcessedSExpr<
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone = crate::backend::models::GcFactory,
> {
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
    let total_combinations: usize = eval_results.iter().map(|v| v.len()).product();

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

/// Type alias for the concrete environment.
type MettaEnvironment = GenericEnvironment<MettaValue, GcFactory>;

/// Process collected S-expression evaluation results.
///
/// Operates on concrete `MettaValue` / `GcFactory` types.
pub fn process_collected_sexpr_generic(
    collected: Vec<(SmallVec<[MettaValue; 2]>, MettaEnvironment)>,
    original_env: MettaEnvironment,
    depth: usize,
    factory: &GcFactory,
) -> GenericProcessedSExpr<MettaValue, GcFactory> {
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
    let (eval_results, envs): (Vec<Vec<MettaValue>>, Vec<_>) = collected
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

/// Process a single combination.
///
/// Checks for grounded operations and rule matches without converting values.
/// Operates on concrete `MettaValue` / `GcFactory` types.
pub fn process_single_combination_generic(
    evaled_items: Vec<MettaValue>,
    mut unified_env: MettaEnvironment,
    depth: usize,
    factory: &GcFactory,
) -> GenericProcessedSExpr<MettaValue, GcFactory> {
    // Check if this is a grounded operation or special form
    if let Some(first) = evaled_items.first() {
        if let Some(op) = first.as_atom() {
            // First, check for grounded operations using generic registry
            if has_grounded_op(op) {
                let args: Vec<MettaValue> = evaled_items[1..].to_vec();
                let mut state = GroundedState::new(op.to_string(), args);

                if let Some(work) = execute_grounded_op(op, &mut state, factory) {
                    match work {
                        GroundedWork::Done(results) => {
                            let values: SmallVec<[MettaValue; 2]> =
                                results.into_iter().map(|(v, _)| v).collect();
                            return GenericProcessedSExpr::Done((values, unified_env));
                        }
                        GroundedWork::EvalArg { .. } => {
                            // Grounded op needs argument evaluation - shouldn't happen here
                            // as args are already evaluated. Return as-is for now.
                        }
                        GroundedWork::Error(e) => {
                            let err = factory.error(
                                factory.string(&format!("{:?}", e)),
                                factory.atom("GroundedError"),
                            );
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
    // Build sexpr once and reuse for both rule matching and no-match fallback
    let sexpr = factory.sexpr(evaled_items);
    let all_matches_with_types = try_match_all_rules(&sexpr, &unified_env, *factory);

    if !all_matches_with_types.is_empty() {
        // Rules match with evaluated arguments - evaluate the rule RHS
        // Strip rhs_type from 3-tuples → 2-tuples
        return GenericProcessedSExpr::EvalRuleMatches {
            matches: all_matches_with_types
                .into_iter()
                .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                .collect(),
            env: unified_env,
            depth,
            base_results: SmallVec::new(),
        };
    }

    // No rules matched.
    //
    // Phase 2.B HE-bisimilarity fix: distinguish "function with no matching
    // rules" from "data constructor". Data constructors (heads never
    // registered with a rule) evaluate to themselves — MeTTa's ADD-mode data
    // semantics. Functions (heads that have SOME rule but none match these
    // args) produce EMPTY — this is how HE's `match` semantics work and
    // prevents ghost results from flowing through conjunctions where an
    // argument-bound call like `(father c $x)` fails to match any rule.
    //
    // At depth == 0 (top level), HE empirically returns the unreduced
    // expression — verified via metta-repl: `(= (f 1) good) !(f 2)` →
    // `[(f 2)]`. So depth==0 falls through to ADD-mode handling below,
    // which returns the unreduced sexpr. Only depth > 0 (inside
    // conjunctions, match goals, etc.) emits Empty per HE pattern-fail
    // semantics. This matches the bound path which has always gated on
    // `depth > 0`.
    if depth > 0 {
        if let Some(head) = sexpr.get_head_symbol() {
            let arity = sexpr.get_arity();
            let has_any_rules = unified_env
                .shared
                .rule_index
                .read()
                .get_candidates(head, arity, None)
                .next()
                .is_some();
            if has_any_rules {
                // Function with no matching rules → empty (HE semantics).
                return GenericProcessedSExpr::Done((SmallVec::new(), unified_env));
            }
        }
    }

    // X.6 MTT-TI-029: top-level auto-add removed from T0 trampoline. The
    // T1 op_dispatch_rules path (vm/mod.rs:6539-6546) is the single source
    // of truth for data-constructor / top-level fact auto-add, and it
    // already has match_space_exists dedup. T0 originally double-added when
    // tier promotion ran the same expression in both tiers, producing
    // duplicate match results (M04/002).
    //
    // Previously: `if depth == 0 { unified_env.add_to_space(&sexpr); }`

    // S1 TOPLEVEL (2026-05-13): HE two-mode runner. Bare top-level
    // (depth==0) S-exprs are ADD-mode candidates — silent side-effecting
    // facts (HE `module.add_atom(atom)` per runner/mod.rs:1076-1083).
    // Mirrors the T1 VM auto-add at vm/mod.rs:6751 — both tiers must
    // perform the same fact insertion to keep `&self` consistent across
    // tier-promoted vs T0-only execution.
    //
    // Regardless of interpret_mode, the fact is added to space (dedup'd
    // via match_space_exists). The OUTPUT differs:
    //   - !interpret_mode (HE ADD mode): emit NOTHING
    //   - interpret_mode (HE INTERPRET): emit the unchanged data
    //     constructor (the runner-level structural is-bang filter then
    //     suppresses output lines from non-bang directives).
    if depth == 0 {
        if !unified_env.match_space_exists(&sexpr) {
            unified_env.add_to_space(&sexpr);
        }
        if !unified_env.in_interpret_mode() {
            return GenericProcessedSExpr::Done((SmallVec::new(), unified_env));
        }
    }
    GenericProcessedSExpr::Done((smallvec![sexpr], unified_env))
}

/// Handle no rule match (generic version).
///
/// **ADD Mode Semantics**: Only top-level expressions (depth == 0) are added to space.
/// Nested sub-expressions evaluated as arguments are NOT added to space.
/// This matches the heap path behavior in `handle_no_rule_match`.
// NOTE: Currently unused — process_single_combination_generic inlines the
// logic to avoid a redundant factory.sexpr() allocation. Kept for reference.
#[allow(dead_code)]
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

// ============================================================================
// Phase 2.A — Binding-Preserving Cartesian Product
// ============================================================================
//
// The existing `GenericCartesianProductIter` + `process_collected_sexpr_generic`
// path assumes each child of an S-expression returns ONE alternative, so the
// code merges the first alternative's bindings from each child and applies
// that single merged binding set to every tuple in the Cartesian product.
//
// When children return MULTIPLE alternatives (nondet), that logic produces
// "ghost" combinations: tuples whose actual contributing alternatives would
// have conflicting bindings (so the combination should be DROPPED per HE
// `BindingsSet::empty()` semantics), but the code instead tags them with
// the first-result merge and forwards them as ghosts.
//
// The bound iterator below carries `BoundValue` per alternative, composes
// bindings per-combination using `compose_outer_inner_generic`, and SKIPS
// combinations whose composition yields empty bindings (conflict) — exactly
// matching HE's silent-pruning semantics.
//
// This path is wired only for the nondet-children case (`total_combinations
// > 1`). Single-alternative children continue to use the existing fast path.

/// Cartesian product iterator that preserves per-result bindings.
///
/// Each step composes the bindings of the chosen alternatives (starting
/// from `outer_carrying`) using `compose_outer_inner_generic`. A combination
/// whose composed bindings are empty — despite at least one contributor
/// being non-empty — is silently skipped (HE-bisimilar conflict pruning).
#[derive(Debug, Clone)]
pub struct GenericCartesianProductBoundIter {
    /// Input vectors of alternatives, one Vec per child expression.
    inputs: Vec<Vec<BoundValue>>,
    /// Current mixed-radix indices into each input vector.
    indices: Vec<usize>,
    /// Whether all combinations have been produced.
    exhausted: bool,
    /// Ambient bindings at the point of S-expr construction. Composed into
    /// every combination's merged bindings (left-most operand of the fold).
    outer_carrying: GenericBindings<MettaValue>,
    /// Factory needed for `compose_outer_inner_generic`. `GcFactory` is Copy.
    factory: GcFactory,
}

impl GenericCartesianProductBoundIter {
    /// Create a new bound Cartesian product iterator.
    pub fn new(
        inputs: Vec<Vec<BoundValue>>,
        outer_carrying: GenericBindings<MettaValue>,
        factory: GcFactory,
    ) -> Self {
        let exhausted = inputs.iter().any(|v| v.is_empty());
        let indices = vec![0; inputs.len()];
        Self {
            inputs,
            indices,
            exhausted,
            outer_carrying,
            factory,
        }
    }

    /// Get a reference to the input vectors (for GC root collection).
    #[inline]
    pub fn inputs(&self) -> &[Vec<BoundValue>] {
        &self.inputs
    }

    /// Get a reference to the outer carrying bindings (for GC root collection).
    #[inline]
    pub fn outer_carrying(&self) -> &GenericBindings<MettaValue> {
        &self.outer_carrying
    }

    /// Advance the mixed-radix index. Sets `exhausted` when wrapping.
    fn advance_indices(&mut self) {
        let mut i = self.indices.len();
        while i > 0 {
            i -= 1;
            self.indices[i] += 1;
            if self.indices[i] < self.inputs[i].len() {
                return;
            }
            self.indices[i] = 0;
            if i == 0 {
                self.exhausted = true;
                return;
            }
        }
        self.exhausted = true;
    }
}

impl Iterator for GenericCartesianProductBoundIter {
    /// Yields `(combo_values, composed_bindings)` for each non-conflicting
    /// combination. Conflicting combinations are silently skipped.
    type Item = (SmallVec<[MettaValue; 8]>, GenericBindings<MettaValue>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.exhausted {
                return None;
            }

            // Build current combo's values.
            let combo_values: SmallVec<[MettaValue; 8]> = self
                .inputs
                .iter()
                .zip(self.indices.iter())
                .map(|(vec, &idx)| vec[idx].0)
                .collect();

            // Compose bindings: fold each alternative's bindings into
            // `outer_carrying`. If any composition step yields empty from two
            // non-empty inputs, that's a conflict — skip this combination.
            let mut merged = self.outer_carrying.clone();
            let mut conflict = false;
            for (vec, &idx) in self.inputs.iter().zip(self.indices.iter()) {
                let item_bindings = &vec[idx].1;
                if item_bindings.is_empty() {
                    continue;
                }
                let was_merged_empty = merged.is_empty();
                let merged_next =
                    compose_outer_inner_generic(&merged, item_bindings, &self.factory);
                // compose returns empty only on conflict (empty-inputs are
                // short-circuited inside compose). So merged_next.is_empty()
                // combined with at-least-one-non-empty input => conflict.
                if merged_next.is_empty() && !was_merged_empty {
                    conflict = true;
                    break;
                }
                merged = merged_next;
            }

            self.advance_indices();

            if !conflict {
                return Some((combo_values, merged));
            }
            // else: loop and try the next combination
        }
    }
}

/// Binding-preserving analogue of `GenericProcessedSExpr`.
///
/// Each variant carries per-result or per-combination bindings so that
/// downstream continuations can dispatch with accurate `outer_carrying`
/// contexts rather than a shared "first-result merge".
pub enum GenericProcessedSExprBound<
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone = crate::backend::models::GcFactory,
> {
    /// Evaluation complete — return bound results.
    Done((SmallVec<[BoundValue; 2]>, GenericEnvironment<V, F>)),

    /// Rule matches found — need to evaluate RHS. `base_results` carry their
    /// per-result bindings (composed from the single Cartesian combination
    /// that produced them).
    EvalRuleMatches {
        matches: Vec<(V, GenericBindings<V>)>,
        env: GenericEnvironment<V, F>,
        depth: usize,
        base_results: SmallVec<[BoundValue; 2]>,
    },

    /// Multiple combinations — need lazy processing. Iterator yields
    /// `(combo_values, composed_bindings)` per surviving combination;
    /// conflicts are already pruned inside the iterator.
    EvalCombinations {
        combinations: GenericCartesianProductBoundIter,
        env: GenericEnvironment<V, F>,
        depth: usize,
    },

    /// Special form redispatch (single combination path). Carries the
    /// combo's merged bindings so the redispatched `EvalWithBindings` can
    /// thread them through.
    RedispatchSExpr {
        items: Vec<V>,
        combo_bindings: GenericBindings<V>,
        env: GenericEnvironment<V, F>,
        depth: usize,
    },
}

/// Compose bindings of a single combination with `outer_carrying`.
/// Returns `None` on conflict, `Some(merged)` otherwise.
#[inline]
fn compose_combo_bindings(
    combo_bindings: &[&GenericBindings<MettaValue>],
    outer_carrying: &GenericBindings<MettaValue>,
    factory: &GcFactory,
) -> Option<GenericBindings<MettaValue>> {
    let mut merged = outer_carrying.clone();
    for item in combo_bindings.iter() {
        if item.is_empty() {
            continue;
        }
        let was_merged_empty = merged.is_empty();
        let merged_next = compose_outer_inner_generic(&merged, item, factory);
        if merged_next.is_empty() && !was_merged_empty {
            return None;
        }
        merged = merged_next;
    }
    Some(merged)
}

/// Binding-preserving analogue of `process_collected_sexpr_generic`.
///
/// Used by `Continuation::CollectSExpr` when children return multiple
/// alternatives (the fast path is still `process_collected_sexpr_generic`).
/// Composes bindings per-combination using `compose_outer_inner_generic`,
/// dropping combinations whose composition conflicts.
pub fn process_collected_sexpr_bound_generic(
    collected: Vec<(SmallVec<[BoundValue; 2]>, MettaEnvironment)>,
    outer_carrying: GenericBindings<MettaValue>,
    original_env: MettaEnvironment,
    depth: usize,
    factory: &GcFactory,
) -> GenericProcessedSExprBound<MettaValue, GcFactory> {
    // Check for errors in sub-expression results — propagate the first error
    // found, preserving its bindings if present.
    for (results, new_env) in &collected {
        if let Some((first_v, first_b)) = results.first() {
            if first_v.is_error() {
                return GenericProcessedSExprBound::Done((
                    smallvec![bv_with(first_v.clone(), first_b.clone())],
                    new_env.clone(),
                ));
            }
        }
    }

    // Split: eval_results_bound: Vec<Vec<BoundValue>>, envs: Vec<MettaEnvironment>
    let (eval_results_bound, envs): (Vec<Vec<BoundValue>>, Vec<_>) = collected
        .into_iter()
        .map(|(sv, env)| (sv.into_vec(), env))
        .unzip();

    let unified_env = original_env.union_all(&envs);

    // Empty any child → no combinations → empty output (HE-bisimilar).
    if eval_results_bound.iter().any(|v| v.is_empty()) {
        return GenericProcessedSExprBound::Done((SmallVec::new(), unified_env));
    }

    // Fast-path: single combination (each child has exactly one alternative).
    let total_combinations: usize = eval_results_bound.iter().map(|v| v.len()).product();
    if total_combinations == 1 {
        // Collect the single combo's values and compose its bindings.
        let combo_values: Vec<MettaValue> = eval_results_bound.iter().map(|v| v[0].0).collect();
        let combo_b_refs: Vec<&GenericBindings<MettaValue>> =
            eval_results_bound.iter().map(|v| &v[0].1).collect();
        match compose_combo_bindings(&combo_b_refs, &outer_carrying, factory) {
            Some(combo_bindings) => {
                return process_single_combination_bound_generic(
                    combo_values,
                    combo_bindings,
                    unified_env,
                    depth,
                    factory,
                );
            }
            None => {
                // Conflict: single combo dropped → zero output.
                return GenericProcessedSExprBound::Done((SmallVec::new(), unified_env));
            }
        }
    }

    // Slow-path: lazy iterator with per-combo composition.
    let iter = GenericCartesianProductBoundIter::new(eval_results_bound, outer_carrying, *factory);
    GenericProcessedSExprBound::EvalCombinations {
        combinations: iter,
        env: unified_env,
        depth,
    }
}

/// Binding-preserving analogue of `process_single_combination_generic`.
///
/// Given a combination's values and its already-composed bindings, checks
/// for grounded operations / special forms / rule matches (matching the
/// non-bound version's dispatch logic), attaching the combo bindings to
/// the produced `BoundValue` results.
pub fn process_single_combination_bound_generic(
    evaled_items: Vec<MettaValue>,
    combo_bindings: GenericBindings<MettaValue>,
    mut unified_env: MettaEnvironment,
    depth: usize,
    factory: &GcFactory,
) -> GenericProcessedSExprBound<MettaValue, GcFactory> {
    if let Some(first) = evaled_items.first() {
        if let Some(op) = first.as_atom() {
            // Grounded operation: execute and tag result(s) with combo bindings.
            if has_grounded_op(op) {
                let args: Vec<MettaValue> = evaled_items[1..].to_vec();
                let mut state = GroundedState::new(op.to_string(), args);

                if let Some(work) = execute_grounded_op(op, &mut state, factory) {
                    match work {
                        GroundedWork::Done(results) => {
                            let values: SmallVec<[BoundValue; 2]> = results
                                .into_iter()
                                .map(|(v, _)| bv_with(v, combo_bindings.clone()))
                                .collect();
                            return GenericProcessedSExprBound::Done((values, unified_env));
                        }
                        GroundedWork::EvalArg { .. } => {
                            // Grounded op needs arg evaluation (unusual at this
                            // point since args are already evaluated). Fall
                            // through to the rule-match / data path below.
                        }
                        GroundedWork::Error(e) => {
                            let err = factory.error(
                                factory.string(&format!("{:?}", e)),
                                factory.atom("GroundedError"),
                            );
                            return GenericProcessedSExprBound::Done((
                                smallvec![bv_with(err, combo_bindings)],
                                unified_env,
                            ));
                        }
                    }
                }
            }

            // Special form redispatch — pass combo bindings through.
            if needs_special_form_redispatch(op) {
                return GenericProcessedSExprBound::RedispatchSExpr {
                    items: evaled_items,
                    combo_bindings,
                    env: unified_env,
                    depth,
                };
            }
        }
    }

    // Try rule matching using the same generic helper as the non-bound path.
    let sexpr = factory.sexpr(evaled_items);
    let all_matches_with_types = try_match_all_rules(&sexpr, &unified_env, *factory);

    if !all_matches_with_types.is_empty() {
        // Rules matched — downstream `dispatch_rule_matches` will thread
        // `combo_bindings` into each RHS evaluation as the `outer_carrying`.
        return GenericProcessedSExprBound::EvalRuleMatches {
            matches: all_matches_with_types
                .into_iter()
                .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                .collect(),
            env: unified_env,
            depth,
            // base_results = empty (no fallback data to emit alongside rule
            // matches); if all matches produce no result downstream, the
            // caller is responsible for the fallback emit.
            base_results: SmallVec::new(),
        };
    }

    // No rules matched.
    //
    // Phase 2.B HE-bisimilarity fix (mirrors process_single_combination_generic):
    // distinguish "function with no matching rules" (→ empty) from "data
    // constructor" (→ data). This is the bound-path fast-path that was missing
    // the check, producing ghost results for free-variable queries like
    // `(grandfather $who $x)` where a combo like `(grandfather b c)` has no
    // matching rule.
    if depth > 0 {
        if let Some(head) = sexpr.get_head_symbol() {
            let arity = sexpr.get_arity();
            let has_any_rules = unified_env
                .shared
                .rule_index
                .read()
                .get_candidates(head, arity, None)
                .next()
                .is_some();
            if has_any_rules {
                // Function with no matching rules → empty (HE semantics).
                return GenericProcessedSExprBound::Done((SmallVec::new(), unified_env));
            }
        }
    }

    // X.6 MTT-TI-029: top-level auto-add removed (see comment above the
    // earlier process_single_combination_generic exit path).

    // S1 TOPLEVEL (2026-05-13): mirror unbound path's add-then-gate logic.
    // Bare top-level S-exprs are always added to space (regardless of
    // interpret_mode); the output gate then suppresses emission only
    // when !interpret_mode.
    if depth == 0 {
        if !unified_env.match_space_exists(&sexpr) {
            unified_env.add_to_space(&sexpr);
        }
        if !unified_env.in_interpret_mode() {
            return GenericProcessedSExprBound::Done((SmallVec::new(), unified_env));
        }
    }
    GenericProcessedSExprBound::Done((smallvec![bv_with(sexpr, combo_bindings)], unified_env))
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
        let inputs: Vec<Vec<MettaValue>> =
            vec![vec![MettaValue::Long(1)], vec![MettaValue::Long(2)]];
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
