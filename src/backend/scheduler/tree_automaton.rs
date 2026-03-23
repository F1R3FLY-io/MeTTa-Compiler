//! Weighted Tree Automaton (WTA) for expression classification.
//!
//! Extracted from `mettail-rust/prattail/src/tree_automaton.rs` (~200 LOC core).
//! Provides a bottom-up tree automaton that classifies MeTTa expressions into
//! cost classes based on their structure (head symbol, arity, child classifications).
//!
//! ## Architecture
//!
//! ```text
//! MeTTa expression tree
//!       │
//!       ▼
//! bottom_up_classify()
//!       │ assigns CostClass + weight to each subexpression
//!       ▼
//! (CostClass, SchedulerWeight) at root
//! ```
//!
//! The tree automaton evaluates expressions bottom-up: leaves are classified
//! first (atoms → GroundCheap, integers → GroundCheap), then internal nodes
//! are classified based on their head symbol and the cost classes of their
//! children.
//!
//! ## References
//!
//! - Comon et al. (2007), "Tree Automata Techniques and Applications" (TATA)
//! - Borchardt (2004), "The Myhill-Nerode theorem for recognizable tree series"

use std::collections::HashMap;
use std::fmt;

use super::semiring::Semiring;

// ══════════════════════════════════════════════════════════════════════════════
// Core types
// ══════════════════════════════════════════════════════════════════════════════

/// A state in the tree automaton.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TreeState {
    /// Unique state identifier.
    pub id: usize,
    /// Optional label for diagnostics (e.g., "GroundCheap").
    pub label: Option<String>,
    /// Whether this is a final/accepting state.
    pub is_final: bool,
}

impl TreeState {
    /// Create a new non-final state.
    pub fn new(id: usize) -> Self {
        TreeState {
            id,
            label: None,
            is_final: false,
        }
    }

    /// Create a final (accepting) state.
    pub fn final_state(id: usize) -> Self {
        TreeState {
            id,
            label: None,
            is_final: true,
        }
    }

    /// Create a labeled state.
    pub fn labeled(id: usize, label: impl Into<String>, is_final: bool) -> Self {
        TreeState {
            id,
            label: Some(label.into()),
            is_final,
        }
    }
}

impl fmt::Display for TreeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fin = if self.is_final { "!" } else { "" };
        if let Some(ref label) = self.label {
            write!(f, "s{}{}({})", self.id, fin, label)
        } else {
            write!(f, "s{}{}", self.id, fin)
        }
    }
}

/// A transition in the weighted tree automaton.
///
/// Bottom-up transition: `f(q₁, ..., qₙ) → q [w]`
/// The function symbol `f` with arity `n` applied to child states `q₁, ..., qₙ`
/// transitions to state `q` with weight `w`.
#[derive(Debug, Clone)]
pub struct TreeTransition<W: Semiring> {
    /// Function symbol (head symbol name, e.g., "+", "if", "my-fn").
    pub symbol: String,
    /// Child state IDs (empty for leaf/constant transitions).
    pub child_states: Vec<usize>,
    /// Target state ID.
    pub target_state: usize,
    /// Transition weight.
    pub weight: W,
}

impl<W: Semiring> TreeTransition<W> {
    /// Create a leaf (nullary) transition: `c → q [w]`.
    pub fn leaf(symbol: impl Into<String>, target: usize, weight: W) -> Self {
        TreeTransition {
            symbol: symbol.into(),
            child_states: Vec::new(),
            target_state: target,
            weight,
        }
    }

    /// Create a unary transition: `f(q₁) → q [w]`.
    pub fn unary(symbol: impl Into<String>, child: usize, target: usize, weight: W) -> Self {
        TreeTransition {
            symbol: symbol.into(),
            child_states: vec![child],
            target_state: target,
            weight,
        }
    }

