# MeTTaTron Integration Guide

**Purpose**: Complete integration guide for using PathMap persistence in MeTTaTron compiler and knowledge base.

**Target audience**: MeTTaTron developers integrating PathMap serialization

---

## 1. Integration Overview

### MeTTaTron Use Cases

1. **Compilation artifacts**: Save compiled MeTTa ASTs for fast loading
2. **Knowledge bases**: Persistent storage of facts, rules, and axioms
3. **Incremental compilation**: Track changes between compilation runs
4. **Query optimization**: Pre-compute and cache query results
5. **Distributed systems**: Share knowledge bases across processes/machines

### Recommended Architecture

```
MeTTaTron
├── In-memory PathMap (working set)
│   └── Active compilation/reasoning
├── ACT snapshot (persistent)
│   └── Compiled knowledge base (read-only)
├── Value store (persistent)
│   └── Term ASTs, metadata
└── Delta files (paths format)
    └── Incremental changes
```

---

## 2. Basic Integration

### Step 1: Add Dependencies

```toml
[dependencies]
pathmap = { version = "0.2", features = ["arena_compact", "serialization"] }
serde = { version = "1.0", features = ["derive"] }
bincode = "1.3"
```

### Step 2: Define Value Types

```rust
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MeTTaTerm {
    Atom(String),
    Variable(String),
    Expression(Vec<MeTTaTerm>),
    Number(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KBEntry {
    term: MeTTaTerm,
    confidence: f64,
    source: String,
    metadata: HashMap<String, String>,
}
```

### Step 3: Create Knowledge Base Wrapper

```rust
use pathmap::PathMap;
use pathmap::arena_compact::ArenaCompactTree;
use std::collections::HashMap;

pub struct KnowledgeBase {
    // In-memory working set
    paths: PathMap<u64>,

    // Value store (term ID → term)
    terms: HashMap<u64, KBEntry>,

    // Snapshot (optional, for fast loading)
    snapshot: Option<ArenaCompactTree<memmap2::Mmap>>,
}

impl KnowledgeBase {
    pub fn new() -> Self {
        KnowledgeBase {
            paths: PathMap::new(),
            terms: HashMap::new(),
            snapshot: None,
        }
    }

    pub fn insert(&mut self, path: &[u8], entry: KBEntry) -> u64 {
        // Hash entry to get ID
        let id = hash_entry(&entry);

        // Store in value store
        self.terms.insert(id, entry);

        // Store ID in PathMap
        self.paths.set_val_at(path, id);

        id
    }

    pub fn get(&self, path: &[u8]) -> Option<&KBEntry> {
        // Try in-memory first
        if let Some(&id) = self.paths.get_val_at(path) {
            return self.terms.get(&id);
        }

        // Try snapshot if available
        if let Some(ref snapshot) = self.snapshot {
            let id = snapshot.get_val_at(path)?;
            return self.terms.get(&id);
        }

        None
    }
}

fn hash_entry(entry: &KBEntry) -> u64 {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    entry.hash(&mut hasher);
    hasher.finish()
}
```

---

## 3. Pattern 1: Compilation Artifacts

**Use case**: Save compiled MeTTa programs for instant loading

### Implementation

