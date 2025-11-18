// PathMap Algebraic Operations - MeTTaTron Integration Examples
//
// Purpose: Practical examples of PathMap algebraic operations for MeTTaTron
// Reference: PATHMAP_ALGEBRAIC_OPERATIONS.md Section 4
//
// Usage:
//   1. Copy relevant examples into your MeTTaTron source
//   2. Adapt to your specific Space/Expression types
//   3. Use as templates for implementing MeTTa reasoning patterns
//
// Examples Include:
//   1. Knowledge Base Merging (join)
//   2. Query Scoping (restrict)
//   3. Differential Updates (subtract)
//   4. Consistency Checking (meet)
//   5. Multi-Space Reasoning (multi-way join)
//   6. Incremental Reasoning (zipper operations)
//   7. Version Control Integration
//   8. Provenance Tracking
//   9. Distributed Knowledge Synchronization
//
// Note: These examples use simplified types. Adapt to your actual MeTTa AST.

#![allow(dead_code)]

use pathmap::{PathMap, WriteZipper};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

// ============================================================================
// Example 1: Knowledge Base Merging
// ============================================================================

/// Represents a MeTTa expression (simplified for examples)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MettaExpr {
    Atom(String),
    Symbol(String),
    List(Vec<MettaExpr>),
    Number(i64),
}

/// Metadata attached to expressions in the knowledge base
#[derive(Debug, Clone)]
pub struct ExprMetadata {
    pub source: String,
    pub confidence: f64,
    pub timestamp: u64,
}

/// Knowledge base entry with expression and metadata
#[derive(Debug, Clone)]
pub struct KnowledgeEntry {
    pub expr: MettaExpr,
    pub metadata: ExprMetadata,
}

/// Merge two knowledge bases, preferring higher confidence values
pub fn merge_knowledge_bases(
    kb1: &PathMap<KnowledgeEntry>,
    kb2: &PathMap<KnowledgeEntry>,
) -> PathMap<KnowledgeEntry> {
    // Custom lattice implementation: prefer higher confidence
    // For this example, we use the built-in join which takes the second value
    // In practice, you'd implement Lattice trait for KnowledgeEntry

    kb1.join(kb2)
}

