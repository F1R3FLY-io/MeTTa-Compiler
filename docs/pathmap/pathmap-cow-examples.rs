// PathMap Copy-On-Write Usage Examples
//
// Practical examples demonstrating COW patterns for MeTTaTron
// Reference: PATHMAP_COW_ANALYSIS.md Section 9
//
// These examples show how to leverage PathMap's COW capabilities
// for efficient knowledge base management in MeTTaTron.

#![allow(dead_code, unused_variables, unused_imports)]

use pathmap::PathMap;
use std::sync::{Arc, RwLock};
use std::collections::VecDeque;

// Mock types for demonstration (replace with actual MeTTaTron types)
type MettaValue = String;
type Expr = String;
type Query = String;

// ============================================================================
// Pattern 1: Cheap Snapshots for Undo/Redo
// ============================================================================

/// Snapshot manager providing efficient undo/redo for knowledge base
///
/// Time complexity:
/// - snapshot(): O(1)
/// - undo(): O(1)
/// - redo(): O(1)
///
/// Space complexity: O(h) where h = history size
pub struct SnapshotManager<V> {
    current: PathMap<V>,
    history: Vec<(String, PathMap<V>)>,  // (description, snapshot)
    redo_stack: Vec<(String, PathMap<V>)>,
    max_history: usize,
}

impl<V: Clone> SnapshotManager<V> {
    pub fn new(max_history: usize) -> Self {
        Self {
            current: PathMap::new(),
            history: Vec::new(),
            redo_stack: Vec::new(),
            max_history,
        }
    }

    /// Create a snapshot of current state (O(1))
    pub fn snapshot(&mut self, description: String) {
        // Clear redo stack on new snapshot
        self.redo_stack.clear();

        // Add current state to history
        self.history.push((description, self.current.clone()));

        // Limit history size (FIFO)
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// Undo last operation (O(1))
    pub fn undo(&mut self) -> Option<String> {
        self.history.pop().map(|(desc, snapshot)| {
            // Save current for redo
            self.redo_stack.push((
                format!("Redo: {}", desc),
                self.current.clone()
            ));

            // Restore snapshot
            self.current = snapshot;
            format!("Undone: {}", desc)
        })
    }

    /// Redo last undone operation (O(1))
    pub fn redo(&mut self) -> Option<String> {
        self.redo_stack.pop().map(|(desc, snapshot)| {
            // Save current for undo
            self.history.push((
                format!("Undo: {}", desc),
                self.current.clone()
            ));

            // Restore redone state
            self.current = snapshot;
            format!("Redone: {}", desc)
        })
    }

    /// Get current state (immutable)
    pub fn get_current(&self) -> &PathMap<V> {
        &self.current
    }

    /// Get mutable access to current state
    pub fn get_current_mut(&mut self) -> &mut PathMap<V> {
        &mut self.current
    }

    /// Get history descriptions
    pub fn get_history(&self) -> Vec<&str> {
        self.history.iter().map(|(desc, _)| desc.as_str()).collect()
    }
}

/// Example usage: MORK space with undo/redo
fn example_snapshot_manager() {
    let mut mgr = SnapshotManager::<MettaValue>::new(100);

    // Initial state
    mgr.get_current_mut().insert("fact1".into(), "value1".into());
    mgr.snapshot("Added fact1".into());

    // Modify
    mgr.get_current_mut().insert("fact2".into(), "value2".into());
    mgr.snapshot("Added fact2".into());

    mgr.get_current_mut().insert("fact3".into(), "value3".into());
    mgr.snapshot("Added fact3".into());

    // Undo twice
    println!("{}", mgr.undo().unwrap());  // "Undone: Added fact3"
    println!("{}", mgr.undo().unwrap());  // "Undone: Added fact2"

    // Current state has only fact1
    assert!(mgr.get_current().contains_key("fact1"));
    assert!(!mgr.get_current().contains_key("fact2"));
    assert!(!mgr.get_current().contains_key("fact3"));

    // Redo once
    println!("{}", mgr.redo().unwrap());  // "Redone: ..."

    // Now has fact1 and fact2
    assert!(mgr.get_current().contains_key("fact1"));
    assert!(mgr.get_current().contains_key("fact2"));
    assert!(!mgr.get_current().contains_key("fact3"));
}

// ============================================================================
// Pattern 2: Concurrent Readers with Single Writer
// ============================================================================

/// Thread-safe PathMap wrapper for concurrent access
///
/// Supports:
/// - Multiple concurrent readers (no blocking)
/// - Single writer (exclusive access)
/// - O(1) snapshot creation under read lock
pub struct ConcurrentPathMap<V> {
    data: Arc<RwLock<PathMap<V>>>,
}

impl<V: Clone> ConcurrentPathMap<V> {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(PathMap::new())),
        }
    }

    /// Read with closure (multiple concurrent readers)
    pub fn read<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&PathMap<V>) -> R,
    {
        let read_guard = self.data.read().unwrap();
        f(&*read_guard)
    }

    /// Write with closure (exclusive access)
    pub fn write<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut PathMap<V>) -> R,
    {
        let mut write_guard = self.data.write().unwrap();
        f(&mut *write_guard)
    }

    /// Create snapshot (O(1), under read lock)
    pub fn snapshot(&self) -> PathMap<V> {
        let read_guard = self.data.read().unwrap();
        read_guard.clone()
    }

    /// Bulk insert (write lock held for duration)
    pub fn bulk_insert(&self, items: Vec<(String, V)>) {
        let mut write_guard = self.data.write().unwrap();
        for (key, value) in items {
            write_guard.insert(key, value);
        }
    }
}