```rust
use pathmap::PathMap;
use pathmap::arena_compact::ArenaCompactTree;
use std::path::Path;

pub struct MeTTaCompiler {
    // Compilation results
    compiled: PathMap<u64>,
    terms: HashMap<u64, CompiledTerm>,
}

#[derive(Serialize, Deserialize)]
pub struct CompiledTerm {
    ast: MeTTaTerm,
    bytecode: Vec<u8>,
    dependencies: Vec<String>,
}

impl MeTTaCompiler {
    pub fn compile_to_disk<P: AsRef<Path>>(
        &self,
        base_path: P
    ) -> std::io::Result<()> {
        let base = base_path.as_ref();

        // 1. Serialize PathMap to ACT
        let act_path = base.with_extension("tree");
        ArenaCompactTree::dump_from_zipper(
            self.compiled.read_zipper(),
            |&id| id,
            &act_path
        )?;

        // 2. Serialize term store
        let terms_path = base.with_extension("terms");
        let terms_file = std::fs::File::create(terms_path)?;
        bincode::serialize_into(terms_file, &self.terms)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        println!("Compiled {} terms to {:?}", self.terms.len(), base);
        Ok(())
    }

    pub fn load_from_disk<P: AsRef<Path>>(
        base_path: P
    ) -> std::io::Result<Self> {
        let base = base_path.as_ref();

        // 1. Load ACT (instant via mmap)
        let act_path = base.with_extension("tree");
        let act = ArenaCompactTree::open_mmap(&act_path)?;

        // 2. Load term store
        let terms_path = base.with_extension("terms");
        let terms_file = std::fs::File::open(terms_path)?;
        let terms: HashMap<u64, CompiledTerm> = bincode::deserialize_from(terms_file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        // 3. Reconstruct in-memory PathMap from ACT
        let mut compiled = PathMap::new();
        for (path, id) in act.iter() {
            compiled.set_val_at(path, id);
        }

        println!("Loaded {} compiled terms from {:?}", terms.len(), base);

        Ok(MeTTaCompiler { compiled, terms })
    }
}

// Usage
fn main() -> std::io::Result<()> {
    // Compile once
    let mut compiler = MeTTaCompiler::new();
    compiler.compile_file("program.metta")?;
    compiler.compile_to_disk("program.compiled")?;

    // Load many times (instant)
    let loaded = MeTTaCompiler::load_from_disk("program.compiled")?;
    loaded.execute()?;

    Ok(())
}
```

### Performance

| Operation | Time | Notes |
|-----------|------|-------|
| **Compile + save** | ~1-2 s | One-time cost |
| **Load** | ~10-20 ms | ACT mmap + deserialize terms |
| **Execute** | ~instant | Terms already compiled |

**Savings**: ~100× faster startup vs recompiling every time

---

## 4. Pattern 2: Incremental Knowledge Base

**Use case**: Track changes to knowledge base, periodic snapshots

### Implementation

```rust
use pathmap::paths_serialization::{serialize_paths, deserialize_paths};
use std::path::PathBuf;

pub struct IncrementalKB {
    kb: KnowledgeBase,
    last_snapshot: PathBuf,
    delta_counter: usize,
}

impl IncrementalKB {
    pub fn new() -> Self {
        IncrementalKB {
            kb: KnowledgeBase::new(),
            last_snapshot: PathBuf::new(),
            delta_counter: 0,
        }
    }

    pub fn insert(&mut self, path: &[u8], entry: KBEntry) {
        self.kb.insert(path, entry);
        self.delta_counter += 1;

        // Auto-snapshot every 10K changes
        if self.delta_counter >= 10_000 {
            self.create_snapshot().expect("Failed to snapshot");
        }
    }

    pub fn create_snapshot(&mut self) -> std::io::Result<()> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let snapshot_path = format!("kb_snapshot_{}.tree", timestamp);
        let terms_path = format!("kb_snapshot_{}.terms", timestamp);

        // Serialize ACT
        ArenaCompactTree::dump_from_zipper(
            self.kb.paths.read_zipper(),
            |&id| id,
            &snapshot_path
        )?;

        // Serialize terms
        let terms_file = std::fs::File::create(&terms_path)?;
        bincode::serialize_into(terms_file, &self.kb.terms)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        self.last_snapshot = PathBuf::from(snapshot_path);
        self.delta_counter = 0;

        println!("Created snapshot: {:?}", self.last_snapshot);
        Ok(())
    }

    pub fn save_delta(&self, delta_path: &str) -> std::io::Result<()> {
        // Extract changes since last snapshot
        // (For simplicity, saving entire current state as delta)
        let delta_file = std::fs::File::create(delta_path)?;
        serialize_paths(
            self.kb.paths.read_zipper(),
            &mut delta_file
        )?;

        println!("Saved {} deltas", self.delta_counter);
        Ok(())
    }

    pub fn load_with_deltas(
        snapshot_base: &str,
        delta_files: &[&str]
    ) -> std::io::Result<Self> {
        // Load snapshot
        let snapshot_path = format!("{}.tree", snapshot_base);
        let terms_path = format!("{}.terms", snapshot_base);

        let act = ArenaCompactTree::open_mmap(&snapshot_path)?;
        let terms_file = std::fs::File::open(&terms_path)?;
        let terms: HashMap<u64, KBEntry> = bincode::deserialize_from(terms_file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut paths = PathMap::new();
        for (path, id) in act.iter() {
            paths.set_val_at(path, id);
        }

        let mut kb = KnowledgeBase { paths, terms, snapshot: Some(act) };

        // Apply deltas
        for delta_file in delta_files {
            let file = std::fs::File::open(delta_file)?;
            deserialize_paths(kb.paths.write_zipper(), file, 0u64)?;
        }

        Ok(IncrementalKB {
            kb,
            last_snapshot: PathBuf::from(snapshot_base),
            delta_counter: 0,
        })
    }
}

// Usage
fn main() -> std::io::Result<()> {
    let mut kb = IncrementalKB::new();

    // Insert many entries
    for i in 0..50_000 {
        let path = format!("fact/{}", i);
        let entry = KBEntry {
            term: MeTTaTerm::Number(i as i64),
            confidence: 1.0,
            source: "generated".to_string(),
            metadata: HashMap::new(),
        };
        kb.insert(path.as_bytes(), entry);
    }
    // Auto-snapshots created at 10K, 20K, 30K, 40K, 50K

    // Save remaining delta
    kb.save_delta("delta_final.paths")?;

    // Later: Load snapshot + deltas
    let loaded = IncrementalKB::load_with_deltas(
        "kb_snapshot_1234567890",
        &["delta_final.paths"]
    )?;

    Ok(())
}
```