    /// Create a binary transition: `f(q₁, q₂) → q [w]`.
    pub fn binary(
        symbol: impl Into<String>,
        left: usize,
        right: usize,
        target: usize,
        weight: W,
    ) -> Self {
        TreeTransition {
            symbol: symbol.into(),
            child_states: vec![left, right],
            target_state: target,
            weight,
        }
    }

    /// Create an n-ary transition: `f(q₁, ..., qₙ) → q [w]`.
    pub fn nary(
        symbol: impl Into<String>,
        children: Vec<usize>,
        target: usize,
        weight: W,
    ) -> Self {
        TreeTransition {
            symbol: symbol.into(),
            child_states: children,
            target_state: target,
            weight,
        }
    }

    /// Arity of this transition (number of children).
    pub fn arity(&self) -> usize {
        self.child_states.len()
    }
}

impl<W: Semiring> fmt::Display for TreeTransition<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.child_states.is_empty() {
            write!(
                f,
                "{} → s{} [{:?}]",
                self.symbol, self.target_state, self.weight,
            )
        } else {
            let children: Vec<String> = self
                .child_states
                .iter()
                .map(|s| format!("s{}", s))
                .collect();
            write!(
                f,
                "{}({}) → s{} [{:?}]",
                self.symbol,
                children.join(", "),
                self.target_state,
                self.weight,
            )
        }
    }
}

/// A Weighted Tree Automaton (WTA).
///
/// `A = (Q, Σ, Δ, F, w)` where:
/// - `Q` is the finite set of states
/// - `Σ` is the ranked alphabet (function symbols with arities)
/// - `Δ` is the set of transitions `f(q₁, ..., qₙ) → q`
/// - `F ⊆ Q` are final (accepting) states
/// - `w: Δ → W` assigns weights from semiring `W` to transitions
#[derive(Debug, Clone)]
pub struct TreeAutomaton<W: Semiring> {
    /// All states.
    pub states: Vec<TreeState>,
    /// All transitions.
    pub transitions: Vec<TreeTransition<W>>,
    /// Final (accepting) state IDs.
    pub final_states: Vec<usize>,
    /// Ranked alphabet: maps symbol name to expected arity.
    pub ranked_alphabet: HashMap<String, usize>,
    /// Transition index: `(symbol, arity)` → list of transition indices.
    /// Built by `build_index()` for O(1) lookup during evaluation.
    transition_index: HashMap<(String, usize), Vec<usize>>,
}

impl<W: Semiring> TreeAutomaton<W> {
    /// Create an empty tree automaton.
    pub fn new() -> Self {
        TreeAutomaton {
            states: Vec::new(),
            transitions: Vec::new(),
            final_states: Vec::new(),
            ranked_alphabet: HashMap::new(),
            transition_index: HashMap::new(),
        }
    }

    /// Add a state and return its ID.
    pub fn add_state(&mut self, is_final: bool) -> usize {
        let id = self.states.len();
        let state = if is_final {
            TreeState::final_state(id)
        } else {
            TreeState::new(id)
        };
        if is_final {
            self.final_states.push(id);
        }
        self.states.push(state);
        id
    }

    /// Add a labeled state and return its ID.
    pub fn add_labeled_state(
        &mut self,
        label: impl Into<String>,
        is_final: bool,
    ) -> usize {
        let id = self.states.len();
        let state = TreeState::labeled(id, label, is_final);
        if is_final {
            self.final_states.push(id);
        }
        self.states.push(state);
        id
    }

    /// Register a ranked symbol (function symbol with arity).
    pub fn add_symbol(&mut self, name: impl Into<String>, arity: usize) {
        self.ranked_alphabet.insert(name.into(), arity);
    }

    /// Add a transition.
    pub fn add_transition(&mut self, transition: TreeTransition<W>) {
        self.ranked_alphabet
            .entry(transition.symbol.clone())
            .or_insert(transition.arity());
        self.transitions.push(transition);
    }

