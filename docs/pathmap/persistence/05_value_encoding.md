# Value Encoding Strategies

**Purpose**: Comprehensive guide to encoding complex values for ACT format's u64 limitation.

**Problem**: ACT format stores only u64 values, but applications often need richer types (strings, structs, MeTTa terms, etc.)

**Solution**: This document presents three encoding strategies with complete examples.

---

## 1. The u64 Limitation

### Why u64 Only?

**Design decision**: ACT format prioritizes:
1. **Simplicity**: Fixed-size values (8 bytes)
2. **Performance**: No deserialization overhead
3. **Memory efficiency**: Compact representation
4. **Zero-copy**: Direct memory access

**Source**: `src/arena_compact.rs` design

### What This Means

**Direct storage** (not possible):
```rust
// ❌ ACT cannot store these directly
struct ComplexValue {
    name: String,
    data: Vec<u8>,
    metadata: HashMap<String, String>,
}

let map: PathMap<ComplexValue> = create_map();
ArenaCompactTree::dump_from_zipper(
    map.read_zipper(),
    |v| ???,  // Cannot convert ComplexValue → u64
    "output.tree"
)?;
```

**Encoding required**:
```rust
// ✅ Must encode as u64
let encoded_map: PathMap<u64> = encode_values(original_map);
ArenaCompactTree::dump_from_zipper(
    encoded_map.read_zipper(),
    |&v| v,
    "output.tree"
)?;
```

---

## 2. Strategy 1: Direct Encoding

**Concept**: Encode value directly into u64 bits

**Applicable to**:
- Small integers
- Enums with ≤ 2^64 variants
- Flags/bitmasks
- Compact data structures

### Example 1A: Enum Encoding

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum NodeType {
    Function,
    Variable,
    Constant,
    Expression,
}

impl NodeType {
    fn encode(&self) -> u64 {
        match self {
            NodeType::Function => 0,
            NodeType::Variable => 1,
            NodeType::Constant => 2,
            NodeType::Expression => 3,
        }
    }

    fn decode(value: u64) -> Option<Self> {
        match value {
            0 => Some(NodeType::Function),
            1 => Some(NodeType::Variable),
            2 => Some(NodeType::Constant),
            3 => Some(NodeType::Expression),
            _ => None,
        }
    }
}

// Usage
let mut map: PathMap<NodeType> = PathMap::new();
map.set_val_at(b"ast/node1", NodeType::Function);
map.set_val_at(b"ast/node2", NodeType::Variable);

// Encode for ACT
let encoded: PathMap<u64> = PathMap::new();
for (path, &node_type) in map.iter() {
    encoded.set_val_at(path, node_type.encode());
}

ArenaCompactTree::dump_from_zipper(
    encoded.read_zipper(),
    |&v| v,
    "ast.tree"
)?;

// Decode when querying
let act = ArenaCompactTree::open_mmap("ast.tree")?;
let encoded_value = act.get_val_at(b"ast/node1").unwrap();
let node_type = NodeType::decode(encoded_value).unwrap();
assert_eq!(node_type, NodeType::Function);
```

### Example 1B: Packed Struct

```rust
#[derive(Debug, Clone, Copy)]
struct CompactValue {
    id: u32,        // 32 bits
    flags: u16,     // 16 bits
    category: u8,   // 8 bits
    priority: u8,   // 8 bits
}

impl CompactValue {
    fn encode(&self) -> u64 {
        (self.id as u64) << 32
            | (self.flags as u64) << 16
            | (self.category as u64) << 8
            | (self.priority as u64)
    }

    fn decode(value: u64) -> Self {
        CompactValue {
            id: (value >> 32) as u32,
            flags: (value >> 16) as u16,
            category: (value >> 8) as u8,
            priority: value as u8,
        }
    }
}

// Usage
let value = CompactValue {
    id: 12345,
    flags: 0b1010_1111_0000_1111,
    category: 42,
    priority: 7,
};

let mut map: PathMap<u64> = PathMap::new();
map.set_val_at(b"item/1", value.encode());

ArenaCompactTree::dump_from_zipper(
    map.read_zipper(),
    |&v| v,
    "items.tree"
)?;