### Benefits

- **Incremental saves**: Only delta files written frequently
- **Fast recovery**: Load latest snapshot + apply deltas
- **Space-efficient**: Delta files compressed (paths format)

---

## 5. Pattern 3: Hybrid In-Memory + Persistent

**Use case**: Fast in-memory reasoning with persistent backing

### Implementation

```rust
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

pub struct HybridKB {
    // In-memory working set (hot data)
    working: Arc<RwLock<PathMap<u64>>>,

    // Persistent snapshot (cold data)
    snapshot: Arc<ArenaCompactTree<memmap2::Mmap>>,

    // Value store
    terms: Arc<RwLock<HashMap<u64, KBEntry>>>,
}

impl HybridKB {
    pub fn new(snapshot_path: &str) -> std::io::Result<Self> {
        let snapshot = Arc::new(ArenaCompactTree::open_mmap(snapshot_path)?);

        // Load term store
        let terms_path = format!("{}.terms", snapshot_path.trim_end_matches(".tree"));
        let terms_file = std::fs::File::open(terms_path)?;
        let terms_map: HashMap<u64, KBEntry> = bincode::deserialize_from(terms_file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(HybridKB {
            working: Arc::new(RwLock::new(PathMap::new())),
            snapshot,
            terms: Arc::new(RwLock::new(terms_map)),
        })
    }

    pub fn query(&self, path: &[u8]) -> Option<KBEntry> {
        // Try working set first (recent updates)
        {
            let working = self.working.read().unwrap();
            if let Some(&id) = working.get_val_at(path) {
                let terms = self.terms.read().unwrap();
                return terms.get(&id).cloned();
            }
        }

        // Fall back to snapshot (persistent)
        if let Some(id) = self.snapshot.get_val_at(path) {
            let terms = self.terms.read().unwrap();
            return terms.get(&id).cloned();
        }

        None
    }

    pub fn insert(&self, path: &[u8], entry: KBEntry) {
        let id = hash_entry(&entry);

        // Update working set
        {
            let mut working = self.working.write().unwrap();
            working.set_val_at(path, id);
        }

        // Update term store
        {
            let mut terms = self.terms.write().unwrap();
            terms.insert(id, entry);
        }
    }

    pub fn start_background_snapshot(&self, interval: Duration) {
        let working = Arc::clone(&self.working);
        let terms = Arc::clone(&self.terms);

        thread::spawn(move || {
            loop {
                thread::sleep(interval);

                // Create snapshot
                let snapshot_time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                let snapshot_path = format!("kb_snapshot_{}.tree", snapshot_time);
                let terms_path = format!("kb_snapshot_{}.terms", snapshot_time);

                // Serialize working set
                let working_locked = working.read().unwrap();
                if let Err(e) = ArenaCompactTree::dump_from_zipper(
                    working_locked.read_zipper(),
                    |&id| id,
                    &snapshot_path
                ) {
                    eprintln!("Snapshot failed: {}", e);
                    continue;
                }

                // Serialize terms
                let terms_locked = terms.read().unwrap();
                let terms_file = match std::fs::File::create(&terms_path) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("Failed to create terms file: {}", e);
                        continue;
                    }
                };

                if let Err(e) = bincode::serialize_into(terms_file, &*terms_locked) {
                    eprintln!("Term serialization failed: {}", e);
                    continue;
                }

                println!("Background snapshot created: {}", snapshot_path);
            }
        });
    }
}

// Usage
fn main() -> std::io::Result<()> {
    // Load existing snapshot
    let kb = HybridKB::new("initial_snapshot.tree")?;

    // Start background snapshots every 5 minutes
    kb.start_background_snapshot(Duration::from_secs(300));

    // Query (fast - checks working set, then snapshot)
    if let Some(entry) = kb.query(b"fact/important") {
        println!("Found: {:?}", entry);
    }

    // Insert (fast - in-memory)
    kb.insert(b"fact/new", KBEntry {
        term: MeTTaTerm::Atom("new_fact".to_string()),
        confidence: 0.9,
        source: "reasoning".to_string(),
        metadata: HashMap::new(),
    });

    // Periodic snapshots created in background

    Ok(())
}
```

