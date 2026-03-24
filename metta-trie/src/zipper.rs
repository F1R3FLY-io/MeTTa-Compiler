//! Focused Zipper API for MettaTrie
//!
//! Provides cursor-based navigation for efficient trie traversal without
//! requiring full key path decomposition on each operation.
//!
//! ## Implemented Methods (focused subset)
//!
//! **ReadZipper**: `descend`, `ascend`, `val`, `expr`, `entry`, `to_next_val`,
//!                 `path_exists`, `is_val`, `child_count`, `at_root`, `reset`
//!
//! **WriteZipper**: `set_val`, `remove_val`
//!
//! Additional methods can be added incrementally as needed.

use std::sync::Arc;

use crate::keys::TrieKey;
use crate::node::{MettaTrie, MettaTrieNode};

/// A read-only cursor into a MettaTrie.
///
/// Supports two usage modes:
/// 1. **Manual navigation**: `descend()`/`ascend()` for targeted access
/// 2. **DFS iteration**: `to_next_val()` for iterating all entries
///
/// The DFS iteration state is maintained via an explicit stack of
/// `(node, sorted_child_keys, next_child_index)` frames.
pub struct ReadZipper<'a, E, V> {
    /// The root node reference.
    root: &'a MettaTrieNode<E, V>,
    /// Manual navigation stack: `(key_used, parent_node)`.
    path: Vec<(&'a TrieKey, &'a MettaTrieNode<E, V>)>,
    /// Current node (for manual navigation and `val()`/`expr()` access).
    current: &'a MettaTrieNode<E, V>,
    /// DFS iteration stack: `(node, sorted_child_keys, next_child_index)`.
    /// Separate from `path` because DFS iteration needs to track which
    /// children have been visited at each level.
    dfs_stack: Vec<(&'a MettaTrieNode<E, V>, Vec<&'a TrieKey>, usize)>,
    /// Whether DFS iteration has been initialized.
    dfs_initialized: bool,
}

impl<'a, E, V> ReadZipper<'a, E, V> {
    /// Create a new ReadZipper at the root of the trie.
    pub fn new(trie: &'a MettaTrie<E, V>) -> Self {
        Self {
            root: trie.root(),
            path: Vec::with_capacity(8),
            current: trie.root(),
            dfs_stack: Vec::new(),
            dfs_initialized: false,
        }
    }

    /// Descend to a child by key. Returns `true` if the child exists.
    ///
    /// Resets DFS iteration state (manual navigation and DFS are independent modes).
    pub fn descend(&mut self, key: &TrieKey) -> bool {
        if let Some((stored_key, child_arc)) = self.current.children.get_key_value(key) {
            self.path.push((stored_key, self.current));
            self.current = child_arc;
            self.dfs_initialized = false;
            true
        } else {
            false
        }
    }

    /// Descend along a full key path. Returns `true` if all keys exist.
    pub fn descend_path(&mut self, keys: &[TrieKey]) -> bool {
        for key in keys {
            if !self.descend(key) {
                return false;
            }
        }
        true
    }

    /// Ascend to the parent. Returns `true` if not already at root.
    pub fn ascend(&mut self) -> bool {
        if let Some((_, parent)) = self.path.pop() {
            self.current = parent;
            self.dfs_initialized = false;
            true
        } else {
            false
        }
    }

    /// Check if the current node has an entry.
    #[inline]
    pub fn is_val(&self) -> bool {
        self.current.has_entry()
    }

    /// Check if a path exists at the current node (i.e., the node is reachable).
    #[inline]
    pub fn path_exists(&self) -> bool {
        true // If we're here, the node exists
    }

