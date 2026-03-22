//! Discrimination Tree Index for Rule Candidate Pruning
//!
//! A trie-based index that prunes rule candidates by matching the structure
//! of an expression against the structure of rule LHS patterns. This extends
//! the existing 2-level indexing (head symbol + first-arg head) to arbitrary
//! depth, reducing the number of rules that need structural matching.
//!
//! ## Design
//!
//! The discrimination tree is a trie where:
//! - Each edge is labeled with a **discrimination key** (atom name, arity, or type tag)
//! - Each node may hold a set of rule entry indices
//! - Traversal follows the expression's structure depth-first
//! - At each level, both the concrete edge (if it exists) and the wildcard edge
//!   are followed (since variable patterns match anything)
//!
//! ## Key Sequence
//!
//! For a pattern `(f (g $x 1) $y)`, the discrimination key sequence is:
//! ```text
//! [Atom("f"), Arity(2), Atom("g"), Arity(2), Var, Long(1), Var]
//! ```
//!
//! For an expression `(f (g a 1) b)`, the query key sequence is:
//! ```text
//! [Atom("f"), Arity(2), Atom("g"), Arity(2), Atom("a"), Long(1), Atom("b")]
//! ```
//!
//! The trie traversal follows both concrete and wildcard (Var) edges at each
//! level, collecting all rule indices reachable through any path.
//!
//! ## Integration
//!
//! The discrimination tree is consulted BEFORE structural matching. It returns
//! a subset of candidate rule indices, reducing the work for structural matchers.
//! Falls back gracefully: if a rule cannot be indexed (e.g., complex patterns),
//! it's placed in a "catch-all" set that's always included.
//!
//! ## Complexity
//!
//! - **Insert**: O(pattern_depth) per rule
//! - **Query**: O(expr_depth * branching_factor) where branching_factor is
//!   typically 2 (concrete + wildcard edge)
//! - **Space**: O(total_unique_keys * rules_per_key)

use std::collections::HashMap;
use std::fmt::Debug;

use smallvec::SmallVec;

use crate::backend::models::MettaValueTrait;

// ============================================================================
// Discrimination Key
// ============================================================================

/// A key in the discrimination tree, representing one level of pattern structure.
///
/// Keys are extracted from both LHS patterns (for insertion) and expressions
/// (for querying). Variable positions in patterns emit `Var`; the query
/// traversal follows both concrete and Var edges.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiscKey {
    /// An atom symbol (e.g., "f", "+", "if").
    /// Uses the interned `&'static str` pointer for O(1) hashing.
    Atom(&'static str),

    /// The arity of an S-expression.
    /// Emitted after the head atom to discriminate by structure.
    Arity(u16),

    /// An integer literal.
    Long(i64),

    /// A boolean literal.
    Bool(bool),

    /// A float literal (stored as bits for exact hashing).
    Float(u64),

    /// A string literal.
    Str(&'static str),

    /// A variable or wildcard position in a pattern.
    /// During query, the traversal follows Var edges in addition to concrete edges.
    Var,
}

// ============================================================================
// Discrimination Tree Node
// ============================================================================

/// A node in the discrimination tree.
///
/// Each node has:
/// - A map of children indexed by `DiscKey`
/// - A set of rule indices that terminate at this node
///
/// Rule indices are stored as `u32` to save memory (supports up to 4B rules).
#[derive(Debug, Default, Clone)]
struct DiscNode {
    /// Children indexed by discrimination key.
    children: HashMap<DiscKey, DiscNode>,

    /// Rule indices that are fully indexed through this node.
    /// A rule appears here when its entire key sequence has been traversed.
    rule_indices: SmallVec<[u32; 4]>,
}

impl DiscNode {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
            rule_indices: SmallVec::new(),
        }
    }
}

// ============================================================================
// Discrimination Tree
// ============================================================================

