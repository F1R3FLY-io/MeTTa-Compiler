# PathMap Persistence Overview

**Purpose**: High-level overview of all serialization formats and their trade-offs.

---

## 1. Why Custom Formats?

PathMap does **not** use Rust's standard Serde traits. Instead, it implements specialized binary formats optimized for trie structures.

### Reasons for Custom Formats

1. **Structural Sharing**: Preserve trie structure in serialized form
2. **Memory Mapping**: Enable mmap without deserialization
3. **Compression**: Built-in zlib-ng compression
4. **Zero-Copy**: Direct access to disk data
5. **Deduplication**: Reuse identical subtrees/path segments

**Source**: PathMap design prioritizes read performance and memory efficiency over standard serialization.

---

## 2. Three Serialization Formats

### Format 1: Paths Format (.paths)

**Location**: `src/paths_serialization.rs`

**Characteristics**:
- Serializes only paths (not trie structure)
- Built-in zlib-ng compression
- Supports any value type
- Mutable after deserialization

**Best for**:
- Change tracking/deltas
- Small to medium datasets
- Arbitrary value types

### Format 2: Arena Compact Tree (ACT)

**Location**: `src/arena_compact.rs`

**Characteristics**:
- Binary trie representation
- Memory-mappable (instant load)
- u64 values only
- Structural deduplication
- Read-only after creation

**Best for**:
- Large datasets (> 100 MB)
- Immutable data
- Instant loading required
- Larger-than-RAM datasets

### Format 3: Topo-DAG (Experimental)

**Location**: `src/serialization.rs`

**Characteristics**:
- Hex-encoded DAG structure
- JSON metadata
- Experimental status

**Best for**:
- Research/development
- Format exploration

---

## 3. Feature Comparison

| Feature | Paths | ACT | Topo-DAG |
|---------|-------|-----|----------|
| **Stable** | ✅ | ✅ | ⚠️ |
| **Compression** | ✅ zlib | ✅ Implicit | Partial |
| **Values** | Any | u64 | Any |
| **Mmap** | ❌ | ✅ | ❌ |
| **Mutable** | ✅ | ❌ | ❌ |
| **Lazy load** | ❌ | ✅ | ❌ |
| **Load time** | O(n) | O(1) | O(n) |
| **File size** | Smallest | Medium | Largest |

---

## 4. Performance Characteristics

### Serialization Time

- **Paths**: O(n×m) + compression overhead
- **ACT**: O(n) nodes + hash computation
- **Topo-DAG**: O(n) + Merkle computation

### Load Time

- **Paths**: O(n×m) - full deserialization
- **ACT**: O(1) - mmap only
- **Topo-DAG**: O(n) - full read

### Memory Usage

- **Paths after load**: Full PathMap in RAM
- **ACT**: Zero initially, OS page cache as accessed
- **Topo-DAG**: Full structure in RAM

### File Size (1M nodes, 10-byte avg paths)

- **Paths**: ~2-5 MB (compressed)
- **ACT**: ~15-20 MB (structural sharing)
- **Topo-DAG**: ~30-50 MB (hex-encoded)

---

## 5. Decision Matrix

**Use Paths Format when**:
- Dataset < 100 MB
- Need arbitrary value types
- Compression is priority
- Will modify after load
- Simple save/load workflow

**Use ACT Format when**:
- Dataset > 100 MB
- Need instant loading
- Data is read-only
- Values fit in u64
- May exceed RAM

**Use Topo-DAG when**:
- Experimenting with formats
- Need human-readable structure
- Research purposes

---

## 6. Code Examples

### Paths Format

```rust
use pathmap::paths_serialization::*;

// Save
serialize_paths(map.read_zipper(), &mut file)?;

// Load
deserialize_paths(map.write_zipper(), file, default_value)?;
```

### ACT Format

```rust
use pathmap::arena_compact::ArenaCompactTree;

// Save
ArenaCompactTree::dump_from_zipper(
    map.read_zipper(),
    |&v| v,  // Map to u64
    "data.tree"
)?;

// Load (mmap)
let act = ArenaCompactTree::open_mmap("data.tree")?;
let value = act.get_val_at(b"key");
```

---

## 7. Integration Recommendations

**For MeTTaTron**:

1. **Primary format**: ACT with external value store
2. **Backup format**: Paths for deltas/changes
3. **Workflow**: In-memory work + periodic ACT snapshots

**See**: [MeTTaTron Integration](07_mettaton_integration.md) for details

---

## References

- [Paths Format](02_paths_format.md)
- [ACT Format](03_act_format.md)
- [Mmap Operations](04_mmap_operations.md)