    /// Get the value at the current node.
    #[inline]
    pub fn val(&self) -> Option<&'a V> {
        self.current.entry.as_ref().map(|(_, v)| v)
    }

    /// Get the expression at the current node.
    #[inline]
    pub fn expr(&self) -> Option<&'a E> {
        self.current.entry.as_ref().map(|(e, _)| e)
    }

    /// Get the full entry (expression + value) at the current node.
    #[inline]
    pub fn entry(&self) -> Option<(&'a E, &'a V)> {
        self.current.entry.as_ref().map(|(e, v)| (e, v))
    }

    /// Number of children at the current node.
    #[inline]
    pub fn child_count(&self) -> usize {
        self.current.child_count()
    }

    /// Check if at the root.
    #[inline]
    pub fn at_root(&self) -> bool {
        self.path.is_empty()
    }

    /// Reset to the root. Also resets DFS iteration state.
    pub fn reset(&mut self) {
        self.path.clear();
        self.current = self.root;
        self.dfs_stack.clear();
        self.dfs_initialized = false;
    }

    /// Depth-first advance to the next node with a value.
    ///
    /// Returns `true` if a next value was found, `false` if iteration is complete.
    /// After returning `true`, use `val()`, `expr()`, or `entry()` to read.
    ///
    /// Uses an explicit DFS stack to correctly iterate all entries across
    /// repeated calls. The stack tracks `(node, child_keys, next_child_idx)`
    /// at each level.
    pub fn to_next_val(&mut self) -> bool {
        // Initialize DFS stack on first call
        if !self.dfs_initialized {
            self.dfs_stack.clear();
            let child_keys: Vec<&'a TrieKey> = self.current.children.key_refs();
            self.dfs_stack.push((self.current, child_keys, 0));
            self.dfs_initialized = true;

            // Check if root itself has an entry
            if self.current.has_entry() {
                return true;
            }
        }

        // Iterative DFS
        while let Some(frame) = self.dfs_stack.last_mut() {
            let (node, ref child_keys, ref mut next_idx) = *frame;

            if *next_idx < child_keys.len() {
                let key = child_keys[*next_idx];
                *next_idx += 1;

                if let Some(child) = node.children.get(key) {
                    let child_ref: &'a MettaTrieNode<E, V> = child;
                    let grandchild_keys: Vec<&'a TrieKey> = child_ref.children.key_refs();
                    self.dfs_stack.push((child_ref, grandchild_keys, 0));

                    if child_ref.has_entry() {
                        self.current = child_ref;
                        return true;
                    }
                }
            } else {
                // All children at this level visited — backtrack
                self.dfs_stack.pop();
            }
        }

        false
    }

    /// Get a reference to the current node.
    #[inline]
    pub fn current_node(&self) -> &'a MettaTrieNode<E, V> {
        self.current
    }
}

