# PathMap Multi-Threading Documentation

**Purpose**: Comprehensive guide to PathMap's multi-threading capabilities, concurrent access patterns, and optimal usage strategies for MeTTaTron integration.

**Version**: 1.0
**Last Updated**: 2025-01-13
**PathMap Version**: Latest (as of documentation date)

---

## Overview

PathMap is a **multi-threaded data structure by design**, not merely thread-safe. It provides:

- ✅ **Arc-like atomic reference counting** for thread-safe structural sharing
- ✅ **Lock-free concurrent reads** from shared structure
- ✅ **Coordinated parallel writes** via ZipperHead
- ✅ **O(1) cloning** with copy-on-write (COW) semantics
- ✅ **Zero data races** guaranteed by type system + atomic operations

This documentation provides rigorous analysis, formal proofs, practical examples, and performance benchmarks for multi-threaded PathMap usage.

---

## Quick Start

**Most Common Use Case: Read-Heavy Knowledge Base**

```rust
use std::sync::Arc;
use pathmap::PathMap;

// Share PathMap across threads for concurrent queries
let kb = Arc::new(PathMap::<KnowledgeEntry>::new());

// Spawn query threads
for query_id in 0..num_threads {
    let kb_ref = Arc::clone(&kb);
    thread::spawn(move || {
        let zipper = kb_ref.read_zipper();
        // Execute queries - no locks, fully parallel
        execute_query(&zipper, query_id);
    });
}
```

