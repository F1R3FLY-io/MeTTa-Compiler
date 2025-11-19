# MeTTaTron Integration Guide

**Purpose**: Practical guide for integrating PathMap threading patterns into MeTTaTron.

---

## 1. Quick Decision Matrix

| Workload | Pattern | Rationale |
|----------|---------|-----------|
| Queries only | A (Arc<PathMap>) | Zero-copy, lock-free |
| Independent reasoning | B (Clone+Merge) | No sync during compute |
| KB construction | C (ZipperHead) | No merge cost |
| Queries + updates | D (Hybrid) | Best of both worlds |

---

## 2. Knowledge Base Design

### 2.1 Basic Structure

```rust
use pathmap::PathMap;
use std::sync::Arc;

pub struct KnowledgeEntry {
    expr: MettaExpr,
    metadata: Metadata,
}

// Must implement: Clone + Send + Sync + Unpin + 'static
impl TrieValue for KnowledgeEntry {}

pub struct KnowledgeBase {
    data: Arc<PathMap<KnowledgeEntry>>,
}
```

### 2.2 Path Schema

```
Recommended hierarchy:
  spaces/<space_id>/
    facts/<domain>/
    rules/<rule_id>/
    queries/<query_id>/
    cache/<hash>/
```

**Benefits**:
- Natural partitioning for parallel writes
- Efficient prefix queries (restrict by space)
- Clear separation of concerns

---

## 3. Pattern A: Query Engine

### 3.1 Implementation

```rust
impl KnowledgeBase {
    pub fn query(&self, pattern: &Pattern) -> Vec<Match> {
        let zipper = self.data.read_zipper();
        self.execute_pattern_match(&zipper, pattern)
    }
    
    pub fn parallel_queries(&self, queries: Vec<Query>) -> Vec<Result> {
        queries.par_iter()
            .map(|q| self.query(&q.pattern))
            .collect()
    }
}
```

### 3.2 Performance

- **Throughput**: Linear with thread count
- **Latency**: ~microseconds per query (map size dependent)
- **Memory**: Single KB + 16 bytes per query thread

---

## 4. Pattern B: Parallel Reasoning

### 4.1 Implementation

```rust
pub fn reason_parallel(&self, goals: Vec<Goal>) -> PathMap<Conclusion> {
    let kb_snapshot = self.data.as_ref().clone();  // O(1)
    
    goals.par_iter()
        .map(|goal| {
            let mut kb = kb_snapshot.clone();  // O(1)
            apply_reasoning_rules(&mut kb, goal);
            kb
        })
        .reduce(|| PathMap::new(), |a, b| a.join(&b))
}
```

### 4.2 Merge Strategies

```rust
// Union: Combine all conclusions
conclusions.reduce(|a, b| a.join(&b))

// Intersection: Only agreed-upon conclusions
conclusions.reduce(|a, b| a.meet(&b))

// Custom: Application-specific logic
conclusions.fold(PathMap::new(), |acc, c| custom_merge(acc, c))
```

---

## 5. Pattern C: KB Construction

### 5.1 Implementation

```rust
pub fn build_kb_parallel(sources: Vec<DataSource>) -> PathMap<KnowledgeEntry> {
    let mut kb = PathMap::new();
    let zipper_head = kb.zipper_head();
    
    thread::scope(|scope| {
        sources.into_iter().enumerate().map(|(idx, source)| {
            let path = format!("source_{}/", idx).into_bytes();
            let writer = zipper_head
                .write_zipper_at_exclusive_path(path)
                .expect("Sources are partitioned");
            
            scope.spawn(move || {
                let mut wz = writer;
                for entry in source.entries() {
                    wz.descend_to(&entry.path);
                    wz.set_val(entry.value);
                    wz.reset();
                }
            })
        }).collect::<Vec<_>>()
        .into_iter()
        .for_each(|h| h.join().unwrap());
    });
    
    kb
}
```

### 5.2 Partitioning

```rust
// By space ID
format!("spaces/{}/", space_id)

// By domain
format!("facts/{}/", domain)

// By hash (for load balancing)
format!("partition_{}/", hash(key) % num_threads)
```

---

## 6. Pattern D: Live System

### 6.1 Implementation