### Benefits

- **Fast queries**: In-memory working set for hot data
- **Fast inserts**: No immediate disk I/O
- **Persistent**: Background snapshots for durability
- **Scalable**: Snapshot handles cold data (can exceed RAM)

---

## 6. Pattern 4: Distributed Knowledge Sharing

**Use case**: Multiple processes share read-only knowledge base

### Implementation

```rust
use std::process;

pub fn spawn_workers(kb_path: &str, num_workers: usize) -> std::io::Result<()> {
    let kb = Arc::new(ArenaCompactTree::open_mmap(kb_path)?);

    // Spawn worker threads
    let handles: Vec<_> = (0..num_workers).map(|worker_id| {
        let kb_clone = Arc::clone(&kb);

        thread::spawn(move || {
            println!("Worker {} started", worker_id);

            // Each worker queries KB independently
            for i in 0..1000 {
                let path = format!("query/{}/{}", worker_id, i);
                if let Some(value) = kb_clone.get_val_at(path.as_bytes()) {
                    // Process value
                    process_result(value);
                }
            }

            println!("Worker {} finished", worker_id);
        })
    }).collect();

    // Wait for workers
    for handle in handles {
        handle.join().unwrap();
    }

    Ok(())
}

fn process_result(value: u64) {
    // Decode value, perform reasoning, etc.
}

// Usage
fn main() -> std::io::Result<()> {
    // Load KB once
    let kb_path = "shared_kb.tree";

    // Spawn 16 worker threads sharing same mmap
    // OS shares physical pages between threads
    spawn_workers(kb_path, 16)?;

    Ok(())
}
```

### Multi-Process Sharing

```rust
// Process 1: Load KB
let kb1 = ArenaCompactTree::open_mmap("shared_kb.tree")?;

// Process 2: Load same KB
let kb2 = ArenaCompactTree::open_mmap("shared_kb.tree")?;

// OS shares physical pages between processes!
// Total RAM usage ≈ 1× KB size (not 2×)
```

### Benefits

- **Shared memory**: OS shares pages between processes
- **No IPC overhead**: Direct memory access
- **Scalability**: 100s of processes can share same KB
- **Read-only**: Safe concurrent access (no coordination needed)

---

## 7. Value Encoding Strategies

### Strategy 1: Term Interning

```rust
use std::collections::HashMap;

pub struct TermStore {
    term_to_id: HashMap<MeTTaTerm, u64>,
    id_to_term: HashMap<u64, MeTTaTerm>,
    next_id: u64,
}

impl TermStore {
    pub fn intern(&mut self, term: MeTTaTerm) -> u64 {
        if let Some(&id) = self.term_to_id.get(&term) {
            return id;  // Reuse existing ID
        }

        let id = self.next_id;
        self.term_to_id.insert(term.clone(), id);
        self.id_to_term.insert(id, term);
        self.next_id += 1;
        id
    }

    pub fn get(&self, id: u64) -> Option<&MeTTaTerm> {
        self.id_to_term.get(&id)
    }
}

// Usage
let mut store = TermStore::new();

let term1 = MeTTaTerm::Atom("x".to_string());
let id1 = store.intern(term1.clone());
let id2 = store.intern(term1.clone());  // Same ID!

assert_eq!(id1, id2);  // Deduplication
```

### Strategy 2: Content-Addressed Terms

