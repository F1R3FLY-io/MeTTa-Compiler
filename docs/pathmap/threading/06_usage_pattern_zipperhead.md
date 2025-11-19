# Pattern C: ZipperHead Coordination

**Use Case**: Coordinated parallel writes to single PathMap

**Characteristics**:
- Memory: Single PathMap + zipper state (minimal)
- Synchronization: Path exclusivity checking
- Scalability: Excellent with good partitioning
- Best for: Parallel knowledge base construction

---

## 1. Basic Implementation

```rust
let mut kb = PathMap::<KnowledgeEntry>::new();
let zipper_head = kb.zipper_head();

thread::scope(|scope| {
    for partition_id in 0..num_threads {
        let path = format!("partition_{}", partition_id).into_bytes();
        
        // Get exclusive write zipper
        let writer = zipper_head
            .write_zipper_at_exclusive_path(&path)
            .expect("Partitions are exclusive");
        
        scope.spawn(move || {
            let mut wz = writer;
            // Write to this partition
            for entry in get_partition_data(partition_id) {
                wz.descend_to(&entry.path);
                wz.set_val(entry.value);
                wz.reset();  // Back to partition root
            }
        });
    }
});
// kb now contains all data from all threads
```

---

## 2. Path Exclusivity

### 2.1 Safe API (Recommended)

```rust
// Returns Result, checks for conflicts
match zipper_head.write_zipper_at_exclusive_path(path) {
    Ok(wz) => {
        // Path is exclusive, safe to write
    }
    Err(PathConflict) => {
        // Path overlaps with existing zipper
        eprintln!("Conflict detected!");
    }
}
```

### 2.2 Unsafe API (Performance)

```rust
// No runtime check - caller guarantees exclusivity
let wz = unsafe {
    zipper_head.write_zipper_at_exclusive_path_unchecked(path)
};
// SAFETY: Must ensure no overlapping paths manually
```

**Use unsafe API only when**:
- Paths are provably disjoint (e.g., numeric prefixes)
- Performance is critical
- Runtime checks are bottleneck

---

## 3. Partitioning Strategies

### 3.1 Prefix Partitioning

```rust
// Partition by first byte
for thread_id in 0..256 {
    let prefix = vec![thread_id as u8];
    let wz = zipper_head.write_zipper_at_exclusive_path(&prefix)?;
    // Each thread writes to keys starting with its assigned byte
}
```

### 3.2 Hash Partitioning

```rust
fn partition_path(key: &[u8], num_partitions: usize) -> Vec<u8> {
    let hash = hash_function(key);
    let partition_id = hash % num_partitions;
    format!("p{}/", partition_id).into_bytes()
}

// Thread writes to partition determined by hash
let partition = partition_path(key, num_threads);
let wz = zipper_head.write_zipper_at_exclusive_path(&partition)?;
```

### 3.3 Domain Partitioning

```rust
// Partition by semantic domain
let domains = vec![
    b"facts/math/".to_vec(),
    b"facts/logic/".to_vec(),
    b"facts/physics/".to_vec(),
];

for (thread_id, domain) in domains.iter().enumerate() {
    let wz = zipper_head.write_zipper_at_exclusive_path(domain)?;
    // Thread writes domain-specific data
}
```

---

## 4. MeTTaTron Integration

```rust
pub fn parallel_kb_construction(
    data: Vec<DataPartition>
) -> PathMap<KnowledgeEntry> {
    let mut kb = PathMap::new();
    let zipper_head = kb.zipper_head();
    
    thread::scope(|scope| {
        let handles: Vec<_> = data.into_iter()
            .enumerate()
            .map(|(idx, partition)| {
                let path = format!("p{}/", idx).into_bytes();
                let writer = zipper_head
                    .write_zipper_at_exclusive_path(path)
                    .expect("Partitions are exclusive by construction");
                
                scope.spawn(move || {
                    let mut wz = writer;
                    for entry in partition.entries {
                        wz.descend_to(&entry.path);
                        wz.set_val(entry.value);
                        wz.reset();
                    }
                })
            })
            .collect();
        
        for handle in handles {
            handle.join().unwrap();
        }
    });
    
    kb
}
```

---

## 5. Performance Characteristics

**From benchmarks** (`benches/parallel.rs:108-177`):

With jemalloc:
- 1 thread: 1.0× (baseline)
- 2 threads: 1.8×
- 4 threads: 3.2×
- 8 threads: 5.6×

**Bottleneck**: Allocator contention (use jemalloc)

**Scalability**: Limited by partition quality and allocator

---

## 6. Comparison with Clone+Merge

| Aspect | ZipperHead | Clone+Merge |
|--------|------------|-------------|
| **Memory** | Single map | N clones |
| **Merge cost** | Zero | O(N × K) |
| **Visibility** | Immediate | After merge |
| **Scalability** | Partition-dependent | Excellent |

**Rule of thumb**: Use ZipperHead when merge cost > coordination overhead

---

## 7. When to Use

✅ **Use this pattern when**:
- Building single shared structure
- Can partition workload by path
- Need immediate visibility
- Want to avoid merge cost

❌ **Don't use when**:
- Workload doesn't partition well
- Frequent path conflicts
- Prefer simplicity over performance

**Alternative**: For unpartitionable workloads, see [Clone+Merge](05_usage_pattern_clone_merge.md)

---

## References

- Benchmark: `benches/parallel.rs:108-177`
- Source: `src/zipper_head.rs`
- Example: [examples/03_zipperhead_parallel.rs](examples/03_zipperhead_parallel.rs)
- Allocator: `../../optimization/PATHMAP_JEMALLOC_ANALYSIS.md`