impl<V: Clone> Clone for ConcurrentPathMap<V> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),  // Clone Arc (refcount++)
        }
    }
}

/// Example usage: Shared MORK space across threads
fn example_concurrent_readers() {
    use std::thread;

    let mork_space = Arc::new(ConcurrentPathMap::<MettaValue>::new());

    // Populate initial data
    mork_space.write(|map| {
        for i in 0..1000 {
            map.insert(format!("fact_{}", i), format!("value_{}", i));
        }
    });

    // Spawn multiple reader threads
    let mut handles = vec![];

    for i in 0..10 {
        let space = mork_space.clone();
        let handle = thread::spawn(move || {
            // Each thread reads concurrently
            space.read(|map| {
                let key = format!("fact_{}", i * 100);
                match map.get(&key) {
                    Some(value) => println!("Thread {}: Found {}", i, value),
                    None => println!("Thread {}: Not found", i),
                }
            });
        });
        handles.push(handle);
    }

    // Writer thread (occasional updates)
    let space_writer = mork_space.clone();
    let writer_handle = thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(10));
        space_writer.write(|map| {
            map.insert("new_fact".into(), "new_value".into());
        });
    });

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }
    writer_handle.join().unwrap();
}

// ============================================================================
// Pattern 3: Transactional MORK Operations
// ============================================================================

/// Transactional wrapper for PathMap
///
/// Provides commit/rollback semantics:
/// - begin_transaction(): Start new transaction (O(1) clone)
/// - commit(): Make changes permanent (O(1))
/// - rollback(): Discard changes (O(1))
pub struct TransactionalPathMap<V> {
    committed: PathMap<V>,
    transaction: Option<PathMap<V>>,
}

impl<V: Clone> TransactionalPathMap<V> {
    pub fn new() -> Self {
        Self {
            committed: PathMap::new(),
            transaction: None,
        }
    }

    /// Start a new transaction (clones current state)
    pub fn begin_transaction(&mut self) -> Result<(), String> {
        if self.transaction.is_some() {
            return Err("Transaction already in progress".into());
        }

        // O(1) clone of committed state
        self.transaction = Some(self.committed.clone());
        Ok(())
    }

    /// Insert within transaction
    pub fn insert(&mut self, key: String, value: V) -> Result<(), String> {
        if let Some(ref mut txn) = self.transaction {
            txn.insert(key, value);
            Ok(())
        } else {
            Err("No active transaction".into())
        }
    }

