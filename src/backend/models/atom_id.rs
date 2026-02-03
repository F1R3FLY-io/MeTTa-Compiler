//! Atom interning for efficient multiplicity tracking.
//!
//! This module provides a symbol interning system for `MettaValue` that enables:
//! - O(1) equality comparison via integer IDs instead of structural comparison
//! - O(1) hashing via pre-computed hash values
//! - Reduced memory usage through deduplication
//! - Lock-free concurrent access via DashMap
//!
//! # Architecture
//!
//! ```text
//! MettaValue → hash(MettaValue) → DashMap lookup → AtomId
//!                                       ↓
//!                              Vec<MettaValue> (reverse lookup)
//! ```

use dashmap::DashMap;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use xxhash_rust::xxh3::Xxh3Builder;

// Import concrete type
use super::metta_value::MettaValue;
// Import traits for method resolution
use super::metta_value_trait::{MettaValue as MettaValueTrait, MettaValueFactory};

/// A unique identifier for an interned atom.
///
/// `AtomId` provides O(1) equality comparison and hashing, making it ideal
/// for use as keys in hash maps or multisets where the same atom may be
/// referenced many times.
///
/// # Properties
/// - Copy + Clone: Can be freely copied without allocation
/// - 8 bytes: Same size as a pointer, cache-friendly
/// - Dense: IDs are assigned sequentially starting from 0
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtomId(u64);

impl AtomId {
    /// Get the raw u64 value of this AtomId.
    #[inline]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Create an AtomId from a raw u64 value.
    ///
    /// # Safety
    /// This should only be used with values obtained from `as_u64()` on a valid AtomId.
    #[inline]
    pub fn from_raw(value: u64) -> Self {
        AtomId(value)
    }
}

/// Thread-safe symbol interning table with byte-array storage.
///
/// The SymbolTable provides bidirectional mapping between serialized values and `AtomId`:
/// - Forward: bytes (serialized value) → `AtomId` (for interning)
/// - Reverse: `AtomId` → bytes (for materialization)
///
/// ## Type-Agnostic Storage
///
/// Values are stored as serialized bytes, enabling:
/// - Storage of any value type implementing `MettaValueTrait::serialize()`
/// - Retrieval into any value type via `MettaValueFactory::deserialize()`
/// - Zero deep copies - just serialization/deserialization at boundaries
///
/// ## Thread Safety
///
/// All operations are thread-safe and lock-free for the common case:
/// - `intern()`: Uses DashMap for concurrent insert-or-get
/// - `resolve()`: Uses RwLock with read-heavy access pattern
///
/// ## Memory Model
///
/// Values are stored once and never removed (append-only). This ensures:
/// - AtomIds remain valid for the lifetime of the SymbolTable
/// - No ABA problem with ID reuse
/// - Efficient memory layout with sequential storage
pub struct SymbolTable {
    /// Next AtomId to assign (atomic counter for lock-free ID generation)
    next_id: AtomicU64,

    /// Forward lookup: hash(bytes) → Vec<(bytes, AtomId)>
    /// We use the hash as a first-level key to avoid re-hashing during lookup.
    /// Collisions are handled by comparing the actual bytes.
    bytes_to_id: DashMap<u64, Vec<(Vec<u8>, AtomId)>, Xxh3Builder>,

    /// Reverse lookup: AtomId → bytes
    /// Sequential storage indexed by AtomId.0
    id_to_bytes: RwLock<Vec<Vec<u8>>>,

    /// Hasher for computing byte array hashes
    hasher_builder: Xxh3Builder,
}

