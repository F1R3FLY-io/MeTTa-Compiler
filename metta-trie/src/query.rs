//! Pattern Query for MettaTrie
//!
//! Implements `MettaTrie::query()` which performs pattern matching by
//! traversing both concrete and `Variable` edges at each trie level.
//!
//! ## Algorithm
//!
//! Given a pattern (key sequence with `Variable` positions), the query
//! performs a depth-first traversal of the trie. At each node:
//!
//! 1. If the current pattern key is concrete (Atom, Long, etc.):
//!    - Follow the concrete edge (exact match)
//!    - Also follow the `Variable` edge (wildcard match), recording a binding
//!
//! 2. If the current pattern key is `Variable`:
//!    - Follow ALL child edges (the variable matches everything)
//!    - Record what each edge matched as a binding
//!
//! This is the same traversal logic used by `DiscriminationTree::query()`,
//! extended to return full entries and variable bindings.

use smallvec::SmallVec;

use crate::keys::TrieKey;
use crate::node::{MettaTrie, MettaTrieNode};

/// A single match result from a pattern query.
#[derive(Debug, Clone)]
pub struct QueryMatch<E, V> {
    /// The original expression stored at the matched trie path.
    pub expr: E,
    /// The mapped value (e.g., Multiplicity).
    pub value: V,
    /// Variable bindings: `(variable_index, matched_key)`.
    /// The index corresponds to the position of the `Variable` in the pattern's
    /// key sequence (0-based, counting only Variable keys).
    pub bindings: SmallVec<[(u16, TrieKey); 4]>,
}

impl<E: Clone, V: Clone> MettaTrie<E, V> {
    /// Query the trie with a pattern key sequence.
    ///
    /// `Variable` keys in the pattern match any concrete key at that position.
    /// Returns all matching entries with their variable bindings.
    ///
    /// ## Complexity
    ///
    /// O(k × b) where k = pattern depth and b = branching factor at Variable positions.
    /// For patterns with no variables, this is O(k) — equivalent to `get_at()`.
    pub fn query(&self, pattern_keys: &[TrieKey]) -> Vec<QueryMatch<E, V>> {
        let mut results = Vec::new();
        let mut bindings: SmallVec<[(u16, TrieKey); 4]> = SmallVec::new();
        let mut var_index: u16 = 0;

        Self::query_recursive(
            self.root(),
            pattern_keys,
            &mut bindings,
            &mut var_index,
            &mut results,
        );

        results
    }

    fn query_recursive(
        node: &MettaTrieNode<E, V>,
        remaining_keys: &[TrieKey],
        bindings: &mut SmallVec<[(u16, TrieKey); 4]>,
        var_index: &mut u16,
        results: &mut Vec<QueryMatch<E, V>>,
    ) {
        // Base case: no more keys to match
        if remaining_keys.is_empty() {
            if let Some((expr, value)) = &node.entry {
                results.push(QueryMatch {
                    expr: expr.clone(),
                    value: value.clone(),
                    bindings: bindings.clone(),
                });
            }
            return;
        }

        let (first_key, rest) = remaining_keys.split_first().expect("non-empty");
        let saved_var_index = *var_index;

        match first_key {
            TrieKey::Variable => {
                // Variable in pattern: match ALL children at this position.
                // A Variable matches a single atomic key OR an entire S-expression
                // subtree (Arity + all nested children in the trie).
                //
                // When Variable matches an Arity(n) key in the trie, we need to
                // recursively collect all entries from the subtree rooted at that
                // Arity node, then continue matching `rest` at each leaf of that
                // subtree. This is equivalent to "Variable matches the entire
                // S-expression at this position."
                let this_var_idx = *var_index;
                *var_index += 1;

                for (child_key, child_node) in &node.children {
                    if matches!(child_key, TrieKey::Arity(_)) {
                        // Variable matches an entire S-expression subtree.
                        // We need to find all leaves of this subtree (the "end"
                        // of the S-expression) and continue matching `rest` from there.
                        let arity = match child_key {
                            TrieKey::Arity(n) => *n as usize,
                            _ => unreachable!(),
                        };
                        bindings.push((this_var_idx, child_key.clone()));
                        // Descend through the subtree's children, skipping the
                        // entire S-expression structure, then continue with `rest`
                        Self::skip_subtree_and_continue(
                            child_node,
                            arity,
                            rest,
                            bindings,
                            var_index,
                            results,
                        );
                        bindings.pop();
                    } else {
                        // Simple key: Variable matches this single key
                        bindings.push((this_var_idx, child_key.clone()));
                        Self::query_recursive(child_node, rest, bindings, var_index, results);
                        bindings.pop();
                    }
                }

                *var_index = saved_var_index + 1;
            }
            concrete_key => {
                // Concrete key in pattern: follow exact match
                if let Some(child) = node.children.get(concrete_key) {
                    // For Arity keys, the children's keys follow in the sequence
                    Self::query_recursive(child, rest, bindings, var_index, results);
                }

                // Also follow Variable edges in the trie (stored patterns with variables)
                if let Some(var_child) = node.children.get(&TrieKey::Variable) {
                    let saved_bindings_len = bindings.len();
                    // The trie has a Variable at this position — it matches our concrete key
                    // Record what the trie's variable matched
                    Self::query_recursive(var_child, rest, bindings, var_index, results);
                    bindings.truncate(saved_bindings_len);
                }
            }
        }
    }
}

