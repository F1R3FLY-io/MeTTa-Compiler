# PathMap Persistence & Disk Operations

**Purpose**: Comprehensive guide to PathMap's serialization formats, memory-mapped operations, and persistent storage strategies for MeTTaTron integration.

**Version**: 1.0
**Last Updated**: 2025-01-13
**PathMap Version**: 0.2.0-alpha0

---

## Overview

PathMap provides **production-ready disk-based operations** through custom binary serialization formats optimized for trie structures. Unlike standard Rust collections, PathMap doesn't use Serde but implements specialized formats that preserve structural sharing and enable memory-mapped access.

### Key Capabilities

- ✅ **Three serialization formats** (paths, ACT, topo-DAG)
- ✅ **Memory-mapped files** for instant loading (O(1) time)
- ✅ **Larger-than-RAM** datasets via OS page cache
- ✅ **Built-in compression** (zlib-ng)
- ✅ **Zero-copy reads** from disk
- ✅ **Structural deduplication** to minimize file size

### What This Is NOT

- ❌ **Not a database** - No ACID transactions, no SQL
- ❌ **Not Serde-based** - Custom binary formats only
- ❌ **Not incrementally updatable** - Must reserialize for changes
- ❌ **Not for all value types** - ACT format limited to u64 values

---

## Quick Start

### Basic Serialization (Paths Format)

```rust
use pathmap::PathMap;
use pathmap::paths_serialization::*;
use std::fs::File;

// Save
let mut map = PathMap::new();
map.set_val_at(b"key1", "value1");
map.set_val_at(b"key2", "value2");

let mut file = File::create("data.paths")?;
serialize_paths(map.read_zipper(), &mut file)?;

// Load
let mut restored = PathMap::new();
let file = File::open("data.paths")?;
deserialize_paths(restored.write_zipper(), file, "default")?;
```

### Memory-Mapped Loading (ACT Format)

```rust
use pathmap::arena_compact::ArenaCompactTree;

// Save (one time)
let map = PathMap::from_iter([
    (b"path1", 100u64),
    (b"path2", 200u64),
]);
ArenaCompactTree::dump_from_zipper(
    map.read_zipper(),
    |&v| v,
    "data.tree"
)?;

// Load instantly (O(1) - no actual file reading!)
let act = ArenaCompactTree::open_mmap("data.tree")?;
let value = act.get_val_at(b"path1");  // Lazy loading, zero-copy
assert_eq!(value, Some(100));
```

---

## Table of Contents

### Core Concepts

1. **[Overview](01_overview.md)** (~2,000 words)
   - Format comparison matrix
   - Serde vs custom formats
   - When to use each format
   - Feature capabilities

2. **[Paths Format](02_paths_format.md)** (~2,000 words)
   - Compressed path serialization
   - zlib-ng compression
   - API reference
   - Use cases (deltas, change tracking)

3. **[ACT Format](03_act_format.md)** (~2,500 words)
   - Arena Compact Tree specification
   - Binary format (ACTree03)
   - Node encoding (varint, line data)
   - Structural deduplication

4. **[Memory-Mapped Operations](04_mmap_operations.md)** (~2,000 words)
   - mmap API usage
   - OS page cache mechanics
   - Lazy loading behavior
   - Large file handling (> RAM)

5. **[Value Encoding](05_value_encoding.md)** (~2,500 words)
   - u64 limitation workarounds
   - Direct encoding strategies
   - External value store patterns
   - Content-addressed storage
   - MeTTa term encoding

### Performance & Integration

6. **[Performance Analysis](06_performance_analysis.md)** (~2,000 words)
   - Serialization complexity proofs
   - Load time analysis (O(1) for mmap)
   - Memory overhead
   - Compression ratios
   - Benchmark results

7. **[MeTTaTron Integration](07_mettaton_integration.md)** (~2,000 words)
   - Recommended patterns
   - Compilation artifact storage
   - Knowledge base snapshots
   - Incremental development workflow
   - Complete examples

---

## Format Comparison Matrix