```rust
use sha2::{Sha256, Digest};

pub fn hash_term(term: &MeTTaTerm) -> u64 {
    let bytes = bincode::serialize(term).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = hasher.finalize();
    u64::from_le_bytes(hash[0..8].try_into().unwrap())
}

// Usage
let term = MeTTaTerm::Expression(vec![
    MeTTaTerm::Atom("add".to_string()),
    MeTTaTerm::Number(1),
    MeTTaTerm::Number(2),
]);

let id = hash_term(&term);
kb.insert(b"expr/1", id);

// Same term → same hash → automatic deduplication
```

### Strategy 3: Hybrid (Small Inline, Large External)

```rust
pub enum EncodedValue {
    Inline(u64),      // Small values (numbers, enum tags)
    External(u64),    // Large values (complex terms)
}

impl EncodedValue {
    pub fn encode(term: &MeTTaTerm) -> u64 {
        match term {
            MeTTaTerm::Number(n) if *n >= 0 && *n < i64::MAX => {
                // Inline: pack number directly
                (*n as u64) & 0x7FFFFFFFFFFFFFFF  // Clear high bit
            }
            _ => {
                // External: hash term
                let hash = hash_term(term);
                hash | 0x8000000000000000  // Set high bit
            }
        }
    }

    pub fn decode(encoded: u64, store: &TermStore) -> Option<MeTTaTerm> {
        if encoded & 0x8000000000000000 == 0 {
            // Inline number
            Some(MeTTaTerm::Number(encoded as i64))
        } else {
            // External term
            let id = encoded & 0x7FFFFFFFFFFFFFFF;
            store.get(id).cloned()
        }
    }
}
```

---

## 8. Production Checklist

### Before Deployment

- [ ] **Choose format**: ACT for large KBs (> 100 MB), Paths for small
- [ ] **Design value encoding**: Direct, external store, or content-addressed
- [ ] **Implement term interning**: Avoid duplicate term storage
- [ ] **Add compression**: Use paths format for delta files
- [ ] **Plan snapshots**: Periodic ACT snapshots + incremental deltas
- [ ] **Test recovery**: Ensure snapshots + deltas can reconstruct state
- [ ] **Benchmark**: Verify load time, query time meet requirements
- [ ] **Profile memory**: Check working set size vs total KB size
- [ ] **Handle errors**: Corrupted files, missing dependencies, etc.
- [ ] **Version files**: Include format version for future compatibility

### Performance Tuning

- [ ] **Merkleize before serialization**: Deduplicate identical subtrees
- [ ] **Pre-warm cache**: Background thread traverses ACT at startup
- [ ] **Sort queries**: Group by prefix for better locality
- [ ] **Batch operations**: Amortize page fault cost
- [ ] **Use jemalloc**: Reduce allocator overhead for in-memory PathMap
- [ ] **Monitor page faults**: `perf stat -e page-faults ./program`

---

## 9. Example: Complete MeTTaTron KB