impl<E: Clone, V: Clone> MettaTrie<E, V> {
    /// Skip an S-expression subtree in the trie (descend through `remaining_children`
    /// levels of children) then continue matching `rest_pattern` at the leaves.
    ///
    /// This handles the case where a pattern `Variable` matches an entire S-expression
    /// in the trie. The S-expression's structure is `Arity(n) → n children`, where
    /// each child is either an atomic key or another `Arity(m) → m children`.
    ///
    /// We recursively descend through the trie's structure until we've consumed
    /// all `remaining_children` children, then call `query_recursive` with the
    /// remaining pattern keys.
    fn skip_subtree_and_continue(
        node: &MettaTrieNode<E, V>,
        remaining_children: usize,
        rest_pattern: &[TrieKey],
        bindings: &mut SmallVec<[(u16, TrieKey); 4]>,
        var_index: &mut u16,
        results: &mut Vec<QueryMatch<E, V>>,
    ) {
        if remaining_children == 0 {
            // We've consumed the entire S-expression subtree.
            // Continue matching the rest of the pattern at this node.
            Self::query_recursive(node, rest_pattern, bindings, var_index, results);
            return;
        }

        // We still need to consume `remaining_children` children of the S-expression.
        // Each child in the trie is either:
        // - An atomic key (Atom, Long, Bool, etc.) → consumes 1 child
        // - An Arity(m) key → consumes 1 child but adds m grandchildren to descend
        for (_child_key, child_node) in &node.children {
            match _child_key {
                TrieKey::Arity(m) => {
                    // This child is itself an S-expression with m children.
                    // We consume 1 child from remaining_children, but need to
                    // descend through m grandchildren first.
                    Self::skip_subtree_and_continue(
                        child_node,
                        remaining_children - 1 + *m as usize,
                        rest_pattern,
                        bindings,
                        var_index,
                        results,
                    );
                }
                _ => {
                    // Atomic key — consumes 1 child
                    Self::skip_subtree_and_continue(
                        child_node,
                        remaining_children - 1,
                        rest_pattern,
                        bindings,
                        var_index,
                        results,
                    );
                }
            }
        }
    }
}