// Decode
let act = ArenaCompactTree::open_mmap("items.tree")?;
let encoded = act.get_val_at(b"item/1").unwrap();
let decoded = CompactValue::decode(encoded);
assert_eq!(decoded.id, 12345);
```

### Example 1C: IEEE 754 Float Encoding

```rust
fn encode_f64(value: f64) -> u64 {
    value.to_bits()
}

fn decode_f64(value: u64) -> f64 {
    f64::from_bits(value)
}

// Usage
let mut map: PathMap<u64> = PathMap::new();
map.set_val_at(b"metrics/latency", encode_f64(45.7));
map.set_val_at(b"metrics/throughput", encode_f64(1234.56));

ArenaCompactTree::dump_from_zipper(
    map.read_zipper(),
    |&v| v,
    "metrics.tree"
)?;

// Decode
let act = ArenaCompactTree::open_mmap("metrics.tree")?;
let latency = decode_f64(act.get_val_at(b"metrics/latency").unwrap());
assert!((latency - 45.7).abs() < 1e-10);
```

### Limitations of Direct Encoding

**Maximum size**: 8 bytes
**Not suitable for**:
- Strings
- Variable-length data
- Nested structures
- Large objects

---

## 3. Strategy 2: External Value Store

**Concept**: Store u64 index/ID in ACT, store actual values in separate structure

**Applicable to**:
- Arbitrary value types
- Large objects
- Variable-length data
- Complex structures

### Architecture

```
ACT (PathMap → u64 ID)
  path1 → 42
  path2 → 123
  path3 → 42  ← Shared value

Value Store (ID → Value)
  42 → "shared_value"
  123 → ComplexStruct { ... }
```

### Example 2A: Basic Value Store

```rust
use std::collections::HashMap;

struct ValueStore<V> {
    values: HashMap<u64, V>,
    next_id: u64,
}

impl<V: Clone> ValueStore<V> {
    fn new() -> Self {
        ValueStore {
            values: HashMap::new(),
            next_id: 0,
        }
    }

    fn insert(&mut self, value: V) -> u64 {
        let id = self.next_id;
        self.values.insert(id, value);
        self.next_id += 1;
        id
    }

    fn get(&self, id: u64) -> Option<&V> {
        self.values.get(&id)
    }
}

// Usage
let mut value_store = ValueStore::new();
let mut map: PathMap<u64> = PathMap::new();

// Insert values
let id1 = value_store.insert("Hello, world!".to_string());
let id2 = value_store.insert("Goodbye!".to_string());

map.set_val_at(b"greeting", id1);
map.set_val_at(b"farewell", id2);

// Serialize ACT
ArenaCompactTree::dump_from_zipper(
    map.read_zipper(),
    |&v| v,
    "paths.tree"
)?;

// Serialize value store (using serde)
let store_json = serde_json::to_string(&value_store.values)?;
std::fs::write("values.json", store_json)?;

// Load
let act = ArenaCompactTree::open_mmap("paths.tree")?;
let values: HashMap<u64, String> = serde_json::from_str(
    &std::fs::read_to_string("values.json")?
)?;

let id = act.get_val_at(b"greeting").unwrap();
let greeting = values.get(&id).unwrap();
assert_eq!(greeting, "Hello, world!");
```

### Example 2B: Deduplicating Value Store

**Goal**: Reuse IDs for identical values (save space)

```rust
use std::collections::HashMap;
use std::hash::Hash;

struct DeduplicatingStore<V: Clone + Eq + Hash> {
    value_to_id: HashMap<V, u64>,
    id_to_value: HashMap<u64, V>,
    next_id: u64,
}

impl<V: Clone + Eq + Hash> DeduplicatingStore<V> {
    fn new() -> Self {
        DeduplicatingStore {
            value_to_id: HashMap::new(),
            id_to_value: HashMap::new(),
            next_id: 0,
        }
    }

    fn insert(&mut self, value: V) -> u64 {
        // Check if value already exists
        if let Some(&id) = self.value_to_id.get(&value) {
            return id;
        }

        // Allocate new ID
        let id = self.next_id;
        self.value_to_id.insert(value.clone(), id);
        self.id_to_value.insert(id, value);
        self.next_id += 1;
        id
    }

    fn get(&self, id: u64) -> Option<&V> {
        self.id_to_value.get(&id)
    }
}

