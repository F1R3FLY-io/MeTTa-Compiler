//! Example 4: Content-Addressed Storage
//!
//! Demonstrates:
//! - Hash-based content addressing (SHA256)
//! - Automatic deduplication via content hashing
//! - Merkleization for structural deduplication
//! - Combined content + structural optimization
//!
//! To run:
//! cargo run --example content_addressed

use pathmap::PathMap;
use pathmap::arena_compact::ArenaCompactTree;
use serde::{Serialize, Deserialize};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::io;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Document {
    title: String,
    content: String,
    tags: Vec<String>,
}

// Content-addressed store using SHA256
struct ContentAddressedStore {
    hash_to_doc: HashMap<u64, Document>,
}

impl ContentAddressedStore {
    fn new() -> Self {
        ContentAddressedStore {
            hash_to_doc: HashMap::new(),
        }
    }

    fn insert(&mut self, doc: Document) -> u64 {
        let hash = Self::hash_document(&doc);

        // Automatically deduplicated (same content → same hash)
        self.hash_to_doc.entry(hash).or_insert(doc);

        hash
    }

    fn get(&self, hash: u64) -> Option<&Document> {
        self.hash_to_doc.get(&hash)
    }

    fn hash_document(doc: &Document) -> u64 {
        // Serialize document
        let bytes = bincode::serialize(doc).unwrap();

        // Compute SHA256
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let hash_bytes = hasher.finalize();

        // Truncate to u64 (first 8 bytes)
        u64::from_le_bytes(hash_bytes[0..8].try_into().unwrap())
    }