    /// Build the transition index for O(1) lookup during evaluation.
    /// Must be called after all transitions are added.
    pub fn build_index(&mut self) {
        self.transition_index.clear();
        for (idx, trans) in self.transitions.iter().enumerate() {
            self.transition_index
                .entry((trans.symbol.clone(), trans.arity()))
                .or_default()
                .push(idx);
        }
    }

    /// Number of states.
    pub fn num_states(&self) -> usize {
        self.states.len()
    }

    /// Number of transitions.
    pub fn num_transitions(&self) -> usize {
        self.transitions.len()
    }
}

impl<W: Semiring> Default for TreeAutomaton<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Semiring> fmt::Display for TreeAutomaton<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TreeAutomaton {{ states: {}, transitions: {}, final: {}, symbols: {} }}",
            self.num_states(),
            self.num_transitions(),
            self.final_states.len(),
            self.ranked_alphabet.len(),
        )
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Bottom-up evaluation
// ══════════════════════════════════════════════════════════════════════════════

/// Result of bottom-up evaluation at a single node: state → weight mapping.
pub type StateWeightMap<W> = HashMap<usize, W>;

/// Evaluate a term bottom-up through a weighted tree automaton.
///
/// For each subterm, computes the set of reachable states and their accumulated
/// weights. At node `f(t₁, ..., tₙ)`, all matching transitions
/// `f(q₁, ..., qₙ) → q [w]` are applied, and weights are accumulated
/// (⊗ along paths, ⊕ across alternatives).
///
/// Uses the transition index if available (call `build_index()` first for
/// O(1) transition lookup per symbol+arity).
pub fn bottom_up_evaluate<W: Semiring>(
    automaton: &TreeAutomaton<W>,
    symbol: &str,
    child_state_maps: &[StateWeightMap<W>],
) -> StateWeightMap<W> {
    let arity = child_state_maps.len();
    let mut result: StateWeightMap<W> = HashMap::new();

    // Use index if available, otherwise linear scan
    let transitions: Vec<usize> = if !automaton.transition_index.is_empty() {
        automaton
            .transition_index
            .get(&(symbol.to_string(), arity))
            .cloned()
            .unwrap_or_default()
    } else {
        automaton
            .transitions
            .iter()
            .enumerate()
            .filter(|(_, t)| t.symbol == symbol && t.arity() == arity)
            .map(|(i, _)| i)
            .collect()
    };

    for &trans_idx in &transitions {
        let trans = &automaton.transitions[trans_idx];

        // For each required child state qᵢ, check that child i reached qᵢ
        // and accumulate the product of transition weight with all child weights.
        let mut combined_weight = trans.weight;
        let mut all_children_match = true;

        for (i, &required_state) in trans.child_states.iter().enumerate() {
            match child_state_maps[i].get(&required_state) {
                Some(child_weight) => {
                    combined_weight = combined_weight.times(child_weight);
                }
                None => {
                    all_children_match = false;
                    break;
                }
            }
        }

        if all_children_match {
            // Accumulate via semiring plus (combine alternative derivations).
            result
                .entry(trans.target_state)
                .and_modify(|existing| *existing = existing.plus(&combined_weight))
                .or_insert(combined_weight);
        }
    }

    result
}

