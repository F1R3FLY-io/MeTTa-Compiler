# Pattern D: Hybrid Read/Write

**Use Case**: Concurrent queries during knowledge base updates

**Characteristics**:
- Memory: Single PathMap + zipper state
- Synchronization: Path exclusivity for writers only
- Scalability: Excellent (readers scale linearly, writers partition-limited)
- Best for: Mixed read/write workloads

---

## 1. Basic Implementation

```rust
let mut kb = PathMap::<KnowledgeEntry>::new();
let zipper_head = kb.zipper_head();

thread::scope(|scope| {
    // Spawn reader threads (lock-free)
    for query_id in 0..num_readers {
        let reader = zipper_head.read_zipper_at_path(b"").unwrap();
        scope.spawn(move || {
            let mut rz = reader;
            execute_query(&mut rz, query_id);
        });
    }
    
    // Spawn writer threads (coordinated)
    for update_id in 0..num_writers {
        let partition = format!("updates_{}/", update_id).into_bytes();
        let writer = zipper_head
            .write_zipper_at_exclusive_path(partition)
            .unwrap();
        
        scope.spawn(move || {
            let mut wz = writer;
            apply_updates(&mut wz, update_id);
        });
    }
});
```

---

## 2. Consistency Model

**Readers see consistent snapshots**:
- Readers access structure at the time zipper was created
- Writers use COW to create new nodes
- Readers see old nodes (immutable)
- **Consistency**: Snapshot isolation (may be stale, never torn)

### 2.1 Example Timeline

```
Time  Reader                    Writer
  0   rz = read_zipper()        
      (gets reference to root)
  1   descend to "a/b"
  2                              wz.set_val("a/b/c", 42)
                                 (creates new nodes)
  3   read value at "a/b"
      (sees OLD value)
```

**Key**: Reader sees consistent state, even if stale

---

## 3. Update Visibility

**When are writes visible to new readers?**

```rust
// Writer updates path
wz.set_val_at(b"facts/new", entry);

// Existing readers: Don't see update (have old snapshot)
let old_reader = existing_zipper;
assert!(old_reader.get(b"facts/new").is_none());

// New readers: See update immediately
let new_reader = zipper_head.read_zipper_at_path(b"");
assert!(new_reader.get(b"facts/new").is_some());
```

**Takeaway**: New zippers see latest state, existing zippers see snapshot

---

## 4. MeTTaTron Integration

```rust
pub struct LiveKnowledgeBase {
    map: PathMap<KnowledgeEntry>,
}

impl LiveKnowledgeBase {
    pub fn query(&mut self) -> ReadZipperUntracked<KnowledgeEntry> {
        // Returns read zipper with current snapshot
        self.map.read_zipper()
    }
    
    pub fn update_partition(&mut self, partition: &str, updates: Vec<Update>) {
        let zipper_head = self.map.zipper_head();
        let path = format!("{}/", partition).into_bytes();
        
        if let Ok(mut wz) = zipper_head.write_zipper_at_exclusive_path(&path) {
            for update in updates {
                wz.descend_to(&update.path);
                wz.set_val(update.value);
                wz.reset();
            }
        }
    }
    
    pub fn query_during_update(&mut self, queries: Vec<Query>) -> Vec<Result> {
        let zipper_head = self.map.zipper_head();
        
        thread::scope(|scope| {
            // Readers
            let readers: Vec<_> = queries.iter().map(|query| {
                let rz = zipper_head.read_zipper_at_path(b"").unwrap();
                scope.spawn(move || execute_query(rz, query))
            }).collect();
            
            // Writer
            let writer = zipper_head
                .write_zipper_at_exclusive_path(b"updates/")
                .unwrap();
            scope.spawn(move || apply_background_updates(writer));
            
            // Collect results
            readers.into_iter()
                .map(|h| h.join().unwrap())
                .collect()
        })
    }
}
```

---

## 5. Performance Characteristics

**Read throughput**: Linear scaling (same as Pattern A)
**Write throughput**: Partition-limited (same as Pattern C)

**Best case**: Readers >> Writers, disjoint write paths
**Worst case**: Many writers to overlapping paths (conflicts)

---

## 6. Partitioning for Hybrid Workload

### 6.1 Separate Read/Write Domains

```rust
// Stable data (read-only)
let readers = spawn_readers(zipper_head, b"stable/");

// Mutable data (write partitions)
let writers = spawn_writers(zipper_head, b"mutable/", num_writers);
```

### 6.2 Hot/Cold Separation

```rust
// Hot data (frequently updated)
let hot_writers = spawn_writers(zipper_head, b"hot/", 4);

// Cold data (rarely updated, frequently read)
let cold_readers = spawn_readers(zipper_head, b"cold/");
```

---

## 7. Consistency Guarantees

**What hybrid pattern provides**:
- ✅ Readers see consistent snapshots (never torn reads)
- ✅ Writers see each other's updates (via path exclusivity)
- ✅ No data races
- ✅ No deadlocks

**What hybrid pattern does NOT provide**:
- ❌ Linearizability (readers may see stale data)
- ❌ Read-your-writes for new readers after update
- ❌ Causal consistency across partitions

**Consistency level**: **Snapshot isolation** for readers

---

## 8. When to Use

✅ **Use this pattern when**:
- Mixed read/write workload
- Reads can tolerate snapshot isolation
- Want maximum throughput
- Can partition writes

❌ **Don't use when**:
- Need linearizability
- Reads must see latest writes immediately
- Cannot partition writes

**Alternatives**:
- For read-only: [Pattern A](04_usage_pattern_read_only.md)
- For write-heavy: [Pattern C](06_usage_pattern_zipperhead.md)

---

## 9. Comparison Matrix

| Aspect | Pattern A | Pattern D |
|--------|-----------|-----------|
| **Reads** | ✅ Many | ✅ Many |
| **Writes** | ❌ None | ✅ Coordinated |
| **Memory** | 16B/thread | Single + zippers |
| **Scalability** | Linear (reads) | Linear (reads) + partition (writes) |

---

## References

- Pattern A: [Read-Only Sharing](04_usage_pattern_read_only.md)
- Pattern C: [ZipperHead](06_usage_pattern_zipperhead.md)
- Example: [examples/04_hybrid_read_write.rs](examples/04_hybrid_read_write.rs)