| Feature | Paths Format | ACT Format | Topo-DAG |
|---------|-------------|------------|----------|
| **Compression** | ✅ zlib-ng | ✅ Implicit | ✅ Optional |
| **Value types** | Any | u64 only | Any |
| **Memory-mapped** | ❌ | ✅ | ❌ |
| **Load time** | O(n) deserialize | O(1) mmap | O(n) |
| **File size** | Small (compressed) | Medium | Large |
| **Read-only** | ❌ Mutable | ✅ Immutable | ✅ Immutable |
| **Lazy loading** | ❌ | ✅ | ❌ |
| **Zero-copy** | ❌ | ✅ | ❌ |
| **Larger than RAM** | ❌ | ✅ | ❌ |
| **Use case** | Deltas, changes | Large immutable data | Experimental |
| **Status** | ✅ Stable | ✅ Stable | ⚠️ Experimental |

---

## Decision Tree

### When to Use Paths Format

✅ **Use when:**
- Need to store arbitrary value types
- Files are small (< 100 MB)
- Need to load into mutable PathMap
- Tracking changes/deltas
- Compression is important

❌ **Don't use when:**
- Files are very large (> 1 GB)
- Need instant loading
- Want zero-copy access
- Operating on larger-than-RAM datasets

### When to Use ACT Format

✅ **Use when:**
- Files are large (> 100 MB)
- Need instant loading (O(1))
- Data is read-only after creation
- Want zero-copy access
- Operating on larger-than-RAM datasets
- Can map values to u64

❌ **Don't use when:**
- Need to store complex value types directly
- Need to modify after serialization
- Values don't fit in u64

### Recommended for MeTTaTron

**Primary**: ACT format with external value store
- Instant loading of compiled knowledge bases
- Memory-efficient for large corpora
- u64 as index into separate value file

**Secondary**: Paths format for incremental changes
- Track deltas between versions
- Small compressed files
- Easy to merge

---

## Examples

All examples are complete, runnable Rust programs:

| Example | Description | Pattern | Lines |
|---------|-------------|---------|-------|
| [01_basic_serialization.rs](examples/01_basic_serialization.rs) | Save/load with both formats | Basic I/O | ~150 |
| [02_mmap_loading.rs](examples/02_mmap_loading.rs) | Memory-mapped file usage | mmap | ~120 |
| [03_value_store.rs](examples/03_value_store.rs) | External value storage | Encoding | ~180 |
| [04_content_addressed.rs](examples/04_content_addressed.rs) | Hash-based deduplication | Optimization | ~200 |
| [05_incremental_snapshots.rs](examples/05_incremental_snapshots.rs) | Snapshot-based workflow | Versioning | ~160 |
| [06_hybrid_persistence.rs](examples/06_hybrid_persistence.rs) | In-memory + periodic saves | Production | ~220 |

**Usage**: Copy relevant examples into MeTTaTron source and adapt to your types.

---

## Benchmarks

Performance validation suite using Criterion:

| Benchmark | Purpose | Validation |
|-----------|---------|------------|
| [serialization_performance.rs](benchmarks/serialization_performance.rs) | Serialize/deserialize speed | Time complexity |
| [mmap_vs_memory.rs](benchmarks/mmap_vs_memory.rs) | Load time comparison | O(1) mmap proof |
| [compression_overhead.rs](benchmarks/compression_overhead.rs) | Compression impact | Throughput cost |
| [query_performance.rs](benchmarks/query_performance.rs) | Query on disk-backed | Access patterns |

**Running benchmarks**:
```bash
# Copy benchmark to benches/ directory
cp benchmarks/mmap_vs_memory.rs /path/to/project/benches/

# Add to Cargo.toml:
[[bench]]
name = "mmap_vs_memory"
harness = false

# Run
cargo bench --bench mmap_vs_memory
```

---

## Integration Checklist

- [ ] **Choose format** based on decision tree
- [ ] **Enable features** in Cargo.toml
  ```toml
  pathmap = { version = "0.2", features = ["arena_compact", "serialization"] }
  ```
- [ ] **Design value encoding** strategy if using ACT
- [ ] **Implement serialization** for your types
- [ ] **Test with realistic data** sizes
- [ ] **Benchmark** load times and query performance
- [ ] **Plan for versioning** (format changes)
- [ ] **Handle errors** gracefully (corrupted files, etc.)
- [ ] **Document** file formats for your team

---

## Key Limitations

### ACT Format

1. **u64 values only** - Complex types need external storage or encoding
2. **Read-only** - Cannot modify after serialization
3. **No incremental updates** - Must reserialize entire (sub)tree
4. **Platform-specific** - Binary format, not portable across endianness

### Paths Format

1. **Full deserialization** - Must load entire map into memory
2. **No lazy loading** - Can't operate on larger-than-RAM datasets
3. **No zero-copy** - Creates new PathMap in memory