/// Trie-based discrimination tree for rule candidate pruning.
///
/// Rules are indexed by their LHS pattern structure. At query time, the tree
/// prunes candidates that cannot possibly match the expression, reducing the
/// number of structural matcher invocations.
///
/// ## Indexing Depth
///
/// The tree indexes patterns up to `max_depth` levels deep (default: 4).
/// Deeper pattern structure is not indexed — rules with deep patterns are
/// still included but not further discriminated.
///
/// ## Catch-All Rules
///
/// Rules that cannot be indexed (e.g., patterns that are bare variables,
/// or patterns with unsupported structure) are placed in `catch_all_indices`.
/// These are always included in query results.
#[derive(Debug, Clone)]
pub struct DiscriminationTree {
    /// Root node of the trie.
    root: DiscNode,

    /// Indices of rules that cannot be discriminated (always included).
    catch_all_indices: SmallVec<[u32; 8]>,

    /// Maximum indexing depth (default: 4).
    max_depth: usize,

    /// Total number of rules indexed.
    total_rules: u32,
}

impl DiscriminationTree {
    /// Create a new discrimination tree with default max depth (4).
    pub fn new() -> Self {
        Self::with_max_depth(4)
    }

    /// Create a new discrimination tree with the given max depth.
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            root: DiscNode::new(),
            catch_all_indices: SmallVec::new(),
            max_depth,
            total_rules: 0,
        }
    }

    /// Insert a rule into the discrimination tree.
    ///
    /// `rule_index` is the index of the rule in the external rule storage
    /// (e.g., index into `Vec<RuleEntry>` within a `RuleGroup`).
    ///
    /// `lhs` is the left-hand side pattern of the rule.
    ///
    /// Returns `true` if the rule was indexed in the trie, `false` if it
    /// was placed in the catch-all set (un-indexable pattern).
    pub fn insert<V: MettaValueTrait>(&mut self, rule_index: u32, lhs: &V) -> bool {
        let mut keys = SmallVec::<[DiscKey; 16]>::new();
        if !Self::extract_keys(lhs, &mut keys, self.max_depth, 0) {
            // Cannot index this pattern — add to catch-all
            self.catch_all_indices.push(rule_index);
            self.total_rules += 1;
            return false;
        }

        if keys.is_empty() || (keys.len() == 1 && keys[0] == DiscKey::Var) {
            // Empty key sequence or bare variable — matches everything, catch-all
            self.catch_all_indices.push(rule_index);
            self.total_rules += 1;
            return false;
        }

        // Walk the trie, creating nodes as needed
        let mut node = &mut self.root;
        for key in &keys {
            node = node.children.entry(key.clone()).or_insert_with(DiscNode::new);
        }
        node.rule_indices.push(rule_index);
        self.total_rules += 1;
        true
    }

    /// Query the discrimination tree for candidate rule indices.
    ///
    /// Returns all rule indices whose key sequences are compatible with the
    /// expression's structure. This includes:
    /// - Rules that match on every concrete key
    /// - Rules with Var keys at positions where the expression has concrete values
    /// - Catch-all rules (always included)
    ///
    /// The result may contain duplicates if a rule is reachable through
    /// multiple paths (e.g., via both concrete and Var edges). The caller
    /// should deduplicate if needed.
    pub fn query<V: MettaValueTrait>(&self, expr: &V) -> SmallVec<[u32; 16]> {
        let mut results = SmallVec::new();

        // Always include catch-all rules
        results.extend(self.catch_all_indices.iter().copied());

        // Extract query keys from expression
        let mut keys = SmallVec::<[DiscKey; 16]>::new();
        Self::extract_query_keys(expr, &mut keys, self.max_depth, 0);

        if keys.is_empty() {
            // No keys to discriminate — return all root-level rules
            self.collect_all_indices(&self.root, &mut results);
            return results;
        }

        // Traverse the trie, following both concrete and Var edges
        self.traverse_query(&self.root, &keys, 0, &mut results);
        results
    }

    /// Return the total number of rules in the tree.
    #[inline]
    pub fn total_rules(&self) -> u32 {
        self.total_rules
    }

    /// Return the number of catch-all rules.
    #[inline]
    pub fn catch_all_count(&self) -> usize {
        self.catch_all_indices.len()
    }

    /// Check if the tree is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_rules == 0
    }

    /// Clear the tree, removing all rules.
    pub fn clear(&mut self) {
        self.root = DiscNode::new();
        self.catch_all_indices.clear();
        self.total_rules = 0;
    }

    // ── Key Extraction (Patterns) ────────────────────────────────────

    /// Extract discrimination keys from a pattern (LHS).
    ///
    /// Returns `false` if the pattern cannot be indexed (unsupported structure).
    fn extract_keys<V: MettaValueTrait>(
        value: &V,
        keys: &mut SmallVec<[DiscKey; 16]>,
        max_depth: usize,
        current_depth: usize,
    ) -> bool {
        if current_depth >= max_depth {
            return true; // Stop indexing at max depth
        }

        // Strip span wrappers
        let value = if value.is_spanned() {
            value.strip_one_span()
        } else {
            value.clone()
        };

        if value.is_variable() {
            keys.push(DiscKey::Var);
            return true;
        }

        if let Some(name) = value.as_atom() {
            if name == "_" {
                keys.push(DiscKey::Var); // Wildcard = variable
            } else {
                keys.push(DiscKey::Atom(name));
            }
            return true;
        }

        if let Some(n) = value.as_long() {
            keys.push(DiscKey::Long(n));
            return true;
        }

        if let Some(b) = value.as_bool() {
            keys.push(DiscKey::Bool(b));
            return true;
        }

        if let Some(f) = value.as_float() {
            keys.push(DiscKey::Float(f.to_bits()));
            return true;
        }

        if let Some(s) = value.as_string() {
            // String interning: only works with 'static str
            // For now, skip string discrimination in patterns
            keys.push(DiscKey::Var); // Treat as variable for safety
            let _ = s;
            return true;
        }

        if let Some(items) = value.as_sexpr() {
            if items.is_empty() {
                keys.push(DiscKey::Arity(0));
                return true;
            }

            // First: head atom
            let head = &items[0];
            if !Self::extract_keys(head, keys, max_depth, current_depth) {
                return false;
            }

            // Then: arity
            keys.push(DiscKey::Arity(items.len() as u16));

            // Then: remaining children (depth-first)
            for child in &items[1..] {
                if !Self::extract_keys(child, keys, max_depth, current_depth + 1) {
                    return false;
                }
            }

            return true;
        }

        if value.is_quoted() || value.is_type() || value.is_error() || value.is_conjunction() {
            // Complex patterns — cannot index
            return false;
        }

        if value.is_unit() || value.is_empty() {
            keys.push(DiscKey::Var); // Treat as catchall
            return true;
        }

        // Unknown type — cannot index
        false
    }

    // ── Key Extraction (Expressions / Queries) ───────────────────────

    /// Extract discrimination keys from an expression (for querying).
    ///
    /// Similar to `extract_keys` but never emits `Var` (expressions don't
    /// have variables in the queried positions — or if they do, they're
    /// treated as concrete atoms).
    fn extract_query_keys<V: MettaValueTrait>(
        value: &V,
        keys: &mut SmallVec<[DiscKey; 16]>,
        max_depth: usize,
        current_depth: usize,
    ) {
        if current_depth >= max_depth {
            return;
        }

        // Strip span wrappers
        let value = if value.is_spanned() {
            value.strip_one_span()
        } else {
            value.clone()
        };

        if let Some(name) = value.as_atom() {
            keys.push(DiscKey::Atom(name));
            return;
        }

        if let Some(n) = value.as_long() {
            keys.push(DiscKey::Long(n));
            return;
        }

        if let Some(b) = value.as_bool() {
            keys.push(DiscKey::Bool(b));
            return;
        }

        if let Some(f) = value.as_float() {
            keys.push(DiscKey::Float(f.to_bits()));
            return;
        }

        if let Some(items) = value.as_sexpr() {
            if items.is_empty() {
                keys.push(DiscKey::Arity(0));
                return;
            }

            // Head
            Self::extract_query_keys(&items[0], keys, max_depth, current_depth);

            // Arity
            keys.push(DiscKey::Arity(items.len() as u16));

            // Children
            for child in &items[1..] {
                Self::extract_query_keys(child, keys, max_depth, current_depth + 1);
            }

            return;
        }

        // For other types (quoted, type, error, etc.), emit nothing
        // The traversal will stop here, and any trie-indexed rules
        // reachable up to this point will be included.
    }

    // ── Trie Traversal ───────────────────────────────────────────────

    /// Traverse the trie following both concrete and Var edges.
    fn traverse_query(
        &self,
        node: &DiscNode,
        keys: &[DiscKey],
        key_idx: usize,
        results: &mut SmallVec<[u32; 16]>,
    ) {
        // Collect any rules that terminate at this node
        results.extend(node.rule_indices.iter().copied());

        if key_idx >= keys.len() {
            // No more keys — collect all reachable rules from descendants
            // (rules with shorter patterns that still match)
            return;
        }

        let key = &keys[key_idx];

        // Follow the concrete edge (exact match)
        if let Some(child) = node.children.get(key) {
            self.traverse_query(child, keys, key_idx + 1, results);
        }

        // Follow the Var edge (wildcard match) — unless key IS Var
        if *key != DiscKey::Var {
            if let Some(var_child) = node.children.get(&DiscKey::Var) {
                // Skip to next non-child key for the Var edge.
                // For S-expressions, Var matches the entire argument (including
                // its children), so we need to skip the right number of keys.
                let skip_count = self.count_subtree_keys(keys, key_idx);
                self.traverse_query(var_child, keys, key_idx + skip_count, results);
            }
        }
    }

    /// Count how many keys a subtree at the given position spans.
    ///
    /// For atoms/literals: 1 key.
    /// For S-expressions: 1 (head) + 1 (arity) + sum(children's key counts).
    fn count_subtree_keys(&self, keys: &[DiscKey], start: usize) -> usize {
        if start >= keys.len() {
            return 1;
        }

        match &keys[start] {
            DiscKey::Atom(_) | DiscKey::Long(_) | DiscKey::Bool(_)
            | DiscKey::Float(_) | DiscKey::Str(_) | DiscKey::Var => {
                // Check if next key is Arity (this atom is head of S-expr)
                if start + 1 < keys.len() {
                    if let DiscKey::Arity(arity) = &keys[start + 1] {
                        // S-expression: head + arity + children
                        let arity = *arity as usize;
                        let mut total = 2; // head + arity
                        let mut pos = start + 2;
                        for _ in 1..arity { // skip head (already counted)
                            let child_keys = self.count_subtree_keys(keys, pos);
                            total += child_keys;
                            pos += child_keys;
                        }
                        return total;
                    }
                }
                1 // Simple atom/literal
            }
            DiscKey::Arity(_) => 1, // Should not appear as subtree start
        }
    }

    /// Recursively collect all rule indices reachable from a node.
    fn collect_all_indices(&self, node: &DiscNode, results: &mut SmallVec<[u32; 16]>) {
        results.extend(node.rule_indices.iter().copied());
        for child in node.children.values() {
            self.collect_all_indices(child, results);
        }
    }
}

