//! MettaTrie Node Structure
//!
//! Defines `MettaTrieNode<K, V>`, the internal trie node with Arc-based
//! structural sharing for CoW (clone-on-write) semantics.
//!
//! ## Structural Sharing
//!
//! Nodes use `Arc<MettaTrieNode<K, V>>` for children, enabling:
//! - O(1) trie clone via `Arc::clone` on the root
//! - O(depth) mutation via path-copy (only cloned nodes from root to mutation point)
//! - Safe concurrent reads while one thread mutates a forked copy
//!
//! ## Entry Storage
//!
//! Each node stores an optional `(E, V)` entry where:
//! - `E` is the original expression that was decomposed to reach this path
//! - `V` is the mapped value (e.g., `Multiplicity`)
//!
//! Storing the original expression avoids costly reconstruction from the
//! trie path during iteration.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::keys::TrieKey;

/// A node in the MettaTrie.
///
/// Generic over:
/// - `E`: The expression type stored at leaves (e.g., `MettaValue`)
/// - `V`: The value type mapped to by the trie (e.g., `Multiplicity`)
///
/// Nodes are reference-counted via `Arc` for structural sharing.
pub struct MettaTrieNode<E, V> {
    /// The entry at this path: original expression + mapped value.
    /// `None` if this is an interior node with no value.
    pub(crate) entry: Option<(E, V)>,

    /// Children indexed by discrimination key.
    /// Empty HashMap for leaf nodes.
    pub(crate) children: HashMap<TrieKey, Arc<MettaTrieNode<E, V>>>,
}

impl<E: Clone, V: Clone> Clone for MettaTrieNode<E, V> {
    fn clone(&self) -> Self {
        Self {
            entry: self.entry.clone(),
            children: self.children.clone(), // Arc children are cheap to clone
        }
    }
}

impl<E, V> Default for MettaTrieNode<E, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E, V> MettaTrieNode<E, V> {
    /// Create a new empty node.
    #[inline]
    pub fn new() -> Self {
        Self {
            entry: None,
            children: HashMap::new(),
        }
    }

    /// Check if this node has a value.
    #[inline]
    pub fn has_entry(&self) -> bool {
        self.entry.is_some()
    }

    /// Check if this node is a leaf (no children).
    #[inline]
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    /// Number of children.
    #[inline]
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

impl<E: fmt::Debug, V: fmt::Debug> fmt::Debug for MettaTrieNode<E, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MettaTrieNode")
            .field("has_entry", &self.entry.is_some())
            .field("children", &self.children.len())
            .finish()
    }
}

/// The MettaTrie: a trie-map discriminating on `TrieKey` sequences.
///
/// Generic over:
/// - `E`: The expression type stored at leaves (e.g., `MettaValue`)
/// - `V`: The value type mapped to by the trie (e.g., `Multiplicity`)
///
/// ## Clone Semantics
///
/// `MettaTrie::clone()` is O(1) — it increments the root `Arc` reference count.
/// Subsequent mutations on the clone trigger path-copy (only the modified path
/// from root to leaf is cloned).
pub struct MettaTrie<E, V> {
    /// Root node of the trie, wrapped in Arc for structural sharing.
    root: Arc<MettaTrieNode<E, V>>,

    /// Cached count of entries (values) in the trie.
    /// Maintained incrementally during insert/remove.
    pub(crate) val_count: usize,
}

impl<E: Clone, V: Clone> Clone for MettaTrie<E, V> {
    /// O(1) clone via Arc reference count increment.
    #[inline]
    fn clone(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            val_count: self.val_count,
        }
    }
}

impl<E, V> Default for MettaTrie<E, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E, V> MettaTrie<E, V> {
    /// Create a new empty MettaTrie.
    #[inline]
    pub fn new() -> Self {
        Self {
            root: Arc::new(MettaTrieNode::new()),
            val_count: 0,
        }
    }

    /// Number of entries (values) in the trie.
    #[inline]
    pub fn val_count(&self) -> usize {
        self.val_count
    }

    /// Check if the trie is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.val_count == 0
    }

    /// Get a reference to the root node.
    #[inline]
    pub fn root(&self) -> &MettaTrieNode<E, V> {
        &self.root
    }
}

impl<E: Clone, V: Clone> MettaTrie<E, V> {
    /// Get a mutable reference to the root node, performing CoW if needed.
    ///
    /// If the Arc has other references, the node is cloned first.
    #[inline]
    pub(crate) fn root_mut(&mut self) -> &mut MettaTrieNode<E, V> {
        Arc::make_mut(&mut self.root)
    }

