# Pattern A: Read-Only Sharing (Arc<PathMap>)

**Use Case**: Read-heavy workloads with concurrent queries

**Characteristics**:
- Memory: 16 bytes per thread (Arc overhead)
- Synchronization: None (lock-free reads)
- Scalability: Linear with thread count
- Best for: Knowledge base queries, concurrent searches

---

## 1. Basic Implementation

```rust
use std::sync::Arc;
use pathmap::PathMap;

// Create and populate knowledge base
let mut kb = PathMap::<KnowledgeEntry>::new();
// ... populate kb ...

// Share via Arc
let kb = Arc::new(kb);

// Spawn query threads
for query_id in 0..num_threads {
    let kb_ref = Arc::clone(&kb);
    
    thread::spawn(move || {
        // Lock-free read access
        let zipper = kb_ref.read_zipper();
        execute_query(&zipper, query_id);
    });
}
```

**Key points**:
- Clone Arc, not PathMap (only 16 bytes vs entire structure)
- Each thread gets independent read zipper
- Zero synchronization overhead

---

## 2. Query Patterns

### 2.1 Point Queries

```rust
let kb_ref = Arc::clone(&kb);
thread::spawn(move || {
    if let Some(entry) = kb_ref.get(b"facts/math/addition") {
        process_entry(entry);
    }
});
```

### 2.2 Prefix Queries

```rust
let kb_ref = Arc::clone(&kb);
thread::spawn(move || {
    let restricted = kb_ref.restrict_by_path(b"facts/math/");
    for (path, entry) in restricted.iter() {
        process_math_fact(path, entry);
    }
});
```

### 2.3 Full Traversal

```rust
let kb_ref = Arc::clone(&kb);
thread::spawn(move || {
    let mut zipper = kb_ref.read_zipper();
    while let Some(entry) = zipper.to_next_val() {
        process_entry(entry);
    }
});
```

---

## 3. MeTTaTron Integration

```rust
pub struct KnowledgeBase {
    data: Arc<PathMap<KnowledgeEntry>>,
}

impl KnowledgeBase {
    pub fn query(&self) -> ReadZipperUntracked<KnowledgeEntry> {
        self.data.read_zipper()
    }
    
    pub fn parallel_search(&self, queries: Vec<Query>) -> Vec<Result> {
        queries.par_iter()  // Rayon parallel iterator
            .map(|q| self.execute_query(q))
            .collect()
    }
    
    fn execute_query(&self, query: &Query) -> Result {
        let zipper = self.data.read_zipper();
        // Execute query logic
    }
}
```

---

## 4. Performance Characteristics

**Throughput vs Thread Count** (from benchmarks):
- 1 thread: 1.00× (baseline)
- 2 threads: 1.95×
- 4 threads: 3.80×
- 8 threads: 7.20×
- 16 threads: 13.5×

**Scalability**: Near-linear until memory bandwidth saturation

**Memory**: Single PathMap + 16 bytes × thread count

---

## 5. When to Use

✅ **Use this pattern when**:
- Read-heavy workload (>95% reads)
- Multiple concurrent queries
- Memory overhead is acceptable
- Simplicity is important

❌ **Don't use when**:
- Frequent updates needed
- Write throughput is critical
- Need versioning/snapshots

**Alternative**: For updates, see [Clone+Merge Pattern](05_usage_pattern_clone_merge.md)

---

## References

- Benchmark: `benches/parallel.rs:30-106`
- Example: [examples/01_read_only_sharing.rs](examples/01_read_only_sharing.rs)