    /// Get from transaction or committed
    pub fn get(&self, key: &str) -> Option<&V> {
        self.transaction.as_ref()
            .and_then(|txn| txn.get(key))
            .or_else(|| self.committed.get(key))
    }

    /// Commit transaction (replace committed with transaction)
    pub fn commit(&mut self) -> Result<(), String> {
        if let Some(txn) = self.transaction.take() {
            self.committed = txn;
            Ok(())
        } else {
            Err("No active transaction".into())
        }
    }

    /// Rollback transaction (discard changes)
    pub fn rollback(&mut self) -> Result<(), String> {
        if self.transaction.take().is_some() {
            Ok(())
        } else {
            Err("No active transaction".into())
        }
    }

    /// Get committed state (immutable)
    pub fn get_committed(&self) -> &PathMap<V> {
        &self.committed
    }
}

/// Example usage: Transactional fact insertion
fn example_transactional_mork() {
    let mut space = TransactionalPathMap::<MettaValue>::new();

    // Transaction 1: Add facts
    space.begin_transaction().unwrap();
    space.insert("fact1".into(), "value1".into()).unwrap();
    space.insert("fact2".into(), "value2".into()).unwrap();
    space.commit().unwrap();

    // Transaction 2: Try to add conflicting fact
    space.begin_transaction().unwrap();
    space.insert("fact3".into(), "value3".into()).unwrap();

    // Detect conflict (mock)
    let has_conflict = space.get("fact3").is_some();
    if has_conflict {
        space.rollback().unwrap();
        println!("Transaction rolled back due to conflict");
    }

    // Committed state still has only fact1 and fact2
    assert!(space.get_committed().contains_key("fact1"));
    assert!(space.get_committed().contains_key("fact2"));
    assert!(!space.get_committed().contains_key("fact3"));
}

// ============================================================================
// Pattern 4: Versioned Knowledge Base
// ============================================================================

/// Versioned PathMap for time-travel queries
///
/// Supports:
/// - Efficient version storage via COW
/// - Time-travel queries (get_at_version)
/// - Garbage collection of old versions
pub struct VersionedPathMap<V> {
    versions: Vec<(u64, PathMap<V>)>,  // (timestamp, version)
    current_version: u64,
}

impl<V: Clone> VersionedPathMap<V> {
    pub fn new() -> Self {
        Self {
            versions: vec![(0, PathMap::new())],
            current_version: 0,
        }
    }

    /// Insert new key-value (creates new version)
    pub fn insert(&mut self, key: String, value: V) {
        // Get latest version
        let (_, latest) = self.versions.last().unwrap();

        // Clone (O(1)) and modify
        let mut new_version = latest.clone();
        new_version.insert(key, value);

        // Add new version
        self.current_version += 1;
        self.versions.push((self.current_version, new_version));
    }

    /// Get value at specific version
    pub fn get_at_version(&self, key: &str, version: u64) -> Option<&V> {
        // Binary search for version (versions are sorted)
        let idx = self.versions.binary_search_by_key(&version, |(v, _)| *v)
            .unwrap_or_else(|idx| idx.saturating_sub(1));

        self.versions.get(idx)
            .and_then(|(_, map)| map.get(key))
    }

    /// Get current version number
    pub fn current_version(&self) -> u64 {
        self.current_version
    }

    /// Get value at current version
    pub fn get_current(&self, key: &str) -> Option<&V> {
        self.versions.last().unwrap().1.get(key)
    }

    /// List all versions
    pub fn list_versions(&self) -> Vec<u64> {
        self.versions.iter().map(|(v, _)| *v).collect()
    }

    /// Garbage collect old versions (keep only last N)
    pub fn gc_old_versions(&mut self, keep_last_n: usize) {
        if self.versions.len() > keep_last_n {
            self.versions.drain(0..self.versions.len() - keep_last_n);
        }
    }

    /// Get memory usage estimate
    pub fn estimate_memory_usage(&self) -> usize {
        // Rough estimate: 32 bytes per PathMap struct + shared nodes
        self.versions.len() * 32
    }
}