    /// Navigate to a node at the given key path, creating nodes as needed.
    /// Returns a mutable reference to the target node.
    ///
    /// Performs CoW (clone-on-write) on each node along the path.
    pub(crate) fn navigate_to_mut(&mut self, keys: &[TrieKey]) -> &mut MettaTrieNode<E, V> {
        let mut current = Arc::make_mut(&mut self.root);
        for key in keys {
            let child = current
                .children
                .entry(key.clone())
                .or_insert_with(|| Arc::new(MettaTrieNode::new()));
            current = Arc::make_mut(child);
        }
        current
    }

    /// Navigate to a node at the given key path for reading.
    /// Returns `None` if any node along the path doesn't exist.
    pub(crate) fn navigate_to(&self, keys: &[TrieKey]) -> Option<&MettaTrieNode<E, V>> {
        let mut current: &MettaTrieNode<E, V> = &self.root;
        for key in keys {
            match current.children.get(key) {
                Some(child) => current = child,
                None => return None,
            }
        }
        Some(current)
    }

    /// Insert an entry at the given key path.
    ///
    /// Returns the previous value if one existed at this path.
    pub fn insert_at(&mut self, keys: &[TrieKey], expr: E, value: V) -> Option<V> {
        let node = self.navigate_to_mut(keys);
        let old = node.entry.take().map(|(_, v)| v);
        node.entry = Some((expr, value));
        if old.is_none() {
            self.val_count += 1;
        }
        old
    }

    /// Check if an entry exists at the given key path.
    #[inline]
    pub fn contains(&self, keys: &[TrieKey]) -> bool {
        self.get_at(keys).is_some()
    }

    /// Get the value at the given key path.
    pub fn get_at(&self, keys: &[TrieKey]) -> Option<&V> {
        self.navigate_to(keys)
            .and_then(|node| node.entry.as_ref().map(|(_, v)| v))
    }

    /// Get the full entry (expression + value) at the given key path.
    pub fn get_entry_at(&self, keys: &[TrieKey]) -> Option<(&E, &V)> {
        self.navigate_to(keys)
            .and_then(|node| node.entry.as_ref().map(|(e, v)| (e, v)))
    }

    /// Remove the entry at the given key path.
    ///
    /// Returns the removed entry if one existed.
    /// Prunes empty interior nodes along the path.
    pub fn remove_at(&mut self, keys: &[TrieKey]) -> Option<(E, V)> {
        if keys.is_empty() {
            let node = Arc::make_mut(&mut self.root);
            let removed = node.entry.take();
            if removed.is_some() {
                self.val_count -= 1;
            }
            return removed;
        }

        // Navigate to the parent of the target node
        let result = Self::remove_recursive(Arc::make_mut(&mut self.root), keys);
        if result.is_some() {
            self.val_count -= 1;
        }
        result
    }

    /// Recursively remove an entry and prune empty nodes.
    fn remove_recursive(node: &mut MettaTrieNode<E, V>, keys: &[TrieKey]) -> Option<(E, V)> {
        if keys.is_empty() {
            return node.entry.take();
        }

        let (first, rest) = keys.split_first().expect("keys is non-empty");

        let result = if let Some(child_arc) = node.children.get_mut(first) {
            let child = Arc::make_mut(child_arc);
            Self::remove_recursive(child, rest)
        } else {
            return None;
        };

        // Prune empty child nodes
        if result.is_some() {
            if let Some(child) = node.children.get(first) {
                if child.entry.is_none() && child.children.is_empty() {
                    node.children.remove(first);
                }
            }
        }

        result
    }

    /// Iterate over all entries in the trie (depth-first).
    ///
    /// Yields `(&E, &V)` pairs — the stored expression and its mapped value.
    pub fn iter(&self) -> TrieIter<'_, E, V> {
        let mut stack = Vec::with_capacity(16);
        stack.push(self.root.as_ref());
        TrieIter { stack }
    }
}

// ============================================================================
// Algebraic Operations
// ============================================================================

impl<E: Clone, V: Clone + crate::algebra::Lattice> MettaTrie<E, V> {
    /// Join (union) of two tries.
    ///
    /// For paths present in both tries, values are combined via `Lattice::pjoin`.
    /// For paths present in only one, the entry is included as-is.
    pub fn join(&self, other: &Self) -> Self {
        let mut result = self.clone();
        Self::join_nodes(Arc::make_mut(&mut result.root), &other.root, &mut result.val_count);
        result
    }

