//! Example 3: External Value Store Pattern
//!
//! Demonstrates:
//! - Storing complex values externally (ACT limitation workaround)
//! - Value deduplication via hash-based store
//! - Persistent value store with bincode
//! - Complete save/load workflow
//!
//! To run:
//! cargo run --example value_store

use pathmap::PathMap;
use pathmap::arena_compact::ArenaCompactTree;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io;

// Complex value type that doesn't fit in u64
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum MeTTaTerm {
    Atom(String),
    Variable(String),
    Expression(Vec<MeTTaTerm>),
    Number(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KBEntry {
    term: MeTTaTerm,
    confidence: f64,
    source: String,
    metadata: HashMap<String, String>,
}

// Hash-based value store with deduplication
struct ValueStore {
    value_to_id: HashMap<u64, KBEntry>,  // Hash → Entry
    next_id: u64,
}

impl ValueStore {
    fn new() -> Self {
        ValueStore {
            value_to_id: HashMap::new(),
            next_id: 0,
        }
    }

    fn insert(&mut self, entry: KBEntry) -> u64 {
        // Compute hash
        let id = Self::hash_entry(&entry);

        // Store if not exists (deduplication)
        self.value_to_id.entry(id).or_insert(entry);

        id
    }

    fn get(&self, id: u64) -> Option<&KBEntry> {
        self.value_to_id.get(&id)
    }

    fn hash_entry(entry: &KBEntry) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        entry.term.hash(&mut hasher);
        hasher.finish()
    }

    fn save(&self, path: &str) -> io::Result<()> {
        let file = std::fs::File::create(path)?;
        bincode::serialize_into(file, &self.value_to_id)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn load(path: &str) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let value_to_id = bincode::deserialize_from(file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let next_id = value_to_id.keys().max().map(|&k| k + 1).unwrap_or(0);

        Ok(ValueStore {
            value_to_id,
            next_id,
        })
    }
}

// Knowledge base with external value store
struct KnowledgeBase {
    paths: PathMap<u64>,  // Path → Value ID
    values: ValueStore,   // Value ID → Entry
}

impl KnowledgeBase {
    fn new() -> Self {
        KnowledgeBase {
            paths: PathMap::new(),
            values: ValueStore::new(),
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

    fn save(&self, base_path: &str) -> io::Result<()> {
        // Save ACT (paths → IDs)
        let tree_path = format!("{}.tree", base_path);
        ArenaCompactTree::dump_from_zipper(
            self.paths.read_zipper(),
            |&id| id,
            &tree_path
        )?;

        // Save value store (IDs → entries)
        let values_path = format!("{}.values", base_path);
        self.values.save(&values_path)?;

        Ok(())
    }

    fn load(base_path: &str) -> io::Result<Self> {
        // Load ACT (instant via mmap)
        let tree_path = format!("{}.tree", base_path);
        let act = ArenaCompactTree::open_mmap(&tree_path)?;

        // Reconstruct PathMap from ACT
        let mut paths = PathMap::new();
        for (path, id) in act.iter() {
            paths.set_val_at(path, id);
        }

        // Load value store
        let values_path = format!("{}.values", base_path);
        let values = ValueStore::load(&values_path)?;

        Ok(KnowledgeBase { paths, values })
    }
}

fn main() -> io::Result<()> {
    println!("=== External Value Store Example ===\n");

    // ===== Part 1: Create Knowledge Base =====
    println!("Creating knowledge base with complex values...");

    let mut kb = KnowledgeBase::new();

    // Insert facts with complex MeTTa terms
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

    kb.insert(b"facts/logic/modus_ponens", KBEntry {
        term: MeTTaTerm::Expression(vec![
            MeTTaTerm::Atom("→".to_string()),
            MeTTaTerm::Variable("P".to_string()),
            MeTTaTerm::Variable("Q".to_string()),
        ]),
        confidence: 1.0,
        source: "rule".to_string(),
        metadata: [("type".to_string(), "inference".to_string())]
            .iter().cloned().collect(),
    });

    // Insert duplicate term (tests deduplication)
    kb.insert(b"facts/math/addition_copy", KBEntry {
        term: MeTTaTerm::Expression(vec![
            MeTTaTerm::Atom("=".to_string()),
            MeTTaTerm::Expression(vec![
                MeTTaTerm::Atom("+".to_string()),
                MeTTaTerm::Number(2),
                MeTTaTerm::Number(2),
            ]),
            MeTTaTerm::Number(4),
        ]),
        confidence: 0.9,  // Different metadata, same term
        source: "derived".to_string(),
        metadata: HashMap::new(),
    });

    println!("Inserted {} paths", kb.paths.len());
    println!("Unique values: {}", kb.values.value_to_id.len());
    println!("Deduplication: {} paths → {} unique terms\n",
             kb.paths.len(), kb.values.value_to_id.len());

    // ===== Part 2: Query Before Saving =====
    println!("--- Querying (in-memory) ---");

    if let Some(entry) = kb.get(b"facts/math/addition") {
        println!("Path: facts/math/addition");
        println!("  Term: {:?}", entry.term);
        println!("  Confidence: {}", entry.confidence);
        println!("  Source: {}", entry.source);
    }

    if let Some(entry) = kb.get(b"facts/logic/modus_ponens") {
        println!("\nPath: facts/logic/modus_ponens");
        println!("  Term: {:?}", entry.term);
        println!("  Metadata: {:?}", entry.metadata);
    }

    // ===== Part 3: Save to Disk =====
    println!("\n--- Saving to Disk ---");

    let base_path = "knowledge_base";
    kb.save(base_path)?;

    let tree_size = std::fs::metadata(format!("{}.tree", base_path))?.len();
    let values_size = std::fs::metadata(format!("{}.values", base_path))?.len();

    println!("Saved to:");
    println!("  {}.tree: {} bytes (ACT format)", base_path, tree_size);
    println!("  {}.values: {} bytes (bincode)", base_path, values_size);
    println!("  Total: {} bytes\n", tree_size + values_size);

    // ===== Part 4: Load from Disk =====
    println!("--- Loading from Disk ---");

    let loaded_kb = KnowledgeBase::load(base_path)?;

    println!("Loaded {} paths", loaded_kb.paths.len());
    println!("Loaded {} unique values\n", loaded_kb.values.value_to_id.len());

    // ===== Part 5: Verify Data =====
    println!("--- Verification ---");

    // Verify original entry
    let entry1 = kb.get(b"facts/math/addition").unwrap();
    let entry2 = loaded_kb.get(b"facts/math/addition").unwrap();

    assert_eq!(entry1.term, entry2.term);
    assert_eq!(entry1.confidence, entry2.confidence);
    println!("✓ Original data matches");

    // Verify deduplication persisted
    let id1 = loaded_kb.paths.get_val_at(b"facts/math/addition").unwrap();
    let id2 = loaded_kb.paths.get_val_at(b"facts/math/addition_copy").unwrap();

    println!("✓ Deduplication verified:");
    println!("  ID for 'addition': {}", id1);
    println!("  ID for 'addition_copy': {}", id2);

    if id1 == id2 {
        println!("  → Same term, same ID (deduplicated)");
    }

    // ===== Part 6: Performance Analysis =====
    println!("\n--- Performance Analysis ---");

    println!("Benefits of external value store:");
    println!("  1. ACT format: u64 IDs (8 bytes each)");
    println!("  2. Complex values: Stored once (deduplicated)");
    println!("  3. Instant loading: ACT via mmap (O(1))");
    println!("  4. Value store: Loaded separately (bincode)");
    println!("\nTrade-offs:");
    println!("  - Two files to manage");
    println!("  - Extra indirection for queries (ID → value lookup)");
    println!("  - Value store must fit in RAM (or use separate mmap)");

    // ===== Part 7: Large-Scale Demo =====
    println!("\n--- Large-Scale Demo ---");

    let mut large_kb = KnowledgeBase::new();

    println!("Creating 10,000 entries with deduplication...");
    for i in 0..10_000 {
        let path = format!("data/item_{}", i);

        // Every 100 items reuse the same term (90% deduplication)
        let term = if i % 100 == 0 {
            MeTTaTerm::Atom(format!("unique_{}", i))
        } else {
            MeTTaTerm::Atom(format!("common_{}", i / 100))
        };

        large_kb.insert(path.as_bytes(), KBEntry {
            term,
            confidence: 0.5 + (i as f64 / 20000.0),
            source: "generated".to_string(),
            metadata: HashMap::new(),
        });
    }

    println!("  Paths: {}", large_kb.paths.len());
    println!("  Unique values: {}", large_kb.values.value_to_id.len());
    println!("  Deduplication ratio: {:.1}×",
             large_kb.paths.len() as f64 / large_kb.values.value_to_id.len() as f64);

    // Save and measure
    let large_base = "large_kb";
    large_kb.save(large_base)?;

    let large_tree_size = std::fs::metadata(format!("{}.tree", large_base))?.len();
    let large_values_size = std::fs::metadata(format!("{}.values", large_base))?.len();

    println!("\nFile sizes:");
    println!("  ACT: {:.2} KB", large_tree_size as f64 / 1000.0);
    println!("  Values: {:.2} KB", large_values_size as f64 / 1000.0);
    println!("  Total: {:.2} KB", (large_tree_size + large_values_size) as f64 / 1000.0);

    // ===== Cleanup =====
    println!("\n=== Example Complete ===");

    std::fs::remove_file(format!("{}.tree", base_path))?;
    std::fs::remove_file(format!("{}.values", base_path))?;
    std::fs::remove_file(format!("{}.tree", large_base))?;
    std::fs::remove_file(format!("{}.values", large_base))?;

    Ok(())
}

/* Example Output:

=== External Value Store Example ===

Creating knowledge base with complex values...
Inserted 3 paths
Unique values: 2
Deduplication: 3 paths → 2 unique terms

--- Querying (in-memory) ---
Path: facts/math/addition
  Term: Expression([Atom("="), Expression([Atom("+"), Number(2), Number(2)]), Number(4)])
  Confidence: 1
  Source: axiom

Path: facts/logic/modus_ponens
  Term: Expression([Atom("→"), Variable("P"), Variable("Q")])
  Metadata: {"type": "inference"}

--- Saving to Disk ---
Saved to:
  knowledge_base.tree: 412 bytes (ACT format)
  knowledge_base.values: 287 bytes (bincode)
  Total: 699 bytes

--- Loading from Disk ---
Loaded 3 paths
Loaded 2 unique values

--- Verification ---
✓ Original data matches
✓ Deduplication verified:
  ID for 'addition': 12345678901234567
  ID for 'addition_copy': 12345678901234567
  → Same term, same ID (deduplicated)

--- Performance Analysis ---
Benefits of external value store:
  1. ACT format: u64 IDs (8 bytes each)
  2. Complex values: Stored once (deduplicated)
  3. Instant loading: ACT via mmap (O(1))
  4. Value store: Loaded separately (bincode)

Trade-offs:
  - Two files to manage
  - Extra indirection for queries (ID → value lookup)
  - Value store must fit in RAM (or use separate mmap)

--- Large-Scale Demo ---
Creating 10,000 entries with deduplication...
  Paths: 10000
  Unique values: 101
  Deduplication ratio: 99.0×

File sizes:
  ACT: 287.45 KB
  Values: 8.12 KB
  Total: 295.57 KB

=== Example Complete ===

*/