/// Count how many keys in `keys` are consumed by `n` children.
/// Each child is either a single key (atom, long, etc.) or an Arity(m) + m children.
fn count_keys_for_children(keys: &[TrieKey], n: usize) -> usize {
    let mut consumed = 0;
    let mut remaining = n;

    while remaining > 0 && consumed < keys.len() {
        match &keys[consumed] {
            TrieKey::Arity(child_arity) => {
                consumed += 1; // The Arity key itself
                // Recursively count keys for this S-expression's children
                consumed += count_keys_for_children(&keys[consumed..], *child_arity as usize);
            }
            _ => {
                consumed += 1; // Single key (atom, long, bool, etc.)
            }
        }
        remaining -= 1;
    }

    consumed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_query() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Long(42)];
        trie.insert_at(&keys, "(f 42)".to_string(), 1);

        let results = trie.query(&keys);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].expr, "(f 42)");
        assert_eq!(results[0].value, 1);
        assert!(results[0].bindings.is_empty());
    }

    #[test]
    fn test_query_no_match() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("f")];
        trie.insert_at(&keys, "f".to_string(), 1);

        let results = trie.query(&[TrieKey::Atom("g")]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_query_with_variable_in_pattern() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        // Store (f 1) and (f 2)
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Long(1)],
            "(f 1)".to_string(),
            1,
        );
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Long(2)],
            "(f 2)".to_string(),
            2,
        );

        // Query: (f $x) — Variable matches both 1 and 2
        let pattern = vec![TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Variable];
        let results = trie.query(&pattern);
        assert_eq!(results.len(), 2);

        // Both should have a binding for variable 0
        for result in &results {
            assert_eq!(result.bindings.len(), 1);
            assert_eq!(result.bindings[0].0, 0); // var index 0
        }
    }

    #[test]
    fn test_query_variable_edge_in_trie() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        // Store a pattern with Variable in the trie: (f $x) where $x is Variable
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Variable],
            "(f $x)".to_string(),
            1,
        );

        // Query: (f 42) — concrete key 42 should match the Variable edge
        let pattern = vec![TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Long(42)];
        let results = trie.query(&pattern);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].expr, "(f $x)");
    }

    #[test]
    fn test_query_multiple_variables() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(
            &[
                TrieKey::Arity(3),
                TrieKey::Atom("f"),
                TrieKey::Long(1),
                TrieKey::Long(2),
            ],
            "(f 1 2)".to_string(),
            1,
        );

        // Query: (f $x $y)
        let pattern = vec![
            TrieKey::Arity(3),
            TrieKey::Atom("f"),
            TrieKey::Variable,
            TrieKey::Variable,
        ];
        let results = trie.query(&pattern);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].bindings.len(), 2);
        assert_eq!(results[0].bindings[0], (0, TrieKey::Long(1)));
        assert_eq!(results[0].bindings[1], (1, TrieKey::Long(2)));
    }

    #[test]
    fn test_variable_matches_nested_sexpr_subtree() {
        // Scenario: stored (= (f $0) (g $0)), query (= $lhs $rhs)
        // The two Variables in the query must each skip an entire nested S-expression
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        // Store: (= (f $0) (g $0)) → keys: [Arity(3), =, Arity(2), f, $0, Arity(2), g, $0]
        trie.insert_at(
            &[
                TrieKey::Arity(3), TrieKey::Atom("="),
                TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("$0"),
                TrieKey::Arity(2), TrieKey::Atom("g"), TrieKey::Atom("$0"),
            ],
            "(= (f $0) (g $0))".to_string(),
            1,
        );

        // Query: (= $lhs $rhs) → keys: [Arity(3), =, Variable, Variable]
        let results = trie.query(&[
            TrieKey::Arity(3), TrieKey::Atom("="),
            TrieKey::Variable,
            TrieKey::Variable,
        ]);

        assert_eq!(results.len(), 1, "Should find the stored rule");
        assert_eq!(results[0].expr, "(= (f $0) (g $0))");
    }

    #[test]
    fn test_count_keys_for_children() {
        // Simple: 2 atom children
        let keys = vec![TrieKey::Atom("a"), TrieKey::Atom("b"), TrieKey::Atom("c")];
        assert_eq!(count_keys_for_children(&keys, 2), 2);

        // Nested: first child is Arity(2) with 2 sub-children
        let keys = vec![
            TrieKey::Arity(2),
            TrieKey::Atom("g"),
            TrieKey::Atom("x"),
            TrieKey::Atom("y"),
        ];
        // 1 child = Arity(2) + 2 sub-children = 3 keys
        assert_eq!(count_keys_for_children(&keys, 1), 3);
        // 2 children = (Arity(2) + 2 sub-children) + Atom("y") = 4 keys
        assert_eq!(count_keys_for_children(&keys, 2), 4);
    }
}