    /// Restrict: keep only paths that exist in `other`, preserving `self`'s values.
    ///
    /// Unlike `meet` (which combines values), `restrict` keeps `self`'s values
    /// unchanged — `other` acts purely as a path filter.
    pub fn restrict(&self, other: &Self) -> Self {
        let mut result = MettaTrie::new();
        Self::restrict_nodes(&self.root, &other.root, Arc::make_mut(&mut result.root), &mut result.val_count);
        result
    }

    /// Meet (intersection) of two tries.
    ///
    /// Only paths present in both tries are included, with values combined
    /// via `Lattice::pmeet`.
    pub fn meet(&self, other: &Self) -> Self {
        let mut result = MettaTrie::new();
        Self::meet_nodes(&self.root, &other.root, Arc::make_mut(&mut result.root), &mut result.val_count);
        result
    }

    fn join_nodes(target: &mut MettaTrieNode<E, V>, source: &MettaTrieNode<E, V>, count: &mut usize) {
        // Merge entries at this node
        if let Some((src_expr, src_val)) = &source.entry {
            if let Some((_, tgt_val)) = &target.entry {
                match tgt_val.pjoin(src_val) {
                    crate::algebra::AlgebraicResult::None => {
                        target.entry = None;
                        *count -= 1;
                    }
                    crate::algebra::AlgebraicResult::Identity(mask) => {
                        if mask == crate::algebra::COUNTER_IDENT {
                            target.entry = Some((src_expr.clone(), src_val.clone()));
                        }
                        // SELF_IDENT: keep target as-is
                    }
                    crate::algebra::AlgebraicResult::Element(v) => {
                        target.entry = Some((target.entry.as_ref().expect("checked above").0.clone(), v));
                    }
                }
            } else {
                target.entry = Some((src_expr.clone(), src_val.clone()));
                *count += 1;
            }
        }

        // Merge children
        for (key, src_child) in &source.children {
            let tgt_child = target
                .children
                .entry(key.clone())
                .or_insert_with(|| Arc::new(MettaTrieNode::new()));
            Self::join_nodes(Arc::make_mut(tgt_child), src_child, count);
        }
    }

    fn meet_nodes(a: &MettaTrieNode<E, V>, b: &MettaTrieNode<E, V>, result: &mut MettaTrieNode<E, V>, count: &mut usize) {
        // Meet entries at this node
        if let (Some((a_expr, a_val)), Some((_, b_val))) = (&a.entry, &b.entry) {
            match a_val.pmeet(b_val) {
                crate::algebra::AlgebraicResult::None => {}
                crate::algebra::AlgebraicResult::Identity(mask) => {
                    if mask == crate::algebra::SELF_IDENT {
                        result.entry = Some((a_expr.clone(), a_val.clone()));
                    } else {
                        result.entry = Some((a_expr.clone(), b_val.clone()));
                    }
                    *count += 1;
                }
                crate::algebra::AlgebraicResult::Element(v) => {
                    result.entry = Some((a_expr.clone(), v));
                    *count += 1;
                }
            }
        }

        // Recurse into shared children
        for (key, a_child) in &a.children {
            if let Some(b_child) = b.children.get(key) {
                let mut result_child = MettaTrieNode::new();
                Self::meet_nodes(a_child, b_child, &mut result_child, count);
                if result_child.entry.is_some() || !result_child.children.is_empty() {
                    result.children.insert(key.clone(), Arc::new(result_child));
                }
            }
        }
    }

    /// Merge with max semantics: at each path, keep the entry with the greater value.
    ///
    /// Unlike `join` (which combines values via `pjoin`), `merge_max` takes the
    /// entry with the larger value according to `Ord`.
    pub fn merge_max(&self, other: &Self) -> Self
    where
        V: Ord,
    {
        let mut result = self.clone();
        Self::merge_max_nodes(Arc::make_mut(&mut result.root), &other.root, &mut result.val_count);
        result
    }