/// Example: Merging module knowledge bases in MeTTaTron
pub fn example_module_merge() {
    let mut base_kb = PathMap::new();
    let mut module_kb = PathMap::new();

    // Base knowledge base
    base_kb.insert(
        b"facts/math/addition",
        KnowledgeEntry {
            expr: MettaExpr::List(vec![
                MettaExpr::Symbol("=".to_string()),
                MettaExpr::List(vec![
                    MettaExpr::Symbol("+".to_string()),
                    MettaExpr::Number(2),
                    MettaExpr::Number(2),
                ]),
                MettaExpr::Number(4),
            ]),
            metadata: ExprMetadata {
                source: "stdlib".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    // Module knowledge base
    module_kb.insert(
        b"facts/math/multiplication",
        KnowledgeEntry {
            expr: MettaExpr::List(vec![
                MettaExpr::Symbol("=".to_string()),
                MettaExpr::List(vec![
                    MettaExpr::Symbol("*".to_string()),
                    MettaExpr::Number(3),
                    MettaExpr::Number(4),
                ]),
                MettaExpr::Number(12),
            ]),
            metadata: ExprMetadata {
                source: "user_module".to_string(),
                confidence: 0.95,
                timestamp: 2000,
            },
        },
    );

    // Merge knowledge bases
    let merged = merge_knowledge_bases(&base_kb, &module_kb);

    println!("Merged knowledge base has {} entries", merged.len());
}

// ============================================================================
// Example 2: Query Scoping with Restrict
// ============================================================================

/// Query a specific namespace in the knowledge base
pub fn query_namespace(kb: &PathMap<KnowledgeEntry>, namespace: &[u8]) -> PathMap<KnowledgeEntry> {
    kb.restrict(namespace)
}

/// Example: Scoped query execution
pub fn example_scoped_query() {
    let mut kb = PathMap::new();

    // Populate with facts in different namespaces
    kb.insert(
        b"facts/math/arithmetic/addition",
        KnowledgeEntry {
            expr: MettaExpr::Atom("add_rule".to_string()),
            metadata: ExprMetadata {
                source: "stdlib".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    kb.insert(
        b"facts/math/geometry/area",
        KnowledgeEntry {
            expr: MettaExpr::Atom("area_rule".to_string()),
            metadata: ExprMetadata {
                source: "stdlib".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    kb.insert(
        b"facts/logic/propositional/modus_ponens",
        KnowledgeEntry {
            expr: MettaExpr::Atom("mp_rule".to_string()),
            metadata: ExprMetadata {
                source: "stdlib".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    // Query only math facts
    let math_facts = query_namespace(&kb, b"facts/math/");
    println!("Math facts: {} entries", math_facts.len()); // 2 entries

    // Query only arithmetic facts
    let arithmetic_facts = query_namespace(&kb, b"facts/math/arithmetic/");
    println!("Arithmetic facts: {} entries", arithmetic_facts.len()); // 1 entry
}

// ============================================================================
// Example 3: Differential Updates with Subtract
// ============================================================================

/// Compute differential update between two knowledge base states
pub fn compute_diff(
    old_kb: &PathMap<KnowledgeEntry>,
    new_kb: &PathMap<KnowledgeEntry>,
) -> (PathMap<KnowledgeEntry>, PathMap<KnowledgeEntry>) {
    // Added entries: in new but not in old
    let added = new_kb.subtract(old_kb);

    // Removed entries: in old but not in new
    let removed = old_kb.subtract(new_kb);

    (added, removed)
}

/// Apply a differential update to a knowledge base
pub fn apply_diff(
    base: &PathMap<KnowledgeEntry>,
    added: &PathMap<KnowledgeEntry>,
    removed: &PathMap<KnowledgeEntry>,
) -> PathMap<KnowledgeEntry> {
    // Remove deleted entries, then add new ones
    base.subtract(removed).join(added)
}

/// Example: Version control for knowledge bases
pub fn example_version_control() {
    let mut v1 = PathMap::new();
    v1.insert(
        b"fact/1",
        KnowledgeEntry {
            expr: MettaExpr::Atom("old_fact".to_string()),
            metadata: ExprMetadata {
                source: "v1".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    let mut v2 = v1.clone();
    v2.insert(
        b"fact/2",
        KnowledgeEntry {
            expr: MettaExpr::Atom("new_fact".to_string()),
            metadata: ExprMetadata {
                source: "v2".to_string(),
                confidence: 1.0,
                timestamp: 2000,
            },
        },
    );

    // Compute diff
    let (added, removed) = compute_diff(&v1, &v2);
    println!("Added: {} entries", added.len());
    println!("Removed: {} entries", removed.len());

    // Apply diff to v1 to get v2
    let reconstructed = apply_diff(&v1, &added, &removed);
    assert_eq!(reconstructed.len(), v2.len());
}

// ============================================================================
// Example 4: Consistency Checking with Meet
// ============================================================================

/// Check consistency between two reasoning contexts
pub fn check_consistency(
    context1: &PathMap<KnowledgeEntry>,
    context2: &PathMap<KnowledgeEntry>,
) -> PathMap<KnowledgeEntry> {
    // Intersection contains only shared knowledge
    context1.meet(context2)
}

/// Verify that a derived context is consistent with axioms
pub fn verify_derivation(
    axioms: &PathMap<KnowledgeEntry>,
    derived: &PathMap<KnowledgeEntry>,
) -> bool {
    // All derived facts should be consistent with axioms
    let intersection = axioms.meet(derived);

    // If intersection equals derived, all derived facts are in axioms
    intersection.len() == derived.len()
}

/// Example: Proof validation
pub fn example_proof_validation() {
    let mut axioms = PathMap::new();
    axioms.insert(
        b"axiom/1",
        KnowledgeEntry {
            expr: MettaExpr::Atom("axiom_1".to_string()),
            metadata: ExprMetadata {
                source: "axioms".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    let mut proof_step = PathMap::new();
    proof_step.insert(
        b"axiom/1",
        KnowledgeEntry {
            expr: MettaExpr::Atom("axiom_1".to_string()),
            metadata: ExprMetadata {
                source: "proof".to_string(),
                confidence: 1.0,
                timestamp: 2000,
            },
        },
    );

    let is_valid = verify_derivation(&axioms, &proof_step);
    println!("Proof step valid: {}", is_valid);
}

// ============================================================================
// Example 5: Multi-Space Reasoning
// ============================================================================

/// Merge multiple reasoning spaces
pub fn merge_spaces(spaces: &[PathMap<KnowledgeEntry>]) -> PathMap<KnowledgeEntry> {
    if spaces.is_empty() {
        return PathMap::new();
    }

    let mut result = spaces[0].clone();
    for space in &spaces[1..] {
        result = result.join(space);
    }

    result
}

/// Parallel merge using Rayon
pub fn merge_spaces_parallel(spaces: &[PathMap<KnowledgeEntry>]) -> PathMap<KnowledgeEntry> {
    use rayon::prelude::*;

    spaces
        .par_iter()
        .cloned()
        .reduce(|| PathMap::new(), |acc, space| acc.join(&space))
}

/// Example: Merging agent knowledge
pub fn example_multi_agent_merge() {
    // Simulate 3 agents with their own knowledge
    let agent1 = {
        let mut kb = PathMap::new();
        kb.insert(
            b"agent1/belief/1",
            KnowledgeEntry {
                expr: MettaExpr::Atom("agent1_belief".to_string()),
                metadata: ExprMetadata {
                    source: "agent1".to_string(),
                    confidence: 0.9,
                    timestamp: 1000,
                },
            },
        );
        kb
    };

    let agent2 = {
        let mut kb = PathMap::new();
        kb.insert(
            b"agent2/belief/1",
            KnowledgeEntry {
                expr: MettaExpr::Atom("agent2_belief".to_string()),
                metadata: ExprMetadata {
                    source: "agent2".to_string(),
                    confidence: 0.85,
                    timestamp: 1500,
                },
            },
        );
        kb
    };

    let agent3 = {
        let mut kb = PathMap::new();
        kb.insert(
            b"agent3/belief/1",
            KnowledgeEntry {
                expr: MettaExpr::Atom("agent3_belief".to_string()),
                metadata: ExprMetadata {
                    source: "agent3".to_string(),
                    confidence: 0.95,
                    timestamp: 2000,
                },
            },
        );
        kb
    };

    let spaces = vec![agent1, agent2, agent3];
    let merged = merge_spaces_parallel(&spaces);

    println!("Merged {} agent spaces into {} entries", spaces.len(), merged.len());
}

// ============================================================================
// Example 6: Incremental Reasoning with Zippers
// ============================================================================

/// Incrementally update a specific reasoning context
pub fn incremental_update(
    kb: &mut PathMap<KnowledgeEntry>,
    context_path: &[u8],
    updates: &PathMap<KnowledgeEntry>,
) -> Result<(), String> {
    // Use zipper to navigate to context and update in-place
    let mut zipper = WriteZipper::from_root(kb);

    match zipper.descend(context_path) {
        Ok(mut context_zipper) => {
            context_zipper.join_mut(updates);
            Ok(())
        }
        Err(e) => Err(format!("Failed to navigate to context: {:?}", e)),
    }
}

/// Example: Incremental reasoning in a specific module
pub fn example_incremental_reasoning() {
    let mut kb = PathMap::new();

    // Initialize with base knowledge
    kb.insert(
        b"module/math/fact1",
        KnowledgeEntry {
            expr: MettaExpr::Atom("base_fact".to_string()),
            metadata: ExprMetadata {
                source: "base".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    // Incremental update to math module
    let mut updates = PathMap::new();
    updates.insert(
        b"fact2",
        KnowledgeEntry {
            expr: MettaExpr::Atom("new_fact".to_string()),
            metadata: ExprMetadata {
                source: "reasoning".to_string(),
                confidence: 0.95,
                timestamp: 2000,
            },
        },
    );

    match incremental_update(&mut kb, b"module/math/", &updates) {
        Ok(_) => println!("Incremental update successful"),
        Err(e) => println!("Update failed: {}", e),
    }
}

// ============================================================================
// Example 7: Provenance Tracking
// ============================================================================

/// Track provenance using HashSet lattice
pub type ProvenanceSet = HashSet<String>;

/// Knowledge entry with provenance
#[derive(Debug, Clone)]
pub struct ProvenanceEntry {
    pub expr: MettaExpr,
    pub sources: ProvenanceSet,
}

/// Merge knowledge with provenance tracking
pub fn merge_with_provenance(
    kb1: &PathMap<ProvenanceEntry>,
    kb2: &PathMap<ProvenanceEntry>,
) -> PathMap<ProvenanceEntry> {
    // HashSet implements Lattice via union
    kb1.join(kb2)
}

/// Example: Tracking inference sources
pub fn example_provenance_tracking() {
    let mut kb1 = PathMap::new();
    let mut sources1 = HashSet::new();
    sources1.insert("axiom_a".to_string());

    kb1.insert(
        b"fact/1",
        ProvenanceEntry {
            expr: MettaExpr::Atom("fact_1".to_string()),
            sources: sources1,
        },
    );

    let mut kb2 = PathMap::new();
    let mut sources2 = HashSet::new();
    sources2.insert("axiom_b".to_string());

    kb2.insert(
        b"fact/1",
        ProvenanceEntry {
            expr: MettaExpr::Atom("fact_1".to_string()),
            sources: sources2,
        },
    );

    // Merge - provenance sets are unioned
    let merged = merge_with_provenance(&kb1, &kb2);

    if let Some(entry) = merged.get(b"fact/1") {
        println!("Fact 1 sources: {:?}", entry.sources);
        // Should contain both "axiom_a" and "axiom_b"
    }
}

// ============================================================================
// Example 8: Distributed Knowledge Synchronization
// ============================================================================

/// Node in a distributed MeTTa system
pub struct DistributedNode {
    pub node_id: String,
    pub local_kb: Arc<RwLock<PathMap<KnowledgeEntry>>>,
}

impl DistributedNode {
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            local_kb: Arc::new(RwLock::new(PathMap::new())),
        }
    }

    /// Sync with another node
    pub fn sync_with(&self, other: &DistributedNode) {
        let other_kb = other.local_kb.read().unwrap();
        let mut local_kb = self.local_kb.write().unwrap();

        // Merge other's knowledge into local
        let merged = local_kb.join(&other_kb);
        *local_kb = merged;
    }

    /// Get diff to send to another node
    pub fn get_updates_since(&self, other_kb: &PathMap<KnowledgeEntry>) -> PathMap<KnowledgeEntry> {
        let local_kb = self.local_kb.read().unwrap();
        local_kb.subtract(other_kb)
    }
}

/// Example: Distributed knowledge synchronization
pub fn example_distributed_sync() {
    let node1 = DistributedNode::new("node1".to_string());
    let node2 = DistributedNode::new("node2".to_string());

    // Node 1 learns something
    {
        let mut kb = node1.local_kb.write().unwrap();
        kb.insert(
            b"fact/node1",
            KnowledgeEntry {
                expr: MettaExpr::Atom("node1_fact".to_string()),
                metadata: ExprMetadata {
                    source: "node1".to_string(),
                    confidence: 1.0,
                    timestamp: 1000,
                },
            },
        );
    }

    // Node 2 learns something
    {
        let mut kb = node2.local_kb.write().unwrap();
        kb.insert(
            b"fact/node2",
            KnowledgeEntry {
                expr: MettaExpr::Atom("node2_fact".to_string()),
                metadata: ExprMetadata {
                    source: "node2".to_string(),
                    confidence: 1.0,
                    timestamp: 2000,
                },
            },
        );
    }

    // Synchronize
    node1.sync_with(&node2);

    println!("Node 1 after sync: {} entries", node1.local_kb.read().unwrap().len());
}

// ============================================================================
// Example 9: Transactional Knowledge Updates
// ============================================================================

/// Transaction for knowledge base updates
pub struct Transaction {
    pub base: PathMap<KnowledgeEntry>,
    pub added: PathMap<KnowledgeEntry>,
    pub removed: PathMap<KnowledgeEntry>,
}

impl Transaction {
    pub fn new(base: PathMap<KnowledgeEntry>) -> Self {
        Self {
            base,
            added: PathMap::new(),
            removed: PathMap::new(),
        }
    }

    /// Add an entry
    pub fn add(&mut self, path: &[u8], entry: KnowledgeEntry) {
        self.added.insert(path, entry);
    }

    /// Remove an entry
    pub fn remove(&mut self, path: &[u8]) {
        if let Some(entry) = self.base.get(path) {
            self.removed.insert(path, entry.clone());
        }
    }

    /// Commit transaction
    pub fn commit(self) -> PathMap<KnowledgeEntry> {
        self.base.subtract(&self.removed).join(&self.added)
    }

    /// Rollback transaction
    pub fn rollback(self) -> PathMap<KnowledgeEntry> {
        self.base
    }
}

/// Example: Transactional reasoning
pub fn example_transactional_update() {
    let mut base = PathMap::new();
    base.insert(
        b"fact/1",
        KnowledgeEntry {
            expr: MettaExpr::Atom("fact_1".to_string()),
            metadata: ExprMetadata {
                source: "base".to_string(),
                confidence: 1.0,
                timestamp: 1000,
            },
        },
    );

    let mut tx = Transaction::new(base.clone());

    // Add new fact
    tx.add(
        b"fact/2",
        KnowledgeEntry {
            expr: MettaExpr::Atom("fact_2".to_string()),
            metadata: ExprMetadata {
                source: "transaction".to_string(),
                confidence: 0.9,
                timestamp: 2000,
            },
        },
    );

    // Remove old fact
    tx.remove(b"fact/1");

    // Commit
    let result = tx.commit();
    println!("Transaction committed: {} entries", result.len());
}

// ============================================================================
// Example 10: Query Optimization with Restrict
// ============================================================================

/// Optimized query planner using restrict
pub struct QueryPlanner {
    kb: PathMap<KnowledgeEntry>,
}

impl QueryPlanner {
    pub fn new(kb: PathMap<KnowledgeEntry>) -> Self {
        Self { kb }
    }

    /// Execute query with namespace scoping
    pub fn query_with_scope(&self, namespace: &[u8], pattern: impl Fn(&KnowledgeEntry) -> bool) -> Vec<KnowledgeEntry> {
        // First restrict to namespace (cheap)
        let scoped = self.kb.restrict(namespace);

        // Then filter by pattern (only on scoped subset)
        scoped
            .iter()
            .filter_map(|(_, entry)| {
                if pattern(entry) {
                    Some(entry.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Example: Optimized query execution
pub fn example_query_optimization() {
    let mut kb = PathMap::new();

    // Populate with many facts in different namespaces
    for i in 0..10000 {
        let namespace = format!("ns/{}/", i % 100);
        let path = format!("{}fact/{}", namespace, i);

        kb.insert(
            path.as_bytes(),
            KnowledgeEntry {
                expr: MettaExpr::Number(i as i64),
                metadata: ExprMetadata {
                    source: "generator".to_string(),
                    confidence: 1.0,
                    timestamp: i as u64,
                },
            },
        );
    }

    let planner = QueryPlanner::new(kb);

    // Query only namespace 42 for even numbers
    let results = planner.query_with_scope(b"ns/42/", |entry| {
        matches!(entry.expr, MettaExpr::Number(n) if n % 2 == 0)
    });

    println!("Found {} matching entries", results.len());
}

// ============================================================================
// Main Function - Run All Examples
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_examples() {
        println!("\n=== Example 1: Module Merge ===");
        example_module_merge();

        println!("\n=== Example 2: Scoped Query ===");
        example_scoped_query();

        println!("\n=== Example 3: Version Control ===");
        example_version_control();

        println!("\n=== Example 4: Proof Validation ===");
        example_proof_validation();

        println!("\n=== Example 5: Multi-Agent Merge ===");
        example_multi_agent_merge();

        println!("\n=== Example 6: Incremental Reasoning ===");
        example_incremental_reasoning();

        println!("\n=== Example 7: Provenance Tracking ===");
        example_provenance_tracking();

        println!("\n=== Example 8: Distributed Sync ===");
        example_distributed_sync();

        println!("\n=== Example 9: Transactional Update ===");
        example_transactional_update();

        println!("\n=== Example 10: Query Optimization ===");
        example_query_optimization();
    }
}

// ============================================================================
// Integration Notes for MeTTaTron
// ============================================================================
//
// To integrate these patterns into MeTTaTron:
//
// 1. Replace simplified MettaExpr with actual MeTTa AST types
// 2. Implement Lattice trait for your value types (KnowledgeEntry, etc.)
// 3. Use PathMap as the backing store for Space implementations
// 4. Leverage algebraic operations for:
//    - Module imports (join)
//    - Scoped queries (restrict)
//    - Differential reasoning (subtract)
//    - Proof validation (meet)
// 5. Consider using WriteZipper for incremental updates
// 6. Use structural sharing (clone) for cheap snapshots
//
// Performance Considerations:
// - Prefer restrict over full iteration when possible
// - Use zipper operations for localized updates
// - Leverage structural sharing via clone
// - Consider parallel operations (rayon) for large merges
// - Identity detection is automatic and fast
//
// Memory Considerations:
// - Clone is O(1) due to structural sharing
// - Algebraic operations create new nodes only where needed
// - Consider using make_unique sparingly (it's expensive)
// - Large multi-way joins can be memory intensive
//
// Correctness Considerations:
// - Ensure your Lattice implementations are associative/commutative
// - Test identity detection behavior with your types
// - Validate that subtract/meet behave as expected
// - Consider using AlgebraicResult for partial operations
