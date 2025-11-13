# Pattern B: Clone + Modify + Merge

**Use Case**: Independent parallel computation with result merging

**Characteristics**:
- Memory: Shared structure + per-thread deltas (COW)
- Synchronization: None during compute, merge at end
- Scalability: Linear for computation, O(N) merge cost
- Best for: Parallel reasoning, distributed computation

---

## 1. Basic Implementation

```rust
let base = PathMap::<KnowledgeEntry>::new();
// ... populate base ...

// Parallel computation phase
let results: Vec<PathMap<_>> = thread::scope(|scope| {
    (0..num_threads).map(|thread_id| {
        let clone = base.clone();  // O(1), shares structure
        
        scope.spawn(move || {
            let mut map = clone;
            // Modify independently - COW isolation
            perform_reasoning(&mut map, thread_id);
            map
        })
    }).collect::<Vec<_>>()
    .into_iter()
    .map(|h| h.join().unwrap())
    .collect()
});

// Merge phase (sequential)
let mut final_result = results[0].clone();
for map in &results[1..] {
    final_result = final_result.join(map);  // Union
}
```

---

## 2. Merge Strategies

### 2.1 Union (join)

```rust
// Combine all knowledge from both maps
let merged = map1.join(&map2);
```

### 2.2 Intersection (meet)

```rust
// Keep only shared knowledge
let common = map1.meet(&map2);
```

### 2.3 Difference (subtract)

```rust
// Remove map2's knowledge from map1
let diff = map1.subtract(&map2);
```

### 2.4 Custom Merge

```rust
fn custom_merge(maps: Vec<PathMap<Entry>>) -> PathMap<Entry> {
    let mut result = PathMap::new();
    for (path, entries) in aggregate_by_path(maps) {
        let merged_entry = merge_entries(entries);
        result.insert(path, merged_entry);
    }
    result
}
```

---

## 3. MeTTaTron Integration

```rust
pub struct ParallelReasoner {
    base_kb: PathMap<KnowledgeEntry>,
}

impl ParallelReasoner {
    pub fn reason_parallel(&self, queries: Vec<Query>) -> PathMap<Conclusion> {
        let results: Vec<_> = queries.par_iter()
            .map(|query| {
                let mut kb = self.base_kb.clone();  // O(1)
                self.apply_reasoning_rules(&mut kb, query);
                kb
            })
            .collect();
        
        // Merge all results
        results.into_iter()
            .reduce(|acc, kb| acc.join(&kb))
            .unwrap_or_else(|| PathMap::new())
    }
}
```

---

## 4. Memory Efficiency

**Example**: 10 threads, 1M nodes, 1% modifications each

Without sharing: 10 × 1M = 10M nodes
With COW: 1M + (10 × 0.01 × 1M) = 1.1M nodes
**Savings**: 9.1× reduction

---

## 5. Performance Characteristics

**Computation phase**: Linear scaling (no synchronization)
**Merge phase**: O(N × K) for K maps of size N

**Optimization**: Parallel tree-reduce merge
```rust
use rayon::prelude::*;

let merged = results.par_iter()
    .cloned()
    .reduce(|| PathMap::new(), |acc, map| acc.join(&map));
```

---

## 6. When to Use

✅ **Use this pattern when**:
- Independent computations
- Can tolerate merge cost
- Need isolation during computation
- Results combine naturally (union/intersection)

❌ **Don't use when**:
- Merge cost is prohibitive
- Need immediate visibility of updates
- Memory for clones is constrained

**Alternative**: For immediate visibility, see [ZipperHead Pattern](06_usage_pattern_zipperhead.md)

---

## References

- Algebraic operations: `../PATHMAP_ALGEBRAIC_OPERATIONS.md`
- COW analysis: `../PATHMAP_COW_ANALYSIS.md`
- Example: [examples/02_clone_per_thread.rs](examples/02_clone_per_thread.rs)