    fn merge_max_nodes(target: &mut MettaTrieNode<E, V>, source: &MettaTrieNode<E, V>, count: &mut usize)
    where
        V: Ord,
    {
        if let Some((src_expr, src_val)) = &source.entry {
            match &target.entry {
                Some((_, tgt_val)) => {
                    if src_val > tgt_val {
                        target.entry = Some((src_expr.clone(), src_val.clone()));
                    }
                }
                None => {
                    target.entry = Some((src_expr.clone(), src_val.clone()));
                    *count += 1;
                }
            }
        }

        for (key, src_child) in &source.children {
            let tgt_child = target
                .children
                .entry(key.clone())
                .or_insert_with(|| Arc::new(MettaTrieNode::new()));
            Self::merge_max_nodes(Arc::make_mut(tgt_child), src_child, count);
        }
    }

    fn restrict_nodes(a: &MettaTrieNode<E, V>, b: &MettaTrieNode<E, V>, result: &mut MettaTrieNode<E, V>, count: &mut usize) {
        // Keep a's entry only if b also has an entry at this path
        if a.entry.is_some() && b.entry.is_some() {
            result.entry = a.entry.clone();
            *count += 1;
        }

        // Recurse into shared children
        for (key, a_child) in &a.children {
            if let Some(b_child) = b.children.get(key) {
                let mut result_child = MettaTrieNode::new();
                Self::restrict_nodes(a_child, b_child, &mut result_child, count);
                if result_child.entry.is_some() || !result_child.children.is_empty() {
                    result.children.insert(key.clone(), Arc::new(result_child));
                }
            }
        }
    }
}

impl<E: Clone, V: Clone + crate::algebra::DistributiveLattice> MettaTrie<E, V> {
    /// Subtract: remove entries in `other` from `self`.
    ///
    /// For paths present in both, values are combined via `DistributiveLattice::psubtract`.
    /// If subtraction yields `None`, the entry is removed.
    pub fn subtract(&self, other: &Self) -> Self {
        let mut result = self.clone();
        Self::subtract_nodes(Arc::make_mut(&mut result.root), &other.root, &mut result.val_count);
        result
    }

    fn subtract_nodes(target: &mut MettaTrieNode<E, V>, source: &MettaTrieNode<E, V>, count: &mut usize) {
        // Subtract entries at this node
        if let (Some((_, tgt_val)), Some((_, src_val))) = (&target.entry, &source.entry) {
            match tgt_val.psubtract(src_val) {
                crate::algebra::AlgebraicResult::None => {
                    target.entry = None;
                    *count -= 1;
                }
                crate::algebra::AlgebraicResult::Identity(mask) => {
                    if mask == crate::algebra::COUNTER_IDENT {
                        target.entry = Some((target.entry.as_ref().expect("checked").0.clone(), src_val.clone()));
                    }
                    // SELF_IDENT: keep as-is
                }
                crate::algebra::AlgebraicResult::Element(v) => {
                    target.entry = Some((target.entry.as_ref().expect("checked").0.clone(), v));
                }
            }
        }

        // Recurse into shared children
        let keys_to_check: Vec<_> = source.children.keys().cloned().collect();
        for key in keys_to_check {
            if let Some(src_child) = source.children.get(&key) {
                if let Some(tgt_child) = target.children.get_mut(&key) {
                    Self::subtract_nodes(Arc::make_mut(tgt_child), src_child, count);
                    // Prune empty children
                    if let Some(child) = target.children.get(&key) {
                        if child.entry.is_none() && child.children.is_empty() {
                            target.children.remove(&key);
                        }
                    }
                }
            }
        }
    }
}

impl<E: fmt::Debug, V: fmt::Debug> fmt::Debug for MettaTrie<E, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MettaTrie")
            .field("val_count", &self.val_count)
            .finish()
    }
}

/// Depth-first iterator over all entries in a MettaTrie.
pub struct TrieIter<'a, E, V> {
    stack: Vec<&'a MettaTrieNode<E, V>>,
}

impl<'a, E, V> Iterator for TrieIter<'a, E, V> {
    type Item = (&'a E, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let node = self.stack.pop()?;

            // Push children for later traversal
            for child in node.children.values() {
                self.stack.push(child);
            }

            // Yield this node's entry if it has one
            if let Some((expr, value)) = &node.entry {
                return Some((expr, value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_trie() {
        let trie: MettaTrie<String, u64> = MettaTrie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.val_count(), 0);
    }

    #[test]
    fn test_insert_and_get() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Arity(2), TrieKey::Atom("foo"), TrieKey::Long(42)];

        assert!(trie.insert_at(&keys, "expr".to_string(), 1).is_none());
        assert_eq!(trie.val_count(), 1);
        assert_eq!(trie.get_at(&keys), Some(&1));
    }

    #[test]
    fn test_insert_replaces() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("x")];