### General

1. **No Serde support** - Can't use with serde-based tools
2. **No ACID transactions** - No guarantees across crashes
3. **No write-ahead log** - No incremental recovery
4. **Version compatibility** - Format changes may break old files

---

## Performance Summary

### Serialization

| Operation | Paths Format | ACT Format |
|-----------|-------------|------------|
| **Serialize** | O(n×m) + compression | O(n) nodes |
| **Deserialize** | O(n×m) + decompression | N/A (mmap) |
| **Load time** | Seconds (large maps) | Instant (mmap) |
| **File size** | Smallest (compressed) | Medium (structural sharing) |

### Query Performance

| Operation | In-Memory | ACT mmap |
|-----------|-----------|----------|
| **Point query** | O(log n) | O(log n) + page faults |
| **Traversal** | O(n) | O(n) + page faults |
| **First query** | Fast | Page fault overhead |
| **Subsequent** | Fast | Fast (cached) |

### Memory Usage

| Scenario | In-Memory | ACT mmap |
|----------|-----------|----------|
| **1 GB dataset** | 1 GB RAM | ~0 MB initially |
| **After queries** | 1 GB RAM | OS page cache (variable) |
| **Multiple processes** | 1 GB × processes | Shared pages |

---

## Common Patterns

### Pattern 1: Compilation Artifacts

```rust
// Compile once
let kb = compile_metta_to_pathmap(source)?;
ArenaCompactTree::dump_from_zipper(
    kb.read_zipper(),
    |term| encode_term(term),
    "compiled.tree"
)?;

// Use many times (instant load)
let kb = ArenaCompactTree::open_mmap("compiled.tree")?;
```

### Pattern 2: Incremental Snapshots

```rust
// Periodic snapshots
let snapshot_v1 = create_snapshot(&kb)?;
save_act("v1.tree", &snapshot_v1)?;

// Track deltas
let delta = compute_delta(&snapshot_v1, &current_kb);
save_paths("delta_v2.paths", &delta)?;

// Reconstruct
let restored = snapshot_v1.join(&delta);
```

### Pattern 3: Hybrid In-Memory + Disk

```rust
// Working set in memory
let mut working_kb = PathMap::new();

// Periodic saves
thread::spawn(move || {
    loop {
        sleep(Duration::from_secs(300));
        save_snapshot(&working_kb)?;
    }
});
```

---

## Troubleshooting

### Issue: "Invalid file magic"

**Cause**: Trying to open file with wrong format or corrupted file

**Solution**: Verify file was created with correct format (ACTree03 for ACT)

### Issue: Memory-mapped file too slow

**Cause**: Cold page cache, lots of page faults

**Solution**:
- First query will be slow (OS loads pages)
- Subsequent queries fast
- Consider `madvise()` hints if supported

### Issue: Cannot serialize complex values

**Cause**: ACT format only supports u64

**Solution**: See [Value Encoding](05_value_encoding.md) for strategies

### Issue: File too large

**Cause**: No structural deduplication or compression

**Solution**:
- Use `merkleize()` before serialization
- Use paths format with compression
- Consider external value storage

---

## References

### PathMap Source Code

- **Paths serialization**: `src/paths_serialization.rs`
- **ACT format**: `src/arena_compact.rs`
- **Topo-DAG**: `src/serialization.rs`
- **Benchmarks**: `benches/serde.rs`
- **Examples**: `examples/arena_compact_tests/`

### Related Documentation

- **Threading**: `../threading/README.md`
- **Algebraic Operations**: `../PATHMAP_ALGEBRAIC_OPERATIONS.md`
- **COW Analysis**: `../PATHMAP_COW_ANALYSIS.md`
- **jemalloc**: `../../optimization/PATHMAP_JEMALLOC_ANALYSIS.md`

### External Resources

- **memmap2 crate**: https://docs.rs/memmap2/
- **zlib-ng**: https://github.com/zlib-ng/zlib-ng
- **PathMap repository**: https://github.com/Bitseat/PathMap

---

## Next Steps

1. **Read format docs**: Start with [Overview](01_overview.md) for format comparison
2. **Choose format**: Use decision tree above
3. **Review examples**: See [examples/](examples/) for patterns
4. **Test integration**: Follow [MeTTaTron Integration](07_mettaton_integration.md)
5. **Benchmark**: Use provided benchmarks to validate performance

---

**Ready to get started?** → [Overview](01_overview.md)