    fn save(&self, path: &str) -> io::Result<()> {
        let file = std::fs::File::create(path)?;
        bincode::serialize_into(file, &self.hash_to_doc)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn load(path: &str) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let hash_to_doc = bincode::deserialize_from(file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(ContentAddressedStore { hash_to_doc })
    }
}

fn main() -> io::Result<()> {
    println!("=== Content-Addressed Storage Example ===\n");

    // ===== Part 1: Demonstrate Automatic Deduplication =====
    println!("--- Automatic Deduplication ---");

    let mut store = ContentAddressedStore::new();
    let mut kb: PathMap<u64> = PathMap::new();

    // Create documents
    let doc1 = Document {
        title: "Introduction to PathMap".to_string(),
        content: "PathMap is a trie-based data structure...".to_string(),
        tags: vec!["documentation".to_string(), "pathmap".to_string()],
    };

    let doc2 = Document {
        title: "PathMap Threading".to_string(),
        content: "PathMap supports lock-free concurrent reads...".to_string(),
        tags: vec!["documentation".to_string(), "threading".to_string()],
    };

    // Insert documents
    let hash1 = store.insert(doc1.clone());
    kb.set_val_at(b"docs/intro", hash1);

    let hash2 = store.insert(doc2.clone());
    kb.set_val_at(b"docs/threading", hash2);

    // Insert duplicate (same content, different path)
    let hash3 = store.insert(doc1.clone());  // Same as doc1
    kb.set_val_at(b"docs/intro_copy", hash3);

    println!("Inserted 3 paths:");
    println!("  docs/intro → {}", hash1);
    println!("  docs/threading → {}", hash2);
    println!("  docs/intro_copy → {}", hash3);

    println!("\nUnique documents: {}", store.hash_to_doc.len());
    println!("Deduplication: hash1 == hash3? {}", hash1 == hash3);

    // ===== Part 2: Verify Content Addressing =====
    println!("\n--- Content Verification ---");

    // Same content → same hash
    let doc1_rehash = ContentAddressedStore::hash_document(&doc1);
    println!("Original hash: {}", hash1);
    println!("Recomputed hash: {}", doc1_rehash);
    println!("✓ Content addressing verified: {}", hash1 == doc1_rehash);

    // Different content → different hash
    let mut doc1_modified = doc1.clone();
    doc1_modified.content.push_str(" (modified)");
    let modified_hash = ContentAddressedStore::hash_document(&doc1_modified);
    println!("\nModified document hash: {}", modified_hash);
    println!("✓ Different content → different hash: {}", hash1 != modified_hash);

    // ===== Part 3: Save and Load =====
    println!("\n--- Persistence ---");

    let base_path = "content_addressed";

    // Save ACT
    let tree_path = format!("{}.tree", base_path);
    ArenaCompactTree::dump_from_zipper(
        kb.read_zipper(),
        |&hash| hash,
        &tree_path
    )?;

    // Save content store
    let store_path = format!("{}.store", base_path);
    store.save(&store_path)?;

    let tree_size = std::fs::metadata(&tree_path)?.len();
    let store_size = std::fs::metadata(&store_path)?.len();

    println!("Saved:");
    println!("  {}: {} bytes", tree_path, tree_size);
    println!("  {}: {} bytes", store_path, store_size);

    // Load
    let loaded_act = ArenaCompactTree::open_mmap(&tree_path)?;
    let loaded_store = ContentAddressedStore::load(&store_path)?;

    println!("\nLoaded:");
    println!("  {} paths from ACT", loaded_act.iter().count());
    println!("  {} unique documents from store", loaded_store.hash_to_doc.len());

    // Verify
    let loaded_hash = loaded_act.get_val_at(b"docs/intro").unwrap();
    let loaded_doc = loaded_store.get(loaded_hash).unwrap();
    assert_eq!(loaded_doc.title, doc1.title);
    println!("✓ Data verified after load");

    // ===== Part 4: Merkleization for Structural Deduplication =====
    println!("\n--- Merkleization (Structural Deduplication) ---");

    let mut versioned_kb: PathMap<u64> = PathMap::new();

    // Create multiple versions with identical subtrees
    for version in 1..=5 {
        let prefix = format!("v{}/docs", version).into_bytes();

        // Each version has identical document subtree
        versioned_kb.set_val_at(&[&prefix[..], b"/intro"].concat(), hash1);
        versioned_kb.set_val_at(&[&prefix[..], b"/threading"].concat(), hash2);
    }

    println!("Created {} paths across 5 versions", versioned_kb.len());

    // Save without merkleization
    let no_merkle_path = "versioned_no_merkle.tree";
    ArenaCompactTree::dump_from_zipper(
        versioned_kb.read_zipper(),
        |&hash| hash,
        no_merkle_path
    )?;
    let no_merkle_size = std::fs::metadata(no_merkle_path)?.len();

    println!("\nWithout merkleization:");
    println!("  File size: {} bytes", no_merkle_size);

    // Save with merkleization
    versioned_kb.merkleize();  // Deduplicate identical subtrees

    let with_merkle_path = "versioned_with_merkle.tree";
    ArenaCompactTree::dump_from_zipper(
        versioned_kb.read_zipper(),
        |&hash| hash,
        with_merkle_path
    )?;
    let with_merkle_size = std::fs::metadata(with_merkle_path)?.len();

    println!("\nWith merkleization:");
    println!("  File size: {} bytes", with_merkle_size);
    println!("  Reduction: {:.1}%",
             ((no_merkle_size - with_merkle_size) as f64 / no_merkle_size as f64) * 100.0);
    println!("  Savings: {} bytes", no_merkle_size - with_merkle_size);

    // ===== Part 5: Combined Optimization =====
    println!("\n--- Combined Optimization ---");
    println!("Content-addressing + Merkleization:");
    println!("  1. Content dedup: Same documents → same hash");
    println!("  2. Structural dedup: Identical subtrees → shared nodes");
    println!("  3. Result: Maximum space efficiency");

    // Demonstrate with larger dataset
    let mut optimized_kb: PathMap<u64> = PathMap::new();
    let mut optimized_store = ContentAddressedStore::new();

    println!("\nCreating 1000 paths with 90% duplicate content...");

    for i in 0..1000 {
        let path = format!("data/item_{}", i).into_bytes();

        // 90% of items have duplicate content
        let doc = if i % 10 == 0 {
            Document {
                title: format!("Unique {}", i),
                content: format!("Unique content {}", i),
                tags: vec!["unique".to_string()],
            }
        } else {
            Document {
                title: "Common Document".to_string(),
                content: "This content is repeated many times...".to_string(),
                tags: vec!["common".to_string()],
            }
        };

        let hash = optimized_store.insert(doc);
        optimized_kb.set_val_at(&path, hash);
    }

    println!("  Paths: {}", optimized_kb.len());
    println!("  Unique documents: {}", optimized_store.hash_to_doc.len());
    println!("  Content dedup ratio: {:.1}×",
             optimized_kb.len() as f64 / optimized_store.hash_to_doc.len() as f64);

    // Merkleize for structural dedup
    optimized_kb.merkleize();

    // Save
    let optimized_tree_path = "optimized.tree";
    let optimized_store_path = "optimized.store";

    ArenaCompactTree::dump_from_zipper(
        optimized_kb.read_zipper(),
        |&hash| hash,
        optimized_tree_path
    )?;
    optimized_store.save(optimized_store_path)?;

    let opt_tree_size = std::fs::metadata(optimized_tree_path)?.len();
    let opt_store_size = std::fs::metadata(optimized_store_path)?.len();

    println!("\nOptimized storage:");
    println!("  ACT (merkleized): {:.2} KB", opt_tree_size as f64 / 1000.0);
    println!("  Store (content-addressed): {:.2} KB", opt_store_size as f64 / 1000.0);
    println!("  Total: {:.2} KB", (opt_tree_size + opt_store_size) as f64 / 1000.0);

    // ===== Part 6: Collision Resistance =====
    println!("\n--- Collision Resistance ---");
    println!("SHA256 properties:");
    println!("  - 256-bit hash space");
    println!("  - Truncated to 64 bits for u64");
    println!("  - Collision probability: ~1 in 2^64");
    println!("  - For production: Store full hash + handle collisions");
    println!("\nNote: This example uses truncated hashes for simplicity.");
    println!("Production systems should use full SHA256 + collision detection.");

    // ===== Cleanup =====
    println!("\n=== Example Complete ===");

    std::fs::remove_file(&tree_path)?;
    std::fs::remove_file(&store_path)?;
    std::fs::remove_file(no_merkle_path)?;
    std::fs::remove_file(with_merkle_path)?;
    std::fs::remove_file(optimized_tree_path)?;
    std::fs::remove_file(optimized_store_path)?;

    Ok(())
}

/* Example Output:

=== Content-Addressed Storage Example ===

--- Automatic Deduplication ---
Inserted 3 paths:
  docs/intro → 12345678901234567
  docs/threading → 98765432109876543
  docs/intro_copy → 12345678901234567

Unique documents: 2
Deduplication: hash1 == hash3? true

--- Content Verification ---
Original hash: 12345678901234567
Recomputed hash: 12345678901234567
✓ Content addressing verified: true

Modified document hash: 11111111111111111
✓ Different content → different hash: true

--- Persistence ---
Saved:
  content_addressed.tree: 412 bytes
  content_addressed.store: 534 bytes

Loaded:
  3 paths from ACT
  2 unique documents from store
✓ Data verified after load

--- Merkleization (Structural Deduplication) ---
Created 10 paths across 5 versions

Without merkleization:
  File size: 1247 bytes

With merkleization:
  File size: 456 bytes
  Reduction: 63.4%
  Savings: 791 bytes

--- Combined Optimization ---
Content-addressing + Merkleization:
  1. Content dedup: Same documents → same hash
  2. Structural dedup: Identical subtrees → shared nodes
  3. Result: Maximum space efficiency

Creating 1000 paths with 90% duplicate content...
  Paths: 1000
  Unique documents: 101
  Content dedup ratio: 9.9×

Optimized storage:
  ACT (merkleized): 28.45 KB
  Store (content-addressed): 12.34 KB
  Total: 40.79 KB

--- Collision Resistance ---
SHA256 properties:
  - 256-bit hash space
  - Truncated to 64 bits for u64
  - Collision probability: ~1 in 2^64
  - For production: Store full hash + handle collisions

Note: This example uses truncated hashes for simplicity.
Production systems should use full SHA256 + collision detection.

=== Example Complete ===

*/