// Usage
let mut store = DeduplicatingStore::new();

let id1 = store.insert("common_value".to_string());
let id2 = store.insert("common_value".to_string());  // Reuses id1!
let id3 = store.insert("unique_value".to_string());

assert_eq!(id1, id2);  // Same ID for identical values
assert_ne!(id1, id3);

println!("Stored {} unique values", store.id_to_value.len());
// Output: Stored 2 unique values (not 3)
```

### Example 2C: Persistent Value Store

**Goal**: Serialize value store to disk

```rust
use std::fs::File;
use std::io::{BufReader, BufWriter};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct PersistentStore<V> {
    values: HashMap<u64, V>,
    next_id: u64,
}

impl<V: Serialize + for<'de> Deserialize<'de>> PersistentStore<V> {
    fn save(&self, path: &str) -> std::io::Result<()> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, &self.values)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(())
    }

    fn load(path: &str) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let values = bincode::deserialize_from(reader)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let next_id = values.keys().max().map(|&k| k + 1).unwrap_or(0);
        Ok(PersistentStore { values, next_id })
    }
}

// Usage
let mut store = PersistentStore::new();
// ... populate store ...
store.save("values.bin")?;

// Later
let loaded_store = PersistentStore::load("values.bin")?;
```

---

## 4. Strategy 3: Content-Addressed Storage

**Concept**: Use hash of value as ID (enables deduplication + verification)

**Applicable to**:
- Immutable data
- Large objects
- Deduplication-heavy workloads
- Content-verified storage

### Architecture

```
ACT (PathMap → Hash)
  path1 → 0x1234abcd...  ← SHA256 hash
  path2 → 0x9876fedc...
  path3 → 0x1234abcd...  ← Same hash = same value

Value Store (Hash → Value)
  0x1234abcd... → "shared_value"
  0x9876fedc... → ComplexStruct { ... }
```

### Example 3A: Hash-Based Store

```rust
use sha2::{Sha256, Digest};
use std::collections::HashMap;

struct ContentAddressedStore<V> {
    values: HashMap<u64, V>,  // Hash (truncated to u64) → Value
}

impl<V: Clone + serde::Serialize> ContentAddressedStore<V> {
    fn new() -> Self {
        ContentAddressedStore {
            values: HashMap::new(),
        }
    }

    fn hash_value(value: &V) -> u64 {
        // Serialize value
        let bytes = bincode::serialize(value).unwrap();

        // Compute SHA256
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let hash = hasher.finalize();

        // Truncate to u64 (first 8 bytes)
        u64::from_le_bytes(hash[0..8].try_into().unwrap())
    }

    fn insert(&mut self, value: V) -> u64 {
        let hash = Self::hash_value(&value);

        // Store if not already present
        self.values.entry(hash).or_insert(value);

        hash
    }

    fn get(&self, hash: u64) -> Option<&V> {
        self.values.get(&hash)
    }
}

// Usage
let mut store = ContentAddressedStore::new();

let hash1 = store.insert("content1".to_string());
let hash2 = store.insert("content1".to_string());  // Same hash
let hash3 = store.insert("content2".to_string());

assert_eq!(hash1, hash2);  // Identical content → same hash
assert_ne!(hash1, hash3);

println!("Unique values: {}", store.values.len());
// Output: Unique values: 2
```

### Example 3B: Full SHA256 (No Truncation)

**Problem**: Truncating to u64 risks hash collisions

**Solution**: Store full SHA256 separately, use truncated hash as lookup key

```rust
use sha2::{Sha256, Digest};

type FullHash = [u8; 32];

struct FullHashStore<V> {
    values: HashMap<u64, Vec<(FullHash, V)>>,  // Handle collisions
}

impl<V: Clone + serde::Serialize + PartialEq> FullHashStore<V> {
    fn new() -> Self {
        FullHashStore {
            values: HashMap::new(),
        }
    }

    fn hash_value(value: &V) -> (u64, FullHash) {
        let bytes = bincode::serialize(value).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let full_hash: FullHash = hasher.finalize().into();
        let short_hash = u64::from_le_bytes(full_hash[0..8].try_into().unwrap());
        (short_hash, full_hash)
    }