/// Find the best (lowest-cost for tropical) final state from a state-weight map.
///
/// Returns `(state_id, weight)` for the best final state, or `None` if no
/// final state is reachable.
pub fn best_final_state<W: Semiring + Ord>(
    automaton: &TreeAutomaton<W>,
    state_map: &StateWeightMap<W>,
) -> Option<(usize, W)> {
    automaton
        .final_states
        .iter()
        .filter_map(|&state_id| {
            state_map.get(&state_id).map(|w| (state_id, *w))
        })
        .min_by(|(_, w1), (_, w2)| w1.cmp(w2))
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::semiring::TropicalWeight;

    fn build_arithmetic_automaton() -> TreeAutomaton<TropicalWeight> {
        let mut wta = TreeAutomaton::new();

        // States: 0=ground_cheap, 1=ground_arith
        let q_cheap = wta.add_labeled_state("GroundCheap", true);
        let q_arith = wta.add_labeled_state("GroundArith", true);

        // Leaf transitions: literals → GroundCheap
        wta.add_transition(TreeTransition::leaf("Atom", q_cheap, TropicalWeight::new(1.0)));
        wta.add_transition(TreeTransition::leaf("Long", q_cheap, TropicalWeight::new(1.0)));
        wta.add_transition(TreeTransition::leaf("Bool", q_cheap, TropicalWeight::new(1.0)));

        // Arithmetic: +(GroundCheap, GroundCheap) → GroundArith
        wta.add_transition(TreeTransition::binary("+", q_cheap, q_cheap, q_arith, TropicalWeight::new(2.0)));
        wta.add_transition(TreeTransition::binary("-", q_cheap, q_cheap, q_arith, TropicalWeight::new(2.0)));
        wta.add_transition(TreeTransition::binary("*", q_cheap, q_cheap, q_arith, TropicalWeight::new(2.0)));

        // Nested arithmetic: +(GroundArith, GroundCheap) → GroundArith
        wta.add_transition(TreeTransition::binary("+", q_arith, q_cheap, q_arith, TropicalWeight::new(2.0)));
        wta.add_transition(TreeTransition::binary("+", q_cheap, q_arith, q_arith, TropicalWeight::new(2.0)));
        wta.add_transition(TreeTransition::binary("+", q_arith, q_arith, q_arith, TropicalWeight::new(2.0)));

        wta.build_index();
        wta
    }

    #[test]
    fn test_leaf_classification() {
        let wta = build_arithmetic_automaton();
        let result = bottom_up_evaluate(&wta, "Long", &[]);
        assert!(result.contains_key(&0)); // state 0 = GroundCheap
        assert_eq!(result[&0], TropicalWeight::new(1.0));
    }

    #[test]
    fn test_arithmetic_classification() {
        let wta = build_arithmetic_automaton();

        // Classify children first
        let left = bottom_up_evaluate(&wta, "Long", &[]);
        let right = bottom_up_evaluate(&wta, "Long", &[]);

        // Classify +(Long, Long)
        let result = bottom_up_evaluate(&wta, "+", &[left, right]);
        assert!(result.contains_key(&1)); // state 1 = GroundArith
        // weight = 2.0 (transition) + 1.0 (left) + 1.0 (right) = 4.0
        assert_eq!(result[&1], TropicalWeight::new(4.0));
    }

    #[test]
    fn test_nested_arithmetic() {
        let wta = build_arithmetic_automaton();

        let a = bottom_up_evaluate(&wta, "Long", &[]);
        let b = bottom_up_evaluate(&wta, "Long", &[]);
        let inner = bottom_up_evaluate(&wta, "+", &[a.clone(), b]);

        let c = bottom_up_evaluate(&wta, "Long", &[]);
        let outer = bottom_up_evaluate(&wta, "+", &[inner, c]);

        // Should classify as GroundArith
        assert!(outer.contains_key(&1));
    }

    #[test]
    fn test_best_final_state() {
        let wta = build_arithmetic_automaton();
        let result = bottom_up_evaluate(&wta, "Long", &[]);
        let best = best_final_state(&wta, &result);
        assert!(best.is_some());
        let (state, weight) = best.expect("should find a final state");
        assert_eq!(state, 0); // GroundCheap
        assert_eq!(weight, TropicalWeight::new(1.0));
    }

    #[test]
    fn test_no_matching_transition() {
        let wta = build_arithmetic_automaton();
        let result = bottom_up_evaluate(&wta, "unknown_symbol", &[]);
        assert!(result.is_empty());
    }
}