/// Example usage: Time-travel queries on knowledge base
fn example_versioned_knowledge_base() {
    let mut vkb = VersionedPathMap::<MettaValue>::new();

    // Version 0: Empty
    assert_eq!(vkb.current_version(), 0);

    // Version 1: Add fact1
    vkb.insert("fact1".into(), "original_value".into());
    assert_eq!(vkb.current_version(), 1);

    // Version 2: Add fact2
    vkb.insert("fact2".into(), "value2".into());

    // Version 3: Modify fact1
    vkb.insert("fact1".into(), "updated_value".into());

    // Time-travel queries
    assert_eq!(
        vkb.get_at_version("fact1", 1),
        Some(&"original_value".to_string())
    );
    assert_eq!(
        vkb.get_at_version("fact1", 3),
        Some(&"updated_value".to_string())
    );
    assert_eq!(vkb.get_at_version("fact2", 1), None);
    assert_eq!(
        vkb.get_at_version("fact2", 2),
        Some(&"value2".to_string())
    );

    // Garbage collection
    println!("Versions before GC: {}", vkb.list_versions().len());
    vkb.gc_old_versions(2);  // Keep only last 2 versions
    println!("Versions after GC: {}", vkb.list_versions().len());
}

// ============================================================================
// Pattern 5: Isolated MORK Spaces for Parallel Evaluation
// ============================================================================

/// Isolated MORK space with shared base + local overlay
///
/// Design:
/// - Shared base: Read-only, shared across all instances
/// - Local overlay: Thread-local mutations
/// - Two-level lookup: local first, then shared
pub struct IsolatedMORKSpace {
    shared_base: Arc<PathMap<MettaValue>>,
    local_overlay: PathMap<MettaValue>,
}

impl IsolatedMORKSpace {
    /// Create new isolated space from shared base
    pub fn new(base: Arc<PathMap<MettaValue>>) -> Self {
        Self {
            shared_base: base,
            local_overlay: PathMap::new(),
        }
    }

    /// Insert into local overlay (doesn't affect shared base)
    pub fn insert_local(&mut self, key: String, value: MettaValue) {
        self.local_overlay.insert(key, value);
    }

    /// Get value (checks local first, then shared)
    pub fn get(&self, key: &str) -> Option<&MettaValue> {
        self.local_overlay.get(key)
            .or_else(|| self.shared_base.get(key))
    }

    /// Check if key exists (local or shared)
    pub fn contains_key(&self, key: &str) -> bool {
        self.local_overlay.contains_key(key) || self.shared_base.contains_key(key)
    }

    /// Get local overlay size
    pub fn local_size(&self) -> usize {
        self.local_overlay.len()
    }

    /// Merge local changes back into a mutable base
    pub fn merge_into_base(self, base: &mut PathMap<MettaValue>) {
        for (key, value) in self.local_overlay.iter() {
            base.insert(key.clone(), value.clone());
        }
    }

    /// Create snapshot of combined state
    pub fn snapshot(&self) -> PathMap<MettaValue> {
        // Start with shared base clone (O(1))
        let mut snapshot = (*self.shared_base).clone();

        // Apply local overlay
        for (key, value) in self.local_overlay.iter() {
            snapshot.insert(key.clone(), value.clone());
        }

        snapshot
    }
}