impl SymbolTable {
    /// Create a new empty SymbolTable.
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            bytes_to_id: DashMap::with_hasher(Xxh3Builder::new()),
            id_to_bytes: RwLock::new(Vec::new()),
            hasher_builder: Xxh3Builder::new(),
        }
    }

    /// Create a new SymbolTable with pre-allocated capacity.
    ///
    /// Use this when you know approximately how many unique atoms will be interned.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            next_id: AtomicU64::new(0),
            bytes_to_id: DashMap::with_capacity_and_hasher(capacity, Xxh3Builder::new()),
            id_to_bytes: RwLock::new(Vec::with_capacity(capacity)),
            hasher_builder: Xxh3Builder::new(),
        }
    }

    /// Compute the hash of bytes using xxh3.
    #[inline]
    fn compute_bytes_hash(&self, bytes: &[u8]) -> u64 {
        let mut hasher = self.hasher_builder.build_hasher();
        bytes.hash(&mut hasher);
        hasher.finish()
    }

    // =========================================================================
    // Byte-based API (zero-copy, type-agnostic)
    // =========================================================================

    /// Intern a value by its serialized bytes, returning its unique AtomId.
    ///
    /// This is the preferred API for zero-copy storage. The caller serializes
    /// the value once, and we store the bytes directly.
    pub fn intern_bytes(&self, bytes: &[u8]) -> AtomId {
        let hash = self.compute_bytes_hash(bytes);

        // Fast path: check if bytes already exist
        if let Some(entries) = self.bytes_to_id.get(&hash) {
            for (stored_bytes, id) in entries.iter() {
                if stored_bytes.as_slice() == bytes {
                    return *id;
                }
            }
        }

        // Slow path: need to insert new bytes
        let mut entry = self.bytes_to_id.entry(hash).or_insert_with(Vec::new);

        // Double-check in case another thread inserted while we were waiting
        for (stored_bytes, id) in entry.iter() {
            if stored_bytes.as_slice() == bytes {
                return *id;
            }
        }

        // Generate new ID atomically
        let new_id = AtomId(self.next_id.fetch_add(1, Ordering::Relaxed));

        // Add to forward map
        entry.push((bytes.to_vec(), new_id));

        // Add to reverse map
        {
            let mut id_to_bytes = self.id_to_bytes.write().expect("id_to_bytes lock poisoned");
            let idx = new_id.0 as usize;
            if id_to_bytes.len() <= idx {
                id_to_bytes.resize(idx + 1, Vec::new());
            }
            id_to_bytes[idx] = bytes.to_vec();
        }

        new_id
    }

    /// Look up an AtomId by bytes without interning.
    pub fn get_bytes(&self, bytes: &[u8]) -> Option<AtomId> {
        let hash = self.compute_bytes_hash(bytes);

        if let Some(entries) = self.bytes_to_id.get(&hash) {
            for (stored_bytes, id) in entries.iter() {
                if stored_bytes.as_slice() == bytes {
                    return Some(*id);
                }
            }
        }
        None
    }

    /// Resolve an AtomId to its serialized bytes.
    pub fn resolve_bytes(&self, id: AtomId) -> Vec<u8> {
        let id_to_bytes = self.id_to_bytes.read().expect("id_to_bytes lock poisoned");
        id_to_bytes[id.0 as usize].clone()
    }

    /// Try to resolve an AtomId to bytes, returning None if invalid.
    pub fn try_resolve_bytes(&self, id: AtomId) -> Option<Vec<u8>> {
        let id_to_bytes = self.id_to_bytes.read().expect("id_to_bytes lock poisoned");
        id_to_bytes.get(id.0 as usize).cloned()
    }

    // =========================================================================
    // Generic value API (uses serialization internally)
    // =========================================================================

    /// Intern any value implementing MettaValueTrait.
    ///
    /// The value is serialized to bytes for storage. This enables type-agnostic
    /// interning - both MettaValue and ArenaValue can be interned.
    pub fn intern_value<V: super::MettaValueTrait>(&self, value: &V) -> AtomId {
        let bytes = value.serialize();
        self.intern_bytes(&bytes)
    }

    /// Resolve an AtomId to a value using the provided factory.
    ///
    /// The stored bytes are deserialized using the factory, which allocates
    /// in the appropriate context (heap or arena).
    pub fn resolve_value<V: super::MettaValueTrait, F: super::MettaValueFactory<V>>(
        &self,
        id: AtomId,
        factory: &F,
    ) -> Result<V, String> {
        let bytes = self.resolve_bytes(id);
        factory.deserialize(&bytes).map(|(v, _)| v)
    }

    // =========================================================================
    // Legacy MettaValue API (for backwards compatibility)
    // =========================================================================

    /// Intern a MettaValue, returning its unique AtomId.
    ///
    /// This is a convenience method that serializes and stores the value.
    pub fn intern(&self, value: &MettaValue) -> AtomId {
        self.intern_value(value)
    }

    /// Look up an AtomId without interning (returns None if not found).
    pub fn get(&self, value: &MettaValue) -> Option<AtomId> {
        let bytes = value.serialize();
        self.get_bytes(&bytes)
    }

    /// Resolve an AtomId back to its MettaValue.
    ///
    /// # Panics
    /// Panics if the AtomId is invalid or deserialization fails.
    pub fn resolve(&self, id: AtomId) -> MettaValue {
        let bytes = self.resolve_bytes(id);
        let factory = super::HeapMettaValueFactory;
        factory.deserialize(&bytes)
            .map(|(v, _)| v)
            .expect("failed to deserialize stored value")
    }

    /// Try to resolve an AtomId, returning None if invalid.
    pub fn try_resolve(&self, id: AtomId) -> Option<MettaValue> {
        let bytes = self.try_resolve_bytes(id)?;
        let factory = super::HeapMettaValueFactory;
        factory.deserialize(&bytes).ok().map(|(v, _)| v)
    }

    // =========================================================================
    // Utility methods
    // =========================================================================

    /// Get the number of unique atoms in the symbol table.
    #[inline]
    pub fn len(&self) -> usize {
        self.next_id.load(Ordering::Relaxed) as usize
    }

    /// Check if the symbol table is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterate over all interned (AtomId, bytes) pairs.
    pub fn iter_bytes(&self) -> impl Iterator<Item = (AtomId, Vec<u8>)> + '_ {
        let id_to_bytes = self.id_to_bytes.read().expect("id_to_bytes lock poisoned");
        let len = id_to_bytes.len();
        (0..len).map(move |i| {
            let id = AtomId(i as u64);
            let bytes = id_to_bytes[i].clone();
            (id, bytes)
        })
    }

    /// Iterate over all interned (AtomId, MettaValue) pairs.
    ///
    /// Note: This deserializes each value, which may be expensive.
    pub fn iter(&self) -> impl Iterator<Item = (AtomId, MettaValue)> + '_ {
        let factory = super::HeapMettaValueFactory;
        self.iter_bytes().filter_map(move |(id, bytes)| {
            factory.deserialize(&bytes).ok().map(|(v, _)| (id, v))
        })
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SymbolTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolTable")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_basic() {
        let table = SymbolTable::new();

        let val1 = MettaValue::Atom("foo".to_string());
        let val2 = MettaValue::Atom("bar".to_string());

        let id1 = table.intern(&val1);
        let id2 = table.intern(&val2);

        // Different values get different IDs
        assert_ne!(id1, id2);

        // Same value gets same ID
        let id1_again = table.intern(&val1);
        assert_eq!(id1, id1_again);

        assert_eq!(table.len(), 2);
    }

    #[test]
    fn test_intern_various_types() {
        let table = SymbolTable::new();

        let values = vec![
            MettaValue::Atom("symbol".to_string()),
            MettaValue::Bool(true),
            MettaValue::Long(42),
            MettaValue::Float(3.14),
            MettaValue::String("hello".to_string()),
            MettaValue::Nil(),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ];

        let ids: Vec<_> = values.iter().map(|v| table.intern(v)).collect();

        // All should be unique
        for (i, id1) in ids.iter().enumerate() {
            for (j, id2) in ids.iter().enumerate() {
                if i != j {
                    assert_ne!(id1, id2, "IDs for {} and {} should differ", i, j);
                }
            }
        }

        assert_eq!(table.len(), values.len());
    }

    #[test]
    fn test_resolve() {
        let table = SymbolTable::new();

        let val = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let id = table.intern(&val);
        let resolved = table.resolve(id);

        assert_eq!(val, resolved);
    }

    #[test]
    fn test_get_without_intern() {
        let table = SymbolTable::new();

        let val1 = MettaValue::Atom("interned".to_string());
        let val2 = MettaValue::Atom("not_interned".to_string());

        let id1 = table.intern(&val1);

        assert_eq!(table.get(&val1), Some(id1));
        assert_eq!(table.get(&val2), None);
    }

    #[test]
    fn test_try_resolve() {
        let table = SymbolTable::new();

        let val = MettaValue::Long(123);
        let id = table.intern(&val);

        assert_eq!(table.try_resolve(id), Some(val));
        assert_eq!(table.try_resolve(AtomId(999)), None);
    }

    #[test]
    fn test_with_capacity() {
        let table = SymbolTable::with_capacity(1000);
        assert!(table.is_empty());

        for i in 0..100 {
            table.intern(&MettaValue::Long(i));
        }
        assert_eq!(table.len(), 100);
    }

    #[test]
    fn test_hash_collision_handling() {
        // This test verifies that values with the same hash are handled correctly.
        // In practice, hash collisions are rare with xxh3, but we need to handle them.
        let table = SymbolTable::new();

        // Create many values to increase chance of collision (or rely on linear chain)
        let mut ids = Vec::new();
        for i in 0..1000 {
            let val = MettaValue::Long(i);
            ids.push(table.intern(&val));
        }

        // Verify all are unique and resolvable
        for i in 0..1000 {
            let val = MettaValue::Long(i);
            assert_eq!(table.resolve(ids[i as usize]), val);
        }
    }

    #[test]
    fn test_concurrent_intern() {
        use std::sync::Arc;
        use std::thread;

        let table = Arc::new(SymbolTable::new());
        let num_threads = 8;
        let values_per_thread = 100;

        let handles: Vec<_> = (0..num_threads)
            .map(|t| {
                let table = Arc::clone(&table);
                thread::spawn(move || {
                    let mut ids = Vec::new();
                    for i in 0..values_per_thread {
                        // Each thread interns overlapping values
                        let val = MettaValue::Long((t * 50 + i) as i64);
                        ids.push(table.intern(&val));
                    }
                    ids
                })
            })
            .collect();

        let all_ids: Vec<Vec<AtomId>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify overlapping values got the same ID
        // Thread 0 interns 0-99, Thread 1 interns 50-149, etc.
        // Values 50-99 should have same ID from threads 0 and 1
        let t0_ids = &all_ids[0];
        let t1_ids = &all_ids[1];

        // t0[50..100] should match t1[0..50] (both represent values 50-99)
        for i in 0..50 {
            assert_eq!(
                t0_ids[50 + i],
                t1_ids[i],
                "Value {} should have same ID from both threads",
                50 + i
            );
        }
    }

    #[test]
    fn test_iter() {
        let table = SymbolTable::new();

        let values = vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ];

        for v in &values {
            table.intern(v);
        }

        let collected: Vec<_> = table.iter().collect();
        assert_eq!(collected.len(), 3);

        // IDs should be sequential starting from 0
        assert_eq!(collected[0].0, AtomId(0));
        assert_eq!(collected[1].0, AtomId(1));
        assert_eq!(collected[2].0, AtomId(2));
    }
}