impl Default for DiscriminationTree {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValue, MettaValueFactory, global_factory};

    fn f() -> crate::backend::models::GcFactory {
        global_factory()
    }

    /// Helper to create (f $x $y) pattern
    fn pattern_f_xy() -> MettaValue {
        f().sexpr(vec![f().atom("f"), f().atom("$x"), f().atom("$y")])
    }

    /// Helper to create (f 1 $y) pattern
    fn pattern_f_1y() -> MettaValue {
        f().sexpr(vec![f().atom("f"), f().long(1), f().atom("$y")])
    }

    /// Helper to create (f 2 $y) pattern
    fn pattern_f_2y() -> MettaValue {
        f().sexpr(vec![f().atom("f"), f().long(2), f().atom("$y")])
    }

    /// Helper to create (g $x) pattern
    fn pattern_g_x() -> MettaValue {
        f().sexpr(vec![f().atom("g"), f().atom("$x")])
    }

    /// Helper to create (f (g $x) $y) pattern
    fn pattern_f_gx_y() -> MettaValue {
        f().sexpr(vec![
            f().atom("f"),
            f().sexpr(vec![f().atom("g"), f().atom("$x")]),
            f().atom("$y"),
        ])
    }

    #[test]
    fn test_empty_tree() {
        let tree = DiscriminationTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.total_rules(), 0);
    }

    #[test]
    fn test_insert_and_query_single_rule() {
        let mut tree = DiscriminationTree::new();

        tree.insert(0, &pattern_f_xy());
        assert_eq!(tree.total_rules(), 1);

        // Query with matching expression
        let expr = f().sexpr(vec![f().atom("f"), f().long(1), f().long(2)]);
        let results = tree.query(&expr);
        assert!(results.contains(&0), "Rule 0 should match (f 1 2)");
    }

    #[test]
    fn test_discrimination_by_first_arg() {
        let mut tree = DiscriminationTree::new();

        tree.insert(0, &pattern_f_1y()); // (f 1 $y)
        tree.insert(1, &pattern_f_2y()); // (f 2 $y)

        // Query (f 1 42) — should match rule 0, not rule 1
        let expr1 = f().sexpr(vec![f().atom("f"), f().long(1), f().long(42)]);
        let results1 = tree.query(&expr1);
        assert!(results1.contains(&0), "Rule 0 should match (f 1 42)");
        assert!(!results1.contains(&1), "Rule 1 should NOT match (f 1 42)");

        // Query (f 2 42) — should match rule 1, not rule 0
        let expr2 = f().sexpr(vec![f().atom("f"), f().long(2), f().long(42)]);
        let results2 = tree.query(&expr2);
        assert!(!results2.contains(&0), "Rule 0 should NOT match (f 2 42)");
        assert!(results2.contains(&1), "Rule 1 should match (f 2 42)");
    }

    #[test]
    fn test_variable_pattern_matches_anything() {
        let mut tree = DiscriminationTree::new();

        tree.insert(0, &pattern_f_xy()); // (f $x $y) — variables match anything
        tree.insert(1, &pattern_f_1y()); // (f 1 $y) — concrete first arg

        // Query (f 1 2) — both rules match
        let expr = f().sexpr(vec![f().atom("f"), f().long(1), f().long(2)]);
        let results = tree.query(&expr);
        assert!(results.contains(&0), "Variable pattern should match");
        assert!(results.contains(&1), "Concrete pattern should match");

        // Query (f 3 4) — only variable pattern matches
        let expr2 = f().sexpr(vec![f().atom("f"), f().long(3), f().long(4)]);
        let results2 = tree.query(&expr2);
        assert!(results2.contains(&0), "Variable pattern should match anything");
        assert!(!results2.contains(&1), "Concrete (f 1 $y) shouldn't match (f 3 4)");
    }

    #[test]
    fn test_arity_discrimination() {
        let mut tree = DiscriminationTree::new();

        tree.insert(0, &pattern_f_xy()); // (f $x $y) — arity 3
        tree.insert(1, &pattern_g_x());  // (g $x) — arity 2

        // Query (f 1 2) — only rule 0
        let expr = f().sexpr(vec![f().atom("f"), f().long(1), f().long(2)]);
        let results = tree.query(&expr);
        assert!(results.contains(&0));
        assert!(!results.contains(&1));

        // Query (g 1) — only rule 1
        let expr2 = f().sexpr(vec![f().atom("g"), f().long(1)]);
        let results2 = tree.query(&expr2);
        assert!(!results2.contains(&0));
        assert!(results2.contains(&1));
    }

    #[test]
    fn test_nested_pattern_discrimination() {
        let mut tree = DiscriminationTree::new();

        tree.insert(0, &pattern_f_gx_y()); // (f (g $x) $y)
        tree.insert(1, &pattern_f_1y());    // (f 1 $y)

        // Query (f (g 42) 99) — only rule 0
        let expr = f().sexpr(vec![
            f().atom("f"),
            f().sexpr(vec![f().atom("g"), f().long(42)]),
            f().long(99),
        ]);
        let results = tree.query(&expr);
        assert!(results.contains(&0), "Nested pattern should match");
        assert!(!results.contains(&1), "Flat pattern shouldn't match nested expr");

        // Query (f 1 99) — only rule 1
        let expr2 = f().sexpr(vec![f().atom("f"), f().long(1), f().long(99)]);
        let results2 = tree.query(&expr2);
        assert!(!results2.contains(&0), "Nested pattern shouldn't match flat expr");
        assert!(results2.contains(&1), "Flat pattern should match");
    }

    #[test]
    fn test_catch_all_for_variable_pattern() {
        let mut tree = DiscriminationTree::new();

        // Bare variable pattern — should be catch-all
        let var_pattern = f().atom("$x");
        let indexed = tree.insert(0, &var_pattern);
        assert!(!indexed, "Bare variable should be catch-all");
        assert_eq!(tree.catch_all_count(), 1);

        // Any query should include catch-all
        let expr = f().long(42);
        let results = tree.query(&expr);
        assert!(results.contains(&0));
    }

    #[test]
    fn test_clear() {
        let mut tree = DiscriminationTree::new();
        tree.insert(0, &pattern_f_xy());
        tree.insert(1, &pattern_g_x());

        tree.clear();
        assert!(tree.is_empty());
        assert_eq!(tree.total_rules(), 0);
    }

    #[test]
    fn test_many_rules_same_head() {
        let mut tree = DiscriminationTree::new();

        // Insert 10 rules with head "f" but different structures
        for i in 0..10u32 {
            let pattern = f().sexpr(vec![
                f().atom("f"),
                f().long(i as i64),
                f().atom("$y"),
            ]);
            tree.insert(i, &pattern);
        }

        // Query (f 5 99) — should only match rule 5
        let expr = f().sexpr(vec![f().atom("f"), f().long(5), f().long(99)]);
        let results = tree.query(&expr);
        assert!(results.contains(&5));
        // Others should not match
        for i in 0..10u32 {
            if i != 5 {
                assert!(!results.contains(&i), "Rule {} should not match (f 5 99)", i);
            }
        }
    }

    #[test]
    fn test_bool_discrimination() {
        let mut tree = DiscriminationTree::new();

        let pattern_true = f().sexpr(vec![f().atom("check"), f().bool(true)]);
        let pattern_false = f().sexpr(vec![f().atom("check"), f().bool(false)]);

        tree.insert(0, &pattern_true);
        tree.insert(1, &pattern_false);

        let expr = f().sexpr(vec![f().atom("check"), f().bool(true)]);
        let results = tree.query(&expr);
        assert!(results.contains(&0));
        assert!(!results.contains(&1));
    }

    #[test]
    fn test_max_depth_limits_indexing() {
        let mut tree = DiscriminationTree::with_max_depth(1);

        // With max_depth=1, only head+arity is indexed (no children)
        tree.insert(0, &pattern_f_1y()); // (f 1 $y)
        tree.insert(1, &pattern_f_2y()); // (f 2 $y)

        // Both share head="f", arity=3 — depth-1 can't discriminate by first arg
        let expr = f().sexpr(vec![f().atom("f"), f().long(1), f().long(42)]);
        let results = tree.query(&expr);
        // Both should be returned (can't discriminate at depth 1)
        assert!(results.contains(&0));
        assert!(results.contains(&1));
    }

    #[test]
    fn test_key_extraction_depth_limiting() {
        let mut keys = SmallVec::new();

        // Deep pattern — should stop at max_depth
        let deep = f().sexpr(vec![
            f().atom("a"),
            f().sexpr(vec![
                f().atom("b"),
                f().sexpr(vec![
                    f().atom("c"),
                    f().sexpr(vec![
                        f().atom("d"),
                        f().atom("$x"),
                    ]),
                ]),
            ]),
        ]);

        DiscriminationTree::extract_keys(&deep, &mut keys, 2, 0);
        // At max_depth=2, we should index: a, Arity(2), b, Arity(2) and then stop
        assert!(keys.len() <= 8, "Keys should be bounded by max_depth");
    }
}
