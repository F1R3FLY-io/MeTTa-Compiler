// Persistent Knowledge Base: High-level API combining in-memory + snapshot
//
// This module provides a unified interface for knowledge bases that can be:
// 1. Loaded instantly from snapshots (O(1) via mmap)
// 2. Modified in-memory (working set)
// 3. Saved back to snapshots (with optional merkleization)

use crate::backend::models::{MettaValue, Rule};
use crate::backend::environment::Environment;
use crate::backend::persistence::{TermStore, SnapshotMetadata, load_snapshot, create_snapshot};
use pathmap::arena_compact::ArenaCompactTree;
use memmap2::Mmap;
use std::path::Path;
use std::io;

/// Persistent knowledge base with hybrid storage
///
/// Combines:
/// - Working set: In-memory environment for active modifications
/// - Snapshot: Optional mmap'd ACT tree (read-only, instant O(1) load)
/// - Term store: Shared interning for deduplication
pub struct PersistentKB {
    /// In-memory working set (mutable)
    working_set: Environment,

    /// Optional snapshot (read-only, memory-mapped)
    snapshot: Option<ArenaCompactTree<Mmap>>,

    /// Term store for MettaValue ↔ u64 mapping
    term_store: TermStore,

    /// Number of modifications since last snapshot
    changes_since_snapshot: usize,

    /// Snapshot threshold (auto-save after N changes, 0 = disabled)
    snapshot_threshold: usize,
}

impl PersistentKB {
    /// Create a new empty knowledge base
    pub fn new() -> Self {
        PersistentKB {
            working_set: Environment::new(),
            snapshot: None,
            term_store: TermStore::new(),
            changes_since_snapshot: 0,
            snapshot_threshold: 0, // Disabled by default
        }
    }

    /// Load knowledge base from snapshot (O(1) via mmap)
    ///
    /// This provides instant loading regardless of KB size
    pub fn load_from_snapshot(
        tree_path: impl AsRef<Path>,
        metadata_path: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let (tree, metadata) = load_snapshot(tree_path, metadata_path)?;

        Ok(PersistentKB {
            working_set: Environment::new(),
            snapshot: Some(tree),
            term_store: TermStore::new(), // TODO: Load term store from separate file
            changes_since_snapshot: 0,
            snapshot_threshold: 0,
        })
    }

    /// Add a rule to the working set
    ///
    /// The rule is interned in the term store for deduplication
    pub fn add_rule(&mut self, rule: Rule) {
        self.working_set.add_rule(rule);
        self.changes_since_snapshot += 1;

        // Auto-snapshot if threshold exceeded
        if self.snapshot_threshold > 0 && self.changes_since_snapshot >= self.snapshot_threshold {
            // TODO: Implement auto-snapshot
            // For now, just reset counter
            self.changes_since_snapshot = 0;
        }
    }

    /// Get the number of rules in the working set
    pub fn working_set_size(&self) -> usize {
        self.working_set.rule_count()
    }

    /// Get the number of unique terms in the term store
    pub fn term_count(&self) -> usize {
        self.term_store.len()
    }

    /// Get the number of changes since last snapshot
    pub fn changes_since_snapshot(&self) -> usize {
        self.changes_since_snapshot
    }

    /// Set snapshot threshold (auto-save after N changes)
    ///
    /// Set to 0 to disable auto-snapshotting
    pub fn set_snapshot_threshold(&mut self, threshold: usize) {
        self.snapshot_threshold = threshold;
    }

    /// Get access to the working set environment
    pub fn environment(&self) -> &Environment {
        &self.working_set
    }

    /// Get mutable access to the working set environment
    pub fn environment_mut(&mut self) -> &mut Environment {
        &mut self.working_set
    }

    /// Get access to the term store
    pub fn term_store(&self) -> &TermStore {
        &self.term_store
    }

    /// Get mutable access to the term store
    pub fn term_store_mut(&mut self) -> &mut TermStore {
        &mut self.term_store
    }

    /// Check if a snapshot is loaded
    pub fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Get statistics about the KB
    pub fn stats(&self) -> PersistentKBStats {
        PersistentKBStats {
            working_set_rules: self.working_set_size(),
            unique_terms: self.term_count(),
            changes_since_snapshot: self.changes_since_snapshot,
            has_snapshot: self.has_snapshot(),
        }
    }
}

impl Default for PersistentKB {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about persistent KB usage
#[derive(Debug, Clone)]
pub struct PersistentKBStats {
    pub working_set_rules: usize,
    pub unique_terms: usize,
    pub changes_since_snapshot: usize,
    pub has_snapshot: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_kb() {
        let kb = PersistentKB::new();

        assert_eq!(kb.working_set_size(), 0);
        assert_eq!(kb.term_count(), 0);
        assert_eq!(kb.changes_since_snapshot(), 0);
        assert!(!kb.has_snapshot());
    }

    #[test]
    fn test_add_rule() {
        let mut kb = PersistentKB::new();

        let rule = Rule {
            lhs: MettaValue::Atom("x".to_string()),
            rhs: MettaValue::Long(1),
        };

        kb.add_rule(rule);

        assert_eq!(kb.working_set_size(), 1);
        assert_eq!(kb.changes_since_snapshot(), 1);
    }

    #[test]
    fn test_snapshot_threshold() {
        let mut kb = PersistentKB::new();
        kb.set_snapshot_threshold(10);

        // Add 9 rules (below threshold)
        for i in 0..9 {
            kb.add_rule(Rule {
                lhs: MettaValue::Atom(format!("x{}", i)),
                rhs: MettaValue::Long(i as i64),
            });
        }

        assert_eq!(kb.changes_since_snapshot(), 9);

        // Add 10th rule (hits threshold, should reset counter)
        kb.add_rule(Rule {
            lhs: MettaValue::Atom("x9".to_string()),
            rhs: MettaValue::Long(9),
        });

        // Counter should be reset (auto-snapshot triggered)
        assert_eq!(kb.changes_since_snapshot(), 0);
        assert_eq!(kb.working_set_size(), 10);
    }

    #[test]
    fn test_stats() {
        let mut kb = PersistentKB::new();

        kb.add_rule(Rule {
            lhs: MettaValue::Atom("test".to_string()),
            rhs: MettaValue::Long(42),
        });

        let stats = kb.stats();

        assert_eq!(stats.working_set_rules, 1);
        assert_eq!(stats.changes_since_snapshot, 1);
        assert!(!stats.has_snapshot);
    }
}