    fn insert(&mut self, value: V) -> u64 {
        let (short_hash, full_hash) = Self::hash_value(&value);

        let entries = self.values.entry(short_hash).or_insert_with(Vec::new);

        // Check if full hash already exists
        for (existing_hash, existing_value) in entries.iter() {
            if existing_hash == &full_hash {
                // Verify value matches (detect hash collision)
                assert_eq!(existing_value, &value, "Hash collision detected!");
                return short_hash;
            }
        }

        // New value
        entries.push((full_hash, value));
        short_hash
    }

    fn get(&self, short_hash: u64, full_hash: &FullHash) -> Option<&V> {
        self.values.get(&short_hash)?.iter()
            .find(|(h, _)| h == full_hash)
            .map(|(_, v)| v)
    }
}
```

### Example 3C: Merkle-Based Deduplication

**Concept**: Combine with PathMap's merkleization for maximum deduplication

```rust
// Step 1: Create PathMap with content-addressed values
let mut store = ContentAddressedStore::new();
let mut map: PathMap<u64> = PathMap::new();

map.set_val_at(b"v1/file1", store.insert("content A".to_string()));
map.set_val_at(b"v1/file2", store.insert("content B".to_string()));
map.set_val_at(b"v2/file1", store.insert("content A".to_string()));  // Same content
map.set_val_at(b"v2/file2", store.insert("content B".to_string()));  // Same content

// Step 2: Merkleize PathMap (deduplicate identical subtrees)
map.merkleize();

// Step 3: Serialize to ACT
ArenaCompactTree::dump_from_zipper(
    map.read_zipper(),
    |&v| v,
    "deduped.tree"
)?;

// Result: Both content-level AND structural deduplication
// - "content A" and "content B" stored once each (content-addressed)
// - Identical v1 and v2 subtrees stored once (merkleization)
```

---

## 5. MeTTa Term Encoding

**Challenge**: Encode MeTTa terms (atoms, expressions, etc.) as u64

### Strategy: External Term Store

```rust
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MeTTaTerm {
    Atom(String),
    Expression(Vec<MeTTaTerm>),
    Variable(String),
}

struct TermStore {
    terms: HashMap<u64, MeTTaTerm>,
    term_to_id: HashMap<MeTTaTerm, u64>,
    next_id: u64,
}

impl TermStore {
    fn new() -> Self {
        TermStore {
            terms: HashMap::new(),
            term_to_id: HashMap::new(),
            next_id: 0,
        }
    }

    fn intern(&mut self, term: MeTTaTerm) -> u64 {
        if let Some(&id) = self.term_to_id.get(&term) {
            return id;
        }

        let id = self.next_id;
        self.terms.insert(id, term.clone());
        self.term_to_id.insert(term, id);
        self.next_id += 1;
        id
    }

    fn get(&self, id: u64) -> Option<&MeTTaTerm> {
        self.terms.get(&id)
    }
}

// Usage
let mut store = TermStore::new();
let mut kb: PathMap<u64> = PathMap::new();

// Create terms
let atom_x = MeTTaTerm::Atom("x".to_string());
let atom_y = MeTTaTerm::Atom("y".to_string());
let expr = MeTTaTerm::Expression(vec![
    MeTTaTerm::Atom("add".to_string()),
    atom_x.clone(),
    atom_y.clone(),
]);

// Intern and store
let expr_id = store.intern(expr);
kb.set_val_at(b"terms/expr1", expr_id);

// Shared atoms (deduplication)
let expr2 = MeTTaTerm::Expression(vec![
    MeTTaTerm::Atom("mul".to_string()),
    atom_x.clone(),  // Reuses same atom
    atom_y.clone(),
]);
let expr2_id = store.intern(expr2);
kb.set_val_at(b"terms/expr2", expr2_id);

println!("Unique terms: {}", store.terms.len());
// Output: Unique terms: 5 (add, mul, x, y, expr1, expr2)
// Atoms "x" and "y" are shared between expressions

// Serialize
ArenaCompactTree::dump_from_zipper(
    kb.read_zipper(),
    |&v| v,
    "kb.tree"
)?;