/// Example usage: Parallel query evaluation with isolated spaces
fn example_parallel_evaluation() {
    use rayon::prelude::*;

    // Shared global MORK space (1000 facts)
    let mut global_space = PathMap::new();
    for i in 0..1000 {
        global_space.insert(format!("global_fact_{}", i), format!("value_{}", i));
    }
    let global_space = Arc::new(global_space);

    // Queries to evaluate
    let queries = vec!["query1", "query2", "query3", "query4"];

    // Thread-local facts for each query
    let local_facts = vec![
        vec![("local_1".to_string(), "value_1".to_string())],
        vec![("local_2".to_string(), "value_2".to_string())],
        vec![("local_3".to_string(), "value_3".to_string())],
        vec![("local_4".to_string(), "value_4".to_string())],
    ];

    // Parallel evaluation
    let results: Vec<_> = queries.par_iter().enumerate().map(|(i, query)| {
        // Each thread gets isolated space
        let mut space = IsolatedMORKSpace::new(global_space.clone());

        // Add thread-local facts
        for (key, value) in &local_facts[i] {
            space.insert_local(key.clone(), value.clone());
        }

        // Evaluate query with isolated space
        // (mock evaluation - replace with actual MeTTa evaluation)
        let result = format!("Result for {} with {} local facts",
            query, space.local_size());

        // Space dropped here - local overlay discarded
        result
    }).collect();

    // Print results
    for (i, result) in results.iter().enumerate() {
        println!("{}", result);
    }

    // Global space unchanged
    assert_eq!(global_space.len(), 1000);
}

// ============================================================================
// Pattern 6: Diff and Merge for Knowledge Base Updates
// ============================================================================

/// Diff between two PathMaps
pub struct PathMapDiff<V> {
    pub added: Vec<(String, V)>,
    pub removed: Vec<(String, V)>,
    pub modified: Vec<(String, V, V)>,  // (key, old_value, new_value)
}

impl<V: Clone + PartialEq> PathMapDiff<V> {
    /// Compute diff between two PathMaps
    pub fn compute(old: &PathMap<V>, new: &PathMap<V>) -> Self {
        let mut added = vec![];
        let mut removed = vec![];
        let mut modified = vec![];

        // Find additions and modifications
        for (key, new_val) in new.iter() {
            match old.get(key) {
                None => added.push((key.clone(), new_val.clone())),
                Some(old_val) if old_val != new_val => {
                    modified.push((key.clone(), old_val.clone(), new_val.clone()));
                }
                _ => {}  // Unchanged
            }
        }

        // Find removals
        for (key, old_val) in old.iter() {
            if !new.contains_key(key) {
                removed.push((key.clone(), old_val.clone()));
            }
        }

        Self { added, removed, modified }
    }

    /// Apply diff to a PathMap
    pub fn apply(self, target: &mut PathMap<V>) {
        // Apply additions and modifications
        for (key, value) in self.added {
            target.insert(key, value);
        }
        for (key, _, new_value) in self.modified {
            target.insert(key, new_value);
        }

        // Apply removals
        for (key, _) in self.removed {
            target.remove(&key);
        }
    }

    /// Check if diff is empty
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
}

/// Three-way merge result
pub enum MergeResult<V> {
    Success(PathMap<V>),
    Conflict(Vec<MergeConflict>),
}

/// Merge conflict types
pub enum MergeConflict {
    BothAdded(String),
    BothModified(String),
    AddedAndRemoved(String),
}

/// Example usage: Diff and merge knowledge bases
fn example_diff_and_merge() {
    let mut base = PathMap::new();
    base.insert("fact1".into(), "original".into());
    base.insert("fact2".into(), "value2".into());

    let mut version_a = base.clone();
    version_a.insert("fact1".into(), "modified_by_a".into());
    version_a.insert("fact3".into(), "added_by_a".into());

    let mut version_b = base.clone();
    version_b.insert("fact2".into(), "modified_by_b".into());
    version_b.remove("fact1");

    // Compute diffs
    let diff_a = PathMapDiff::compute(&base, &version_a);
    let diff_b = PathMapDiff::compute(&base, &version_b);

    println!("Diff A: {} added, {} removed, {} modified",
        diff_a.added.len(), diff_a.removed.len(), diff_a.modified.len());
    println!("Diff B: {} added, {} removed, {} modified",
        diff_b.added.len(), diff_b.removed.len(), diff_b.modified.len());

    // Three-way merge (simplified - would need conflict resolution)
    let mut merged = base.clone();
    diff_a.apply(&mut merged);
    // diff_b.apply(&mut merged);  // Would conflict on fact1
}