**For detailed patterns, see**: [Usage Patterns](#usage-patterns)

---

## Table of Contents

### Core Concepts

1. **[Threading Model](01_threading_model.md)** (~3,000 words)
   - Send/Sync trait implementations
   - TrieValue trait bounds
   - Design philosophy and guarantees
   - Thread safety mechanisms

2. **[Reference Counting](02_reference_counting.md)** (~4,000 words)
   - Arc-based implementation (default)
   - slim_ptrs atomic implementation
   - Memory ordering analysis (Relaxed/Release/Acquire)
   - Performance characteristics

3. **[Concurrent Access Patterns](03_concurrent_access_patterns.md)** (~4,000 words)
   - Multiple threads reading simultaneously
   - Independent modifications via cloning
   - ZipperHead coordination for parallel writes
   - Race condition analysis and safety proofs

### Usage Patterns

4. **[Pattern A: Read-Only Sharing](04_usage_pattern_read_only.md)** (~2,000 words)
   - `Arc<PathMap>` for read-heavy workloads
   - Lock-free concurrent queries
   - Memory overhead: 16 bytes per thread
   - **Use case**: Knowledge base queries

5. **[Pattern B: Clone + Modify + Merge](05_usage_pattern_clone_merge.md)** (~2,000 words)
   - Independent computation with structural sharing
   - COW isolation guarantees
   - Merge strategies (join, meet, subtract)
   - **Use case**: Parallel reasoning with independent results

6. **[Pattern C: ZipperHead Coordination](06_usage_pattern_zipperhead.md)** (~2,000 words)
   - Coordinated parallel updates
   - Path exclusivity enforcement
   - Safe and unsafe APIs
   - **Use case**: Parallel knowledge base construction

7. **[Pattern D: Hybrid Read/Write](07_usage_pattern_hybrid.md)** (~2,000 words)
   - Concurrent reads during writes
   - Mixed workload optimization
   - Lock-free reads + exclusive writes
   - **Use case**: Queries during knowledge base updates

### Performance & Integration

8. **[Performance Analysis](08_performance_analysis.md)** (~3,000 words)
   - Clone operation complexity proofs
   - Atomic operations overhead
   - Structural sharing efficiency
   - Scalability characteristics
   - Allocator bottleneck analysis

9. **[MeTTaTron Integration](09_mettaton_integration.md)** (~3,000 words)
   - Knowledge base access patterns
   - Parallel reasoning implementations
   - Concurrent KB construction
   - Trade-offs decision matrix
   - Best practices

10. **[Formal Proofs](10_formal_proofs.md)** (~3,000 words)
    - **Theorem 10.1**: Data race freedom
    - **Theorem 10.2**: Clone O(1) complexity
    - **Theorem 10.3**: Structural sharing correctness
    - **Theorem 10.4**: Memory ordering safety
    - **Theorem 10.5**: ZipperHead path exclusivity

---

## Practical Examples

All examples are complete, runnable Rust programs demonstrating specific patterns:

| Example | Description | Pattern | Lines |
|---------|-------------|---------|-------|
| [01_read_only_sharing.rs](examples/01_read_only_sharing.rs) | Arc<PathMap> with concurrent queries | A | ~100 |
| [02_clone_per_thread.rs](examples/02_clone_per_thread.rs) | Independent reasoning with merge | B | ~120 |
| [03_zipperhead_parallel.rs](examples/03_zipperhead_parallel.rs) | Coordinated parallel inserts | C | ~130 |
| [04_hybrid_read_write.rs](examples/04_hybrid_read_write.rs) | Queries during updates | D | ~140 |
| [05_kb_query_engine.rs](examples/05_kb_query_engine.rs) | MeTTaTron query system | A | ~200 |
| [06_parallel_reasoning.rs](examples/06_parallel_reasoning.rs) | Multi-threaded inference | B | ~250 |
| [07_concurrent_construction.rs](examples/07_concurrent_construction.rs) | Parallel data loading | C | ~180 |
| [08_versioned_kb.rs](examples/08_versioned_kb.rs) | Clone-based versioning | B | ~160 |
| [09_distributed_workers.rs](examples/09_distributed_workers.rs) | Channel-based work distribution | C | ~220 |
| [10_lockfree_updates.rs](examples/10_lockfree_updates.rs) | Concurrent update queue | B | ~190 |

**Usage**: Copy relevant examples into MeTTaTron source code and adapt to your types.

---

## Performance Benchmarks

Comprehensive benchmark suite validating performance claims:

| Benchmark | Purpose | Validation |
|-----------|---------|------------|
| [clone_performance.rs](benchmarks/clone_performance.rs) | Clone overhead vs map size | Confirms O(1) |
| [concurrent_reads.rs](benchmarks/concurrent_reads.rs) | Read scalability vs threads | Tests 1-128 threads |
| [zipperhead_overhead.rs](benchmarks/zipperhead_overhead.rs) | Coordination cost | Path conflict detection |
| [pattern_comparison.rs](benchmarks/pattern_comparison.rs) | Pattern A vs B vs C | Crossover analysis |
| [atomic_operations.rs](benchmarks/atomic_operations.rs) | Refcount overhead | Memory ordering impact |
| [memory_usage.rs](benchmarks/memory_usage.rs) | Structural sharing | Requires jemalloc |
| [allocator_comparison.rs](benchmarks/allocator_comparison.rs) | System vs jemalloc | Write contention |

**Running benchmarks**:
```bash
# Copy benchmark file to benches/ directory
cp benchmarks/concurrent_reads.rs /path/to/project/benches/

# Add to Cargo.toml:
[[bench]]
name = "concurrent_reads"
harness = false

# Run with CPU affinity and max frequency
cargo bench --bench concurrent_reads
```

---

## Quick Decision Matrix

**Which pattern should I use?**

| Workload | Pattern | Rationale |
|----------|---------|-----------|
| Read-only queries | **A: Arc<PathMap>** | Zero-copy sharing, lock-free |
| Independent reasoning threads | **B: Clone+Merge** | No synchronization during compute |
| Parallel KB construction | **C: ZipperHead** | Updates immediately visible |
| Queries during updates | **D: Hybrid** | Lock-free reads + exclusive writes |
| Small batch updates | **B: Clone-modify-swap** | Simple, low overhead |
| Large batch updates | **C: ZipperHead** | Avoids final merge cost |

**Memory overhead comparison**:
- Pattern A: 16 bytes per thread (Arc pointer)
- Pattern B: Shared structure + per-thread deltas (COW)
- Pattern C: Single PathMap + zipper state
- Pattern D: Single PathMap + zipper state

**See**: [MeTTaTron Integration Guide](09_mettaton_integration.md) for detailed decision criteria.

---

## Key Findings Summary

### Threading Capabilities

1. **Reference Counting**: Arc-like atomic (both default and slim_ptrs modes)
   - Source: `src/trie_node.rs:2306-2769`
   - Memory ordering: Relaxed increment, Release/Acquire drop

2. **Clone Operation**: O(1) complexity with structural sharing
   - Source: `src/trie_map.rs:39-45`
   - Proof: [Theorem 10.2](10_formal_proofs.md#theorem-102)

3. **Concurrent Reads**: Fully lock-free, scales linearly with threads
   - Evidence: `benches/parallel.rs` (tested up to 256 threads)
   - Benchmark: [concurrent_reads.rs](benchmarks/concurrent_reads.rs)

4. **Concurrent Writes**: Via ZipperHead coordination or independent clones
   - Path exclusivity enforced at compile-time + runtime
   - Proof: [Theorem 10.5](10_formal_proofs.md#theorem-105)

5. **Data Race Freedom**: Guaranteed by Send/Sync + atomic operations
   - Proof: [Theorem 10.1](10_formal_proofs.md#theorem-101)

### Performance Characteristics

| Operation | Complexity | Synchronization | Scalability |
|-----------|-----------|-----------------|-------------|
| Clone | O(1) | Atomic increment | Perfect |
| Read (concurrent) | O(path length) | None | Linear w/ threads |
| Write (disjoint paths) | O(path length) | Path check only | Partition-dependent |
| Merge (join/meet) | O(n + m) | None | N/A (sequential) |

**Bottleneck**: Allocator, not atomic operations
- **Solution**: Enable `jemalloc` feature for write-heavy workloads
- **Impact**: 2-4× speedup in parallel write benchmarks

---

## Limitations and Caveats

1. **No overlapping writes**: Multiple threads cannot write to overlapping paths simultaneously
   - **Mitigation**: Use ZipperHead coordination or separate clones

2. **Allocator contention**: System allocator bottlenecks under heavy parallel writes
   - **Mitigation**: Enable `jemalloc` feature in Cargo.toml

3. **No weak references**: TrieNodeODRc doesn't support weak pointers
   - **Implication**: Cyclic structures will leak memory

4. **No RWLock included**: PathMap doesn't provide built-in read-write lock
   - **Design**: Structural sharing preferred over locking
   - **Alternative**: Wrap in `Arc<RwLock<PathMap>>` if needed (sacrifices lock-free reads)

**See**: Each pattern document discusses specific limitations.

---

## Integration Checklist for MeTTaTron

- [ ] **Choose primary pattern** based on workload (see decision matrix)
- [ ] **Enable jemalloc** in Cargo.toml if write-heavy
- [ ] **Implement KnowledgeEntry** with `Clone + Send + Sync + Unpin + 'static`
- [ ] **Design path schema** for ZipperHead partitioning (if using Pattern C)
- [ ] **Benchmark** with realistic data and thread counts
- [ ] **Profile allocator** to confirm jemalloc benefit
- [ ] **Test** concurrent access patterns with ThreadSanitizer
- [ ] **Review** formal proofs for safety guarantees

**See**: [MeTTaTron Integration Guide](09_mettaton_integration.md) for step-by-step implementation.

---

## Source Code References

All claims in this documentation are supported by specific source code references:

**PathMap Repository**: `/home/dylon/Workspace/f1r3fly.io/PathMap/`

| Topic | File | Lines | Description |
|-------|------|-------|-------------|
| Send/Sync | `src/trie_map.rs` | 36-37 | Trait implementations |
| Clone impl | `src/trie_map.rs` | 39-45 | O(1) structural sharing |
| Arc refcount | `src/trie_node.rs` | 2306-2427 | Default implementation |
| slim_ptrs | `src/trie_node.rs` | 2432-2769 | Optimized atomic |
| TrieValue | `src/lib.rs` | 151-153 | Send+Sync requirement |
| Parallel benches | `benches/parallel.rs` | 1-443 | Up to 256 threads |
| ZipperHead | `src/zipper_head.rs` | Full file | Coordination API |

**See individual documents for detailed references.**

---

## Additional Resources

- **PathMap Main Documentation**: `/home/dylon/Workspace/f1r3fly.io/PathMap/README.md`
- **PathMap Book**: `/home/dylon/Workspace/f1r3fly.io/PathMap/pathmap-book/`
- **Algebraic Operations**: `../PATHMAP_ALGEBRAIC_OPERATIONS.md`
- **Copy-on-Write Analysis**: `../PATHMAP_COW_ANALYSIS.md`
- **jemalloc Analysis**: `../../optimization/PATHMAP_JEMALLOC_ANALYSIS.md`

---

## Contributing

When adding new threading patterns or examples:

1. Follow existing naming conventions (e.g., `NN_descriptive_name.{md,rs}`)
2. Include formal complexity analysis
3. Provide source code references (file:line)
4. Add benchmarks validating claims
5. Update this README with links and decision matrix
6. Ensure all code is complete and runnable

---

## Version History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2025-01-13 | Initial comprehensive threading documentation |

---

## Contact

For questions about PathMap threading or MeTTaTron integration, refer to:
- PathMap issues: https://github.com/Bitseat/PathMap/issues
- MeTTaTron documentation: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/docs/`

---

**Next Steps**: Start with [Threading Model](01_threading_model.md) for foundational concepts, or jump directly to [Usage Patterns](#usage-patterns) for practical implementation guidance.