// Save term store
let store_json = serde_json::to_string(&store.terms)?;
std::fs::write("terms.json", store_json)?;
```

---

## 6. Performance Considerations

### Direct Encoding

**Pros**:
- Zero overhead (value stored directly)
- Fastest queries (no indirection)
- Smallest file size

**Cons**:
- Limited to 8 bytes
- No deduplication for complex types

**Use when**: Values fit in u64 and performance is critical

### External Value Store

**Pros**:
- Supports arbitrary types
- Deduplication possible
- Flexible (can change value format independently)

**Cons**:
- Extra indirection (ID → value lookup)
- Two files to manage (ACT + value store)
- Larger total storage (IDs + values)

**Use when**: Values are complex or variable-length

### Content-Addressed Storage

**Pros**:
- Automatic deduplication
- Content verification (hash check)
- Immutability enforced

**Cons**:
- Hash computation overhead
- Risk of hash collisions (mitigate with full hashes)
- More complex implementation

**Use when**: Deduplication is critical and data is immutable

---

## 7. Hybrid Approaches

### Approach 1: Small Inline, Large External

**Strategy**: Store small values directly, large values externally

```rust
enum HybridValue {
    Inline(u64),      // Encoded directly (e.g., small ints, enums)
    External(u64),    // ID into value store
}

impl HybridValue {
    fn encode(&self) -> u64 {
        match self {
            HybridValue::Inline(value) => {
                // High bit = 0 for inline
                *value & 0x7FFFFFFFFFFFFFFF
            }
            HybridValue::External(id) => {
                // High bit = 1 for external
                *id | 0x8000000000000000
            }
        }
    }

    fn decode(encoded: u64, store: &ValueStore<String>) -> Result<Value, Error> {
        if encoded & 0x8000000000000000 == 0 {
            // Inline value
            Ok(Value::Integer(encoded as i64))
        } else {
            // External value
            let id = encoded & 0x7FFFFFFFFFFFFFFF;
            store.get(id)
                .map(|s| Value::String(s.clone()))
                .ok_or(Error::NotFound)
        }
    }
}
```

### Approach 2: Tiered Storage

**Strategy**: Multiple value stores by type

```rust
struct TieredStore {
    strings: HashMap<u64, String>,
    structs: HashMap<u64, ComplexStruct>,
    blobs: HashMap<u64, Vec<u8>>,
}

impl TieredStore {
    fn encode_string(&mut self, s: String) -> u64 {
        let id = hash(&s);
        let tagged_id = (id << 8) | 0x01;  // Type tag: 0x01 = string
        self.strings.insert(id, s);
        tagged_id
    }

    fn encode_struct(&mut self, s: ComplexStruct) -> u64 {
        let id = hash(&s);
        let tagged_id = (id << 8) | 0x02;  // Type tag: 0x02 = struct
        self.structs.insert(id, s);
        tagged_id
    }

    fn decode(&self, tagged_id: u64) -> Result<Value, Error> {
        let type_tag = tagged_id & 0xFF;
        let id = tagged_id >> 8;

        match type_tag {
            0x01 => self.strings.get(&id).map(|s| Value::String(s.clone())),
            0x02 => self.structs.get(&id).map(|s| Value::Struct(s.clone())),
            _ => None,
        }.ok_or(Error::InvalidType)
    }
}
```

---

## 8. Integration Examples

### Example: Complete MeTTaTron Knowledge Base

```rust
use pathmap::PathMap;
use pathmap::arena_compact::ArenaCompactTree;
use std::collections::HashMap;

// Step 1: Define structures
#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
struct KBEntry {
    term: String,
    confidence: f64,
    metadata: HashMap<String, String>,
}

struct KnowledgeBase {
    paths: PathMap<u64>,
    values: DeduplicatingStore<KBEntry>,
}

impl KnowledgeBase {
    fn new() -> Self {
        KnowledgeBase {
            paths: PathMap::new(),
            values: DeduplicatingStore::new(),
        }
    }

    fn insert(&mut self, path: &[u8], entry: KBEntry) {
        let id = self.values.insert(entry);
        self.paths.set_val_at(path, id);
    }

    fn get(&self, path: &[u8]) -> Option<&KBEntry> {
        let id = self.paths.get_val_at(path)?;
        self.values.get(*id)
    }

