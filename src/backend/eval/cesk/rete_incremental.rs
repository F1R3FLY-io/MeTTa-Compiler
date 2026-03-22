//! Rete-Style Incremental Matching for PLN Derivation (Phase 5.2)
//!
//! On `add-atom`, only re-evaluate affected rules instead of invalidating
//! everything. Tracks which rule groups contributed to each subgoal's result,
//! enabling selective invalidation on space mutations.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::backend::models::{MettaValue, MettaValueTrait};

// ============================================================================
// Space Mutation Events
// ============================================================================

/// A mutation event from add-atom or remove-atom.
#[derive(Debug, Clone)]
pub enum SpaceMutation {
    /// A new rule was added.
    AddRule {
        head: &'static str,
        arity: usize,
        rule_index: u32,
    },
    /// A rule was removed.
    RemoveRule {
        head: &'static str,
        arity: usize,
        rule_index: u32,
    },
    /// A fact was added to the atomspace.
    AddFact {
        head: Option<&'static str>,
        arity: usize,
    },
    /// A fact was removed from the atomspace.
    RemoveFact {
        head: Option<&'static str>,
        arity: usize,
    },
}

// ============================================================================
// Dependency Tracking
// ============================================================================

/// Which rule groups contributed to a subgoal's result.
#[derive(Debug, Clone)]
pub struct SubgoalDependency {
    /// Content hash of the subgoal expression.
    pub subgoal_hash: u64,
    /// (head, arity) rule groups that were consulted.
    pub consulted_groups: SmallVec<[(&'static str, usize); 4]>,
    /// Specific rule indices that matched.
    pub matched_rules: SmallVec<[u32; 4]>,
    /// Hashes of other subgoals this one transitively depends on.
    pub transitive_deps: SmallVec<[u64; 4]>,
}

// ============================================================================
// Incremental Index
// ============================================================================

/// Maps (head, arity) pairs to subgoal hashes that depend on them.
///
/// When a space mutation affects a (head, arity) group, all dependent
/// subgoals must be invalidated. Transitive dependencies are followed
/// to ensure completeness.
#[derive(Debug, Default)]
pub struct IncrementalIndex {
    /// (head, arity) -> set of subgoal hashes depending on this group.
    group_deps: HashMap<(&'static str, usize), HashSet<u64>>,
    /// subgoal_hash -> its dependency record.
    dep_records: HashMap<u64, SubgoalDependency>,
    /// Reverse transitive dep index: subgoal_hash -> set of subgoals that depend ON it.
    /// When subgoal X is invalidated, all entries in reverse_deps[X] must also be invalidated.
    reverse_deps: HashMap<u64, HashSet<u64>>,
}

impl IncrementalIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a subgoal depends on certain rule groups.
    pub fn record_dependency(&mut self, dep: SubgoalDependency) {
        let hash = dep.subgoal_hash;
        for &(head, arity) in &dep.consulted_groups {
            self.group_deps.entry((head, arity))
                .or_default()
                .insert(hash);
        }
        // Build reverse dependency index: for each transitive dep X,
        // record that `hash` depends on X (so invalidating X invalidates hash).
        for &trans in &dep.transitive_deps {
            self.reverse_deps.entry(trans)
                .or_default()
                .insert(hash);
        }
        self.dep_records.insert(hash, dep);
    }

    /// Compute the minimal set of subgoal hashes that must be invalidated
    /// due to a space mutation. Follows transitive dependencies.
    pub fn compute_invalidation_set(&self, mutation: &SpaceMutation) -> HashSet<u64> {
        let mut to_invalidate = HashSet::new();

        // Find directly affected subgoals
        let affected_key = match mutation {
            SpaceMutation::AddRule { head, arity, .. }
            | SpaceMutation::RemoveRule { head, arity, .. } => {
                Some((*head, *arity))
            }
            SpaceMutation::AddFact { head: Some(head), arity }
            | SpaceMutation::RemoveFact { head: Some(head), arity } => {
                Some((*head, *arity))
            }
            _ => None,
        };

        if let Some(key) = affected_key {
            if let Some(direct_deps) = self.group_deps.get(&key) {
                // BFS: start with directly affected subgoals, then follow
                // reverse dependencies (subgoals that depend on invalidated ones).
                let mut queue: Vec<u64> = direct_deps.iter().copied().collect();
                while let Some(hash) = queue.pop() {
                    if to_invalidate.insert(hash) {
                        // Follow reverse dependencies: anything that depends ON this
                        // subgoal must also be invalidated.
                        if let Some(dependents) = self.reverse_deps.get(&hash) {
                            for &dependent in dependents {
                                if !to_invalidate.contains(&dependent) {
                                    queue.push(dependent);
                                }
                            }
                        }
                    }
                }
            }
        }

        to_invalidate
    }

    /// Selectively invalidate entries in a SubgoalTable.
    ///
    /// Returns the number of entries invalidated.
    pub fn selective_invalidate(
        &self,
        table: &mut super::tabling::SubgoalTable<MettaValue>,
        mutation: &SpaceMutation,
    ) -> usize {
        let to_invalidate = self.compute_invalidation_set(mutation);
        let mut count = 0;
        for hash in &to_invalidate {
            table.abandon(*hash);
            count += 1;
        }
        count
    }

    /// Clear all dependency records.
    pub fn clear(&mut self) {
        self.group_deps.clear();
        self.dep_records.clear();
        self.reverse_deps.clear();
    }

    /// Number of tracked subgoals.
    pub fn tracked_subgoals(&self) -> usize {
        self.dep_records.len()
    }

    /// Number of tracked (head, arity) groups.
    pub fn tracked_groups(&self) -> usize {
        self.group_deps.len()
    }
}

// ============================================================================
// Thread-Local Incremental Index
// ============================================================================

use std::cell::RefCell;

thread_local! {
    /// Thread-local incremental index for tracking subgoal dependencies.
    /// Consulted during rule matching to record which groups contributed.
    static THREAD_INCREMENTAL_INDEX: RefCell<IncrementalIndex> = RefCell::new(IncrementalIndex::new());
}

/// Access the thread-local incremental index.
#[inline]
pub fn with_incremental_index<R>(f: impl FnOnce(&mut IncrementalIndex) -> R) -> R {
    THREAD_INCREMENTAL_INDEX.with(|cell| {
        let mut index = cell.borrow_mut();
        f(&mut index)
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_index() {
        let index = IncrementalIndex::new();
        assert_eq!(index.tracked_subgoals(), 0);
        assert_eq!(index.tracked_groups(), 0);
    }

    #[test]
    fn test_record_dependency() {
        let mut index = IncrementalIndex::new();
        index.record_dependency(SubgoalDependency {
            subgoal_hash: 100,
            consulted_groups: SmallVec::from_slice(&[("f", 2), ("g", 3)]),
            matched_rules: SmallVec::from_slice(&[0, 1]),
            transitive_deps: SmallVec::new(),
        });

        assert_eq!(index.tracked_subgoals(), 1);
        assert_eq!(index.tracked_groups(), 2);
    }

    #[test]
    fn test_direct_invalidation() {
        let mut index = IncrementalIndex::new();
        index.record_dependency(SubgoalDependency {
            subgoal_hash: 100,
            consulted_groups: SmallVec::from_slice(&[("f", 2)]),
            matched_rules: SmallVec::from_slice(&[0]),
            transitive_deps: SmallVec::new(),
        });
        index.record_dependency(SubgoalDependency {
            subgoal_hash: 200,
            consulted_groups: SmallVec::from_slice(&[("g", 3)]),
            matched_rules: SmallVec::from_slice(&[1]),
            transitive_deps: SmallVec::new(),
        });

        // Add rule to (f, 2) → only subgoal 100 invalidated
        let mutation = SpaceMutation::AddRule { head: "f", arity: 2, rule_index: 5 };
        let invalidated = index.compute_invalidation_set(&mutation);
        assert!(invalidated.contains(&100));
        assert!(!invalidated.contains(&200));
    }

    #[test]
    fn test_transitive_invalidation() {
        let mut index = IncrementalIndex::new();

        // Subgoal 100 depends on (f, 2)
        index.record_dependency(SubgoalDependency {
            subgoal_hash: 100,
            consulted_groups: SmallVec::from_slice(&[("f", 2)]),
            matched_rules: SmallVec::from_slice(&[0]),
            transitive_deps: SmallVec::new(),
        });

        // Subgoal 200 depends on (g, 3) and transitively on subgoal 100
        index.record_dependency(SubgoalDependency {
            subgoal_hash: 200,
            consulted_groups: SmallVec::from_slice(&[("g", 3)]),
            matched_rules: SmallVec::from_slice(&[1]),
            transitive_deps: SmallVec::from_slice(&[100]),
        });

        // Mutating (f, 2) invalidates 100, which transitively invalidates 200
        let mutation = SpaceMutation::AddRule { head: "f", arity: 2, rule_index: 5 };
        let invalidated = index.compute_invalidation_set(&mutation);
        assert!(invalidated.contains(&100));
        assert!(invalidated.contains(&200));
    }

    #[test]
    fn test_no_invalidation_for_unrelated_mutation() {
        let mut index = IncrementalIndex::new();
        index.record_dependency(SubgoalDependency {
            subgoal_hash: 100,
            consulted_groups: SmallVec::from_slice(&[("f", 2)]),
            matched_rules: SmallVec::from_slice(&[0]),
            transitive_deps: SmallVec::new(),
        });

        // Unrelated mutation
        let mutation = SpaceMutation::AddRule { head: "h", arity: 1, rule_index: 10 };
        let invalidated = index.compute_invalidation_set(&mutation);
        assert!(invalidated.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut index = IncrementalIndex::new();
        index.record_dependency(SubgoalDependency {
            subgoal_hash: 100,
            consulted_groups: SmallVec::from_slice(&[("f", 2)]),
            matched_rules: SmallVec::from_slice(&[0]),
            transitive_deps: SmallVec::new(),
        });
        index.clear();
        assert_eq!(index.tracked_subgoals(), 0);
    }
}