```rust
pub struct LiveKB {
    map: PathMap<KnowledgeEntry>,
}

impl LiveKB {
    pub fn query_while_updating(&mut self) {
        let zipper_head = self.map.zipper_head();
        
        thread::scope(|scope| {
            // Query threads
            for _ in 0..num_query_threads {
                let rz = zipper_head.read_zipper_at_path(b"").unwrap();
                scope.spawn(move || run_queries(rz));
            }
            
            // Update thread
            let wz = zipper_head
                .write_zipper_at_exclusive_path(b"updates/")
                .unwrap();
            scope.spawn(move || apply_updates(wz));
        });
    }
}
```

---

## 7. Integration Checklist

- [ ] **Define KnowledgeEntry type** with Clone + Send + Sync + Unpin + 'static
- [ ] **Design path schema** for natural partitioning
- [ ] **Choose pattern** based on workload (see decision matrix)
- [ ] **Enable jemalloc** in Cargo.toml if write-heavy
- [ ] **Consider slim_ptrs** for memory-constrained deployments
- [ ] **Implement error handling** for ZipperHead conflicts
- [ ] **Add metrics** for performance monitoring
- [ ] **Test with ThreadSanitizer** to verify thread safety
- [ ] **Benchmark** with realistic data and thread counts
- [ ] **Profile** to identify bottlenecks

---

## 8. Common Pitfalls

### 8.1 Non-Thread-Safe Values

```rust
// ❌ Wrong: Rc is not Send
type Entry = Rc<MettaExpr>;

// ✅ Right: Arc is Send + Sync
type Entry = Arc<MettaExpr>;
```

### 8.2 Overlapping Write Paths

```rust
// ❌ Wrong: Paths overlap
let wz1 = zipper_head.write_zipper_at_exclusive_path(b"a/b")?;
let wz2 = zipper_head.write_zipper_at_exclusive_path(b"a")?;  // Error!

// ✅ Right: Disjoint paths
let wz1 = zipper_head.write_zipper_at_exclusive_path(b"partition_1/")?;
let wz2 = zipper_head.write_zipper_at_exclusive_path(b"partition_2/")?;
```

### 8.3 Excessive Cloning

```rust
// ❌ Wrong: Clone per iteration
for entry in entries {
    let mut kb = base.clone();
    kb.insert(entry.path, entry.value);
    results.push(kb);
}

// ✅ Right: Single clone, batch updates
let mut kb = base.clone();
for entry in entries {
    kb.insert(entry.path, entry.value);
}
results.push(kb);
```

---

## 9. Performance Tuning

### 9.1 Thread Pool Sizing

```rust
// Queries: Use all cores
let num_query_threads = num_cpus::get();

// Writers: Depends on partitioning
let num_writer_threads = min(num_partitions, num_cpus::get());

// Hybrid: Balance based on workload
let num_readers = (num_cpus::get() * 3) / 4;
let num_writers = num_cpus::get() / 4;
```

### 9.2 Batch Size Tuning

```rust
// Too small: High overhead
let batch_size = 1;  // ❌

// Too large: Poor load balancing
let batch_size = total_work;  // ❌

// Just right: ~1000-10000 items per batch
let batch_size = total_work / (num_threads * 10);  // ✅
```

---

## 10. Monitoring

### 10.1 Key Metrics

```rust
pub struct KBMetrics {
    query_latency_ms: Histogram,
    query_throughput: Counter,
    update_latency_ms: Histogram,
    clone_count: Counter,
    merge_latency_ms: Histogram,
    map_size_bytes: Gauge,
}
```

### 10.2 Performance Alerts

- Query latency > 100ms → Check map size, consider indexing
- Clone rate > 1000/sec → Consider batching
- Merge latency > 1s → Use parallel tree-reduce
- Allocator stalls → Enable jemalloc

---

## 11. Example: Complete Integration

See [examples/05_kb_query_engine.rs](examples/05_kb_query_engine.rs) for full implementation.

---

## References

- Pattern guides: [04_usage_pattern_read_only.md](04_usage_pattern_read_only.md) - [07_usage_pattern_hybrid.md](07_usage_pattern_hybrid.md)
- Performance: [08_performance_analysis.md](08_performance_analysis.md)
- Examples: [examples/](examples/)