impl<E: Clone, V: Clone> MettaTrie<E, V> {
    /// Create a read zipper positioned at the root.
    pub fn read_zipper(&self) -> ReadZipper<'_, E, V> {
        ReadZipper::new(self)
    }

    /// Set the value at the given key path, creating nodes as needed.
    ///
    /// This is the write-zipper equivalent of `set_val` — a convenience method
    /// that navigates and sets in one call. Returns the previous value if any.
    pub fn set_val_at(&mut self, keys: &[TrieKey], expr: E, value: V) -> Option<V> {
        self.insert_at(keys, expr, value)
    }

    /// Remove the value at the given key path, pruning empty nodes.
    ///
    /// This is the write-zipper equivalent of `remove_val`. Returns the removed
    /// entry if it existed.
    pub fn remove_val_at(&mut self, keys: &[TrieKey]) -> Option<(E, V)> {
        self.remove_at(keys)
    }

    /// Graft: copy an entire subtrie from `source` at `source_keys` into `self`
    /// at `target_keys`.
    ///
    /// All entries under the source path are inserted under the target path.
    /// Existing entries at overlapping paths in the target are overwritten.
    pub fn graft(&mut self, target_keys: &[TrieKey], source: &Self, source_keys: &[TrieKey]) {
        if let Some(source_node) = source.navigate_to(source_keys) {
            let mut added = 0usize;
            let target_node = self.navigate_to_mut(target_keys);
            Self::graft_recursive(target_node, source_node, &mut added);
            self.val_count += added;
        }
    }

    fn graft_recursive(
        target: &mut MettaTrieNode<E, V>,
        source: &MettaTrieNode<E, V>,
        added: &mut usize,
    ) {
        // Copy entry
        if let Some(entry) = &source.entry {
            if target.entry.is_none() {
                *added += 1;
            }
            target.entry = Some(entry.clone());
        }

        // Copy children recursively
        for (key, src_child) in source.children.iter() {
            let tgt_child = target
                .children
                .entry_or_insert(key.clone(), || Arc::new(MettaTrieNode::new()));
            Self::graft_recursive(Arc::make_mut(tgt_child), src_child, added);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_zipper_descend_ascend() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Long(42)],
            "(f 42)".to_string(),
            1,
        );

        let mut rz = trie.read_zipper();
        assert!(rz.at_root());
        assert!(!rz.is_val());

        // Descend to Arity(2)
        assert!(rz.descend(&TrieKey::Arity(2)));
        assert!(!rz.at_root());
        assert!(!rz.is_val());

        // Descend to Atom("f")
        assert!(rz.descend(&TrieKey::Atom("f")));
        assert!(!rz.is_val());

        // Descend to Long(42) — has value
        assert!(rz.descend(&TrieKey::Long(42)));
        assert!(rz.is_val());
        assert_eq!(rz.val(), Some(&1));
        assert_eq!(rz.expr(), Some(&"(f 42)".to_string()));

        // Ascend back
        assert!(rz.ascend());
        assert!(rz.ascend());
        assert!(rz.ascend());
        assert!(rz.at_root());
        assert!(!rz.ascend()); // Can't go above root
    }

    #[test]
    fn test_read_zipper_descend_path() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Long(42)],
            "(f 42)".to_string(),
            1,
        );

        let mut rz = trie.read_zipper();
        assert!(rz.descend_path(&[
            TrieKey::Arity(2),
            TrieKey::Atom("f"),
            TrieKey::Long(42)
        ]));
        assert_eq!(rz.val(), Some(&1));
    }

    #[test]
    fn test_read_zipper_nonexistent_path() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(&[TrieKey::Atom("f")], "f".to_string(), 1);

        let mut rz = trie.read_zipper();
        assert!(!rz.descend(&TrieKey::Atom("g"))); // No "g" edge
        assert!(rz.at_root()); // Still at root after failed descend
    }

    #[test]
    fn test_read_zipper_reset() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(&[TrieKey::Atom("f"), TrieKey::Atom("x")], "fx".to_string(), 1);

        let mut rz = trie.read_zipper();
        rz.descend(&TrieKey::Atom("f"));
        assert!(!rz.at_root());

        rz.reset();
        assert!(rz.at_root());
    }

    #[test]
    fn test_read_zipper_to_next_val_iterates_all() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(&[TrieKey::Atom("a")], "a".to_string(), 1);
        trie.insert_at(&[TrieKey::Atom("b")], "b".to_string(), 2);
        trie.insert_at(&[TrieKey::Atom("c")], "c".to_string(), 3);

        let mut rz = trie.read_zipper();
        let mut values = Vec::new();
        while rz.to_next_val() {
            values.push(*rz.val().expect("has value"));
        }
        values.sort();
        assert_eq!(values, vec![1, 2, 3]);
    }

    #[test]
    fn test_read_zipper_to_next_val_nested() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(&[TrieKey::Atom("a")], "a".to_string(), 1);
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("x")],
            "(f x)".to_string(),
            2,
        );
        trie.insert_at(
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("y")],
            "(f y)".to_string(),
            3,
        );

        let mut rz = trie.read_zipper();
        let mut values = Vec::new();
        while rz.to_next_val() {
            values.push(*rz.val().expect("has value"));
        }
        values.sort();
        assert_eq!(values, vec![1, 2, 3]);
    }

    #[test]
    fn test_read_zipper_to_next_val_empty_trie() {
        let trie: MettaTrie<String, u64> = MettaTrie::new();
        let mut rz = trie.read_zipper();
        assert!(!rz.to_next_val());
    }

    #[test]
    fn test_read_zipper_child_count() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(&[TrieKey::Atom("a")], "a".to_string(), 1);
        trie.insert_at(&[TrieKey::Atom("b")], "b".to_string(), 2);
        trie.insert_at(&[TrieKey::Atom("c")], "c".to_string(), 3);

        let rz = trie.read_zipper();
        assert_eq!(rz.child_count(), 3);
    }

    // ── Write operation tests ──────────────────────────────────────────

    #[test]
    fn test_set_val_at() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        assert!(trie.set_val_at(&[TrieKey::Atom("x")], "x".to_string(), 1).is_none());
        assert_eq!(trie.get_at(&[TrieKey::Atom("x")]), Some(&1));

        // Overwrite
        let old = trie.set_val_at(&[TrieKey::Atom("x")], "x".to_string(), 42);
        assert_eq!(old, Some(1));
        assert_eq!(trie.get_at(&[TrieKey::Atom("x")]), Some(&42));
    }

    #[test]
    fn test_remove_val_at() {
        let mut trie: MettaTrie<String, u64> = MettaTrie::new();
        trie.insert_at(&[TrieKey::Atom("x")], "x".to_string(), 1);

        let removed = trie.remove_val_at(&[TrieKey::Atom("x")]);
        assert_eq!(removed, Some(("x".to_string(), 1)));
        assert!(trie.is_empty());

        // Remove non-existent
        assert!(trie.remove_val_at(&[TrieKey::Atom("x")]).is_none());
    }

    #[test]
    fn test_graft() {
        let mut source: MettaTrie<String, u64> = MettaTrie::new();
        source.insert_at(
            &[TrieKey::Atom("a")],
            "a".to_string(),
            1,
        );
        source.insert_at(
            &[TrieKey::Atom("b")],
            "b".to_string(),
            2,
        );

        let mut target: MettaTrie<String, u64> = MettaTrie::new();
        target.insert_at(&[TrieKey::Atom("existing")], "existing".to_string(), 99);

        // Graft source's root subtree into target at Arity(2)/Atom("f")
        target.graft(
            &[TrieKey::Arity(2), TrieKey::Atom("f")],
            &source,
            &[], // source root
        );

        assert_eq!(target.val_count(), 3);
        assert_eq!(target.get_at(&[TrieKey::Atom("existing")]), Some(&99));
        assert_eq!(
            target.get_at(&[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("a")]),
            Some(&1)
        );
        assert_eq!(
            target.get_at(&[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Atom("b")]),
            Some(&2)
        );
    }
}