```rust
use pathmap::PathMap;
use pathmap::arena_compact::ArenaCompactTree;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MeTTaTerm {
    Atom(String),
    Variable(String),
    Expression(Vec<MeTTaTerm>),
    Number(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KBEntry {
    term: MeTTaTerm,
    confidence: f64,
    source: String,
    metadata: HashMap<String, String>,
}

pub struct MeTTaTronKB {
    working: Arc<RwLock<PathMap<u64>>>,
    snapshot: Option<Arc<ArenaCompactTree<memmap2::Mmap>>>,
    terms: Arc<RwLock<HashMap<u64, KBEntry>>>,
}

impl MeTTaTronKB {
    pub fn new() -> Self {
        MeTTaTronKB {
            working: Arc::new(RwLock::new(PathMap::new())),
            snapshot: None,
            terms: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn load_snapshot(path: &str) -> std::io::Result<Self> {
        let snapshot = Arc::new(ArenaCompactTree::open_mmap(path)?);

        let terms_path = format!("{}.terms", path.trim_end_matches(".tree"));
        let terms_file = std::fs::File::open(terms_path)?;
        let terms_map = bincode::deserialize_from(terms_file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(MeTTaTronKB {
            working: Arc::new(RwLock::new(PathMap::new())),
            snapshot: Some(snapshot),
            terms: Arc::new(RwLock::new(terms_map)),
        })
    }

    pub fn insert(&self, path: &[u8], entry: KBEntry) {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let mut hasher = DefaultHasher::new();
        entry.hash(&mut hasher);
        let id = hasher.finish();

        {
            let mut working = self.working.write().unwrap();
            working.set_val_at(path, id);
        }

        {
            let mut terms = self.terms.write().unwrap();
            terms.entry(id).or_insert(entry);
        }
    }

    pub fn query(&self, path: &[u8]) -> Option<KBEntry> {
        // Try working set
        {
            let working = self.working.read().unwrap();
            if let Some(&id) = working.get_val_at(path) {
                let terms = self.terms.read().unwrap();
                return terms.get(&id).cloned();
            }
        }

        // Try snapshot
        if let Some(ref snapshot) = self.snapshot {
            if let Some(id) = snapshot.get_val_at(path) {
                let terms = self.terms.read().unwrap();
                return terms.get(&id).cloned();
            }
        }

        None
    }

    pub fn save(&self, base_path: &str) -> std::io::Result<()> {
        let working = self.working.read().unwrap();
        let terms = self.terms.read().unwrap();

        // Merge working set with snapshot
        let mut merged = PathMap::new();

        // Add snapshot entries
        if let Some(ref snapshot) = self.snapshot {
            for (path, id) in snapshot.iter() {
                merged.set_val_at(path, id);
            }
        }

        // Overlay working set
        for (path, &id) in working.iter() {
            merged.set_val_at(path, id);
        }

        // Merkleize for deduplication
        merged.merkleize();

        // Save ACT
        let tree_path = format!("{}.tree", base_path);
        ArenaCompactTree::dump_from_zipper(
            merged.read_zipper(),
            |&id| id,
            &tree_path
        )?;

        // Save terms
        let terms_path = format!("{}.terms", base_path);
        let terms_file = std::fs::File::create(terms_path)?;
        bincode::serialize_into(terms_file, &*terms)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        println!("Saved KB to {}", base_path);
        Ok(())
    }
}

// Usage
fn main() -> std::io::Result<()> {
    // Create KB
    let kb = MeTTaTronKB::new();

    // Insert facts
    kb.insert(b"facts/math/addition", KBEntry {
        term: MeTTaTerm::Expression(vec![
            MeTTaTerm::Atom("=".to_string()),
            MeTTaTerm::Expression(vec![
                MeTTaTerm::Atom("+".to_string()),
                MeTTaTerm::Number(2),
                MeTTaTerm::Number(2),
            ]),
            MeTTaTerm::Number(4),
        ]),
        confidence: 1.0,
        source: "axiom".to_string(),
        metadata: HashMap::new(),
    });

    // Save
    kb.save("mettaton_kb")?;

    // Load (instant)
    let loaded_kb = MeTTaTronKB::load_snapshot("mettaton_kb.tree")?;

    // Query
    if let Some(entry) = loaded_kb.query(b"facts/math/addition") {
        println!("Found: {:?}", entry);
    }

    Ok(())
}
```

---

## 10. Troubleshooting

### Issue: Slow Queries After Load

**Cause**: Cold page cache

**Solution**: Pre-warm cache
```rust
// Background thread to warm cache
let kb_clone = Arc::clone(&kb);
thread::spawn(move || {
    for (_, _) in kb_clone.snapshot.as_ref().unwrap().iter() {
        // Touch all pages
    }
});
```

### Issue: Large Term Store File

**Cause**: Many duplicate terms not deduplicated

**Solution**: Use term interning or content-addressed storage
```rust
let mut store = TermStore::new();
let id = store.intern(term);  // Reuses IDs for identical terms
```

### Issue: Out of Memory on Large KB

**Cause**: Loading entire KB into RAM

**Solution**: Use ACT format with mmap (working set only)
```rust
// ❌ Loads all into RAM
let mut kb = PathMap::new();
deserialize_paths(kb.write_zipper(), file, default)?;

// ✅ Loads only working set
let kb = ArenaCompactTree::open_mmap("kb.tree")?;
```

---

## References

### Source Code
- **PathMap**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/trie_map.rs`
- **ACT**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/arena_compact.rs`
- **Paths**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/paths_serialization.rs`

### Related Documentation
- [Paths Format](02_paths_format.md)
- [ACT Format](03_act_format.md)
- [Value Encoding](05_value_encoding.md)
- [Performance Analysis](06_performance_analysis.md)

### MeTTaTron Resources
- **MeTTa Compiler**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/`
- **Hyperon (official MeTTa)**: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/`