// ============================================================================
// Main Function (for running examples)
// ============================================================================

fn main() {
    println!("=== PathMap COW Examples ===\n");

    println!("1. Snapshot Manager:");
    example_snapshot_manager();

    println!("\n2. Concurrent Readers:");
    example_concurrent_readers();

    println!("\n3. Transactional MORK:");
    example_transactional_mork();

    println!("\n4. Versioned Knowledge Base:");
    example_versioned_knowledge_base();

    println!("\n5. Parallel Evaluation:");
    example_parallel_evaluation();

    println!("\n6. Diff and Merge:");
    example_diff_and_merge();

    println!("\nAll examples completed successfully!");
}

// ============================================================================
// Integration Guide
// ============================================================================

/*
## Integrating into MeTTaTron

### 1. Snapshot Manager for MORK Spaces

```rust
use crate::pathmap_patterns::SnapshotManager;

pub struct MettaTronEngine {
    mork_space: SnapshotManager<MettaValue>,
    // ... other fields
}

impl MettaTronEngine {
    pub fn checkpoint(&mut self) {
        self.mork_space.snapshot("User checkpoint".into());
    }

    pub fn undo(&mut self) {
        if let Some(desc) = self.mork_space.undo() {
            println!("Undone: {}", desc);
        }
    }
}
```

### 2. Concurrent Access Pattern

```rust
pub struct SharedMORKSpace {
    space: Arc<ConcurrentPathMap<MettaValue>>,
}

impl SharedMORKSpace {
    pub fn query(&self, key: &str) -> Option<MettaValue> {
        self.space.read(|map| map.get(key).cloned())
    }

    pub fn insert_fact(&self, key: String, value: MettaValue) {
        self.space.write(|map| {
            map.insert(key, value);
        });
    }
}
```

### 3. Transactional Operations

```rust
pub fn execute_transaction(
    space: &mut TransactionalPathMap<MettaValue>,
    operations: Vec<Operation>,
) -> Result<(), String> {
    space.begin_transaction()?;

    for op in operations {
        match op.execute(space) {
            Ok(_) => continue,
            Err(e) => {
                space.rollback()?;
                return Err(format!("Transaction failed: {}", e));
            }
        }
    }

    space.commit()
}
```

### 4. Parallel Query Evaluation

```rust
use rayon::prelude::*;

pub fn evaluate_queries_parallel(
    queries: &[Query],
    global_space: Arc<PathMap<MettaValue>>,
) -> Vec<Vec<MettaValue>> {
    queries.par_iter().map(|query| {
        let mut space = IsolatedMORKSpace::new(global_space.clone());
        // Add query-specific facts
        // Evaluate and return results
        evaluate_query(query, &space)
    }).collect()
}
```

## Performance Tips

1. **Snapshot Frequency**: Balance between granularity and memory usage
   - Too frequent: Memory overhead from many versions
   - Too infrequent: Loss of undo granularity
   - Recommended: Snapshot on significant operations or every N modifications

2. **Garbage Collection**: Implement periodic cleanup
   ```rust
   if history.len() > MAX_HISTORY {
       history.gc_old_versions(MAX_HISTORY / 2);
   }
   ```

3. **Read vs. Write Patterns**:
   - Read-heavy: COW excels (zero-cost clones, structural sharing)
   - Write-heavy: Consider batching mutations or avoiding snapshots

4. **Memory Monitoring**: Track memory usage with jemalloc
   ```rust
   #[cfg(feature = "jemalloc")]
   let allocated = tikv_jemalloc_ctl::stats::allocated::read().unwrap();
   if allocated > THRESHOLD {
       trigger_gc();
   }
   ```

## Testing

Add tests for each pattern:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_undo_redo() {
        // Test snapshot manager functionality
    }

    #[test]
    fn test_concurrent_access() {
        // Test concurrent readers with writer
    }

    #[test]
    fn test_transaction_rollback() {
        // Test transaction commit/rollback
    }
}
```

See PATHMAP_COW_ANALYSIS.md Section 11.4 for complete testing strategy.
*/