    fn save(&self, base_path: &str) -> std::io::Result<()> {
        // Save ACT
        let act_path = format!("{}.tree", base_path);
        ArenaCompactTree::dump_from_zipper(
            self.paths.read_zipper(),
            |&v| v,
            &act_path
        )?;

        // Save value store
        let store_path = format!("{}.values", base_path);
        let json = serde_json::to_string(&self.values.id_to_value)?;
        std::fs::write(store_path, json)?;

        Ok(())
    }

    fn load(base_path: &str) -> std::io::Result<Self> {
        // Load ACT (instant via mmap)
        let act_path = format!("{}.tree", base_path);
        let act = ArenaCompactTree::open_mmap(&act_path)?;

        // Load value store
        let store_path = format!("{}.values", base_path);
        let json = std::fs::read_to_string(store_path)?;
        let id_to_value: HashMap<u64, KBEntry> = serde_json::from_str(&json)?;

        // Reconstruct paths from ACT
        let mut paths = PathMap::new();
        for (path, id) in act.iter() {
            paths.set_val_at(path, id);
        }

        Ok(KnowledgeBase {
            paths,
            values: DeduplicatingStore {
                id_to_value,
                value_to_id: id_to_value.iter()
                    .map(|(&id, v)| (v.clone(), id))
                    .collect(),
                next_id: id_to_value.keys().max().map(|&k| k + 1).unwrap_or(0),
            },
        })
    }
}

// Usage
fn main() -> std::io::Result<()> {
    let mut kb = KnowledgeBase::new();

    kb.insert(b"facts/math/addition", KBEntry {
        term: "(+ 2 2 4)".to_string(),
        confidence: 1.0,
        metadata: [("source".to_string(), "axiom".to_string())]
            .iter().cloned().collect(),
    });

    kb.insert(b"facts/logic/modus_ponens", KBEntry {
        term: "(→ P Q) ∧ P ⊢ Q".to_string(),
        confidence: 1.0,
        metadata: [("source".to_string(), "rule".to_string())]
            .iter().cloned().collect(),
    });

    // Save
    kb.save("knowledge_base")?;

    // Load (instant for ACT, fast for value store)
    let loaded_kb = KnowledgeBase::load("knowledge_base")?;

    // Query
    let entry = loaded_kb.get(b"facts/math/addition").unwrap();
    println!("Term: {}", entry.term);
    println!("Confidence: {}", entry.confidence);

    Ok(())
}
```

---

## 9. Recommendations

### For Small Values (≤ 8 bytes)

**Use**: Direct encoding (Strategy 1)
**Examples**: Integers, floats, enums, flags, packed structs
**Benefits**: Zero overhead, maximum performance

### For Complex Immutable Values

**Use**: Content-addressed storage (Strategy 3)
**Examples**: Large structs, immutable documents, snapshots
**Benefits**: Automatic deduplication, content verification

### For Complex Mutable Values

**Use**: External value store (Strategy 2)
**Examples**: Mutable objects, variable-length strings, nested structures
**Benefits**: Flexibility, arbitrary types

### For MeTTa Knowledge Bases

**Use**: Hybrid approach
- **Direct encoding**: For node types, flags, small metadata
- **External store**: For term ASTs, complex expressions
- **Content-addressed**: For immutable compiled terms
**Benefits**: Balance performance and flexibility

---

## 10. Trade-off Summary

| Strategy | File Size | Query Speed | Flexibility | Complexity |
|----------|-----------|-------------|-------------|------------|
| **Direct** | Smallest | Fastest | Low | Low |
| **External** | Medium | Medium | High | Medium |
| **Content-addressed** | Small* | Medium | Medium | High |

*Assuming deduplication is effective

---

## References

### Source Code
- **ACT format**: `src/arena_compact.rs`
- **PathMap core**: `src/trie_map.rs`

### Related Documentation
- [ACT Format](03_act_format.md) - ACT structure details
- [Overview](01_overview.md) - Format comparison
- [MeTTaTron Integration](07_mettaton_integration.md) - Integration patterns

### External Resources
- **bincode**: https://docs.rs/bincode/ (efficient binary serialization)
- **serde**: https://docs.rs/serde/ (serialization framework)
- **sha2**: https://docs.rs/sha2/ (SHA256 hashing)