        trie.insert_at(&keys, "x".to_string(), 1);
        let old = trie.insert_at(&keys, "x".to_string(), 2);
        assert_eq!(old, Some(1));
        assert_eq!(trie.val_count(), 1);
        assert_eq!(trie.get_at(&keys), Some(&2));
    }

    #[test]
    fn test_remove() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Arity(2), TrieKey::Atom("foo"), TrieKey::Long(42)];

        trie.insert_at(&keys, "expr".to_string(), 1);
        let removed = trie.remove_at(&keys);
        assert_eq!(removed, Some(("expr".to_string(), 1)));
        assert!(trie.is_empty());
        assert_eq!(trie.get_at(&keys), None);
    }

    #[test]
    fn test_remove_prunes_empty_nodes() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Arity(2), TrieKey::Atom("foo"), TrieKey::Long(42)];

        trie.insert_at(&keys, "expr".to_string(), 1);
        trie.remove_at(&keys);

        // Interior nodes should be pruned
        assert!(trie.root().children.is_empty());
    }

    #[test]
    fn test_cow_clone_isolation() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("x")];
        trie.insert_at(&keys, "x".to_string(), 1);

        // Clone (O(1) Arc increment)
        let mut forked = trie.clone();

        // Mutate the fork
        forked.insert_at(&keys, "x".to_string(), 42);

        // Original unchanged
        assert_eq!(trie.get_at(&keys), Some(&1));
        // Fork changed
        assert_eq!(forked.get_at(&keys), Some(&42));
    }

    #[test]
    fn test_iter() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(&[TrieKey::Atom("a")], "a".to_string(), 1);
        trie.insert_at(&[TrieKey::Atom("b")], "b".to_string(), 2);
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("x")],
            "(f x)".to_string(),
            3,
        );

        let mut entries: Vec<_> = trie.iter().map(|(e, v)| (e.clone(), *v)).collect();
        entries.sort_by_key(|(_, v)| *v);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("a".to_string(), 1));
        assert_eq!(entries[1], ("b".to_string(), 2));
        assert_eq!(entries[2], ("(f x)".to_string(), 3));
    }

    #[test]
    fn test_multiple_paths_shared_prefix() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();

        // (f x) and (f y) share the Arity(2) + Atom("f") prefix
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("x")],
            "(f x)".to_string(),
            1,
        );
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("y")],
            "(f y)".to_string(),
            2,
        );

        assert_eq!(trie.val_count(), 2);
        assert_eq!(
            trie.get_at(&[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("x")]),
            Some(&1)
        );
        assert_eq!(
            trie.get_at(&[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("y")]),
            Some(&2)
        );
    }

    #[test]
    fn test_get_entry() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("hello")];
        trie.insert_at(&keys, "hello_expr".to_string(), 99);

        let entry = trie.get_entry_at(&keys);
        assert_eq!(entry, Some((&"hello_expr".to_string(), &99)));
    }

    // ── Algebraic operation tests ──────────────────────────────────────

    /// Counter type implementing Lattice for algebraic operation tests.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct Count(u64);

    impl crate::algebra::Lattice for Count {
        fn pjoin(&self, other: &Self) -> crate::algebra::AlgebraicResult<Self> {
            crate::algebra::AlgebraicResult::Element(Count(self.0 + other.0))
        }
        fn pmeet(&self, other: &Self) -> crate::algebra::AlgebraicResult<Self> {
            let min = self.0.min(other.0);
            if min == self.0 {
                crate::algebra::AlgebraicResult::Identity(crate::algebra::SELF_IDENT)
            } else {
                crate::algebra::AlgebraicResult::Identity(crate::algebra::COUNTER_IDENT)
            }
        }
    }

    impl crate::algebra::DistributiveLattice for Count {
        fn psubtract(&self, other: &Self) -> crate::algebra::AlgebraicResult<Self> {
            if other.0 >= self.0 {
                crate::algebra::AlgebraicResult::None
            } else {
                crate::algebra::AlgebraicResult::Element(Count(self.0 - other.0))
            }
        }
    }

    #[test]
    fn test_join_disjoint() {
        let mut a: MettaTrie<String, Count> = MettaTrie::new();
        let mut b: MettaTrie<String, Count> = MettaTrie::new();
        a.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(1));
        b.insert_at(&[TrieKey::Atom("y")], "y".into(), Count(2));

        let result = a.join(&b);
        assert_eq!(result.val_count(), 2);
        assert_eq!(result.get_at(&[TrieKey::Atom("x")]), Some(&Count(1)));
        assert_eq!(result.get_at(&[TrieKey::Atom("y")]), Some(&Count(2)));
    }

    #[test]
    fn test_join_overlapping() {
        let mut a: MettaTrie<String, Count> = MettaTrie::new();
        let mut b: MettaTrie<String, Count> = MettaTrie::new();
        a.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(3));
        b.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(5));

        let result = a.join(&b);
        assert_eq!(result.val_count(), 1);
        // pjoin is additive: 3 + 5 = 8
        assert_eq!(result.get_at(&[TrieKey::Atom("x")]), Some(&Count(8)));
    }

    #[test]
    fn test_meet_overlapping() {
        let mut a: MettaTrie<String, Count> = MettaTrie::new();
        let mut b: MettaTrie<String, Count> = MettaTrie::new();
        a.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(3));
        a.insert_at(&[TrieKey::Atom("y")], "y".into(), Count(7));
        b.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(5));
        b.insert_at(&[TrieKey::Atom("z")], "z".into(), Count(9));

        let result = a.meet(&b);
        // Only "x" is in both
        assert_eq!(result.val_count(), 1);
        // pmeet returns minimum: min(3, 5) = 3
        assert_eq!(result.get_at(&[TrieKey::Atom("x")]), Some(&Count(3)));
        assert_eq!(result.get_at(&[TrieKey::Atom("y")]), None);
        assert_eq!(result.get_at(&[TrieKey::Atom("z")]), None);
    }

    #[test]
    fn test_subtract() {
        let mut a: MettaTrie<String, Count> = MettaTrie::new();
        let mut b: MettaTrie<String, Count> = MettaTrie::new();
        a.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(5));
        a.insert_at(&[TrieKey::Atom("y")], "y".into(), Count(3));
        b.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(2));
        b.insert_at(&[TrieKey::Atom("y")], "y".into(), Count(10));

        let result = a.subtract(&b);
        // x: 5 - 2 = 3 (kept)
        assert_eq!(result.get_at(&[TrieKey::Atom("x")]), Some(&Count(3)));
        // y: 3 - 10 → None (annihilated, removed)
        assert_eq!(result.get_at(&[TrieKey::Atom("y")]), None);
        assert_eq!(result.val_count(), 1);
    }

    #[test]
    fn test_merge_max() {
        let mut a: MettaTrie<String, Count> = MettaTrie::new();
        let mut b: MettaTrie<String, Count> = MettaTrie::new();
        a.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(3));
        a.insert_at(&[TrieKey::Atom("y")], "y".into(), Count(7));
        b.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(5));
        b.insert_at(&[TrieKey::Atom("z")], "z".into(), Count(9));

        let result = a.merge_max(&b);
        assert_eq!(result.val_count(), 3); // x, y, z
        assert_eq!(result.get_at(&[TrieKey::Atom("x")]), Some(&Count(5))); // max(3, 5)
        assert_eq!(result.get_at(&[TrieKey::Atom("y")]), Some(&Count(7))); // only in a
        assert_eq!(result.get_at(&[TrieKey::Atom("z")]), Some(&Count(9))); // only in b
    }

    #[test]
    fn test_restrict() {
        let mut a: MettaTrie<String, Count> = MettaTrie::new();
        let mut filter: MettaTrie<String, Count> = MettaTrie::new();
        a.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(5));
        a.insert_at(&[TrieKey::Atom("y")], "y".into(), Count(3));
        a.insert_at(&[TrieKey::Atom("z")], "z".into(), Count(7));
        filter.insert_at(&[TrieKey::Atom("x")], "x".into(), Count(999));
        filter.insert_at(&[TrieKey::Atom("z")], "z".into(), Count(999));

        let result = a.restrict(&filter);
        // Only x and z are in the filter — values from a are preserved
        assert_eq!(result.val_count(), 2);
        assert_eq!(result.get_at(&[TrieKey::Atom("x")]), Some(&Count(5)));
        assert_eq!(result.get_at(&[TrieKey::Atom("z")]), Some(&Count(7)));
        assert_eq!(result.get_at(&[TrieKey::Atom("y")]), None);
    }
}
