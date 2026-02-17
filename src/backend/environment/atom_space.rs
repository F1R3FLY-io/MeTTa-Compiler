//! AtomSpace: Unified atom storage with MORK PathMap + variable atom support.
//!
//! Extracted from `GenericEnvironmentShared` to provide a shared storage backend
//! for both environment-based spaces (&self) and standalone spaces (new-space).
//!
//! ## Architecture
//!
//! Ground atoms (no variables) are stored in the MORK PathMap trie with
//! multiplicity tracking. Variable atoms are stored separately in a Vec
//! (MORK trie search cannot find stored atoms with variables at concrete
//! query positions — see plan's MORK Capability Analysis).
//!
//! ## GC Safety
//!
//! `variable_atoms` and `large_expr_pathmap` hold `MettaValue` references that
//! MUST be traced by the GC. Callers must include `collect_gc_roots()` output
//! in their root set.

use std::sync::atomic::{AtomicUsize, Ordering};

use mork_interning::SharedMappingHandle;
use parking_lot::RwLock;
use pathmap::PathMap;

use super::bloom::HeadArityBloomFilter;
use super::multiplicity::Multiplicity;
use crate::backend::models::MettaValueTrait;

/// Unified atom storage backed by MORK PathMap + variable atom Vec.
///
/// Ground atoms are stored in the PathMap (literal MORK encoding) with Bloom filter
/// for O(1) match rejection. Variable atoms are stored separately since MORK's trie
/// search is directional — `query_multi()` cannot find stored atoms with variables
/// at positions where the query has concrete values.
pub struct AtomSpace<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> {
    /// PathMap trie for ground atom storage (value = atom multiplicity).
    /// Rules are stored as `(= lhs rhs)` MORK byte keys.
    pub(crate) btm: RwLock<PathMap<Multiplicity>>,

    /// MORK symbol interning handle. Shared across all forks (Arc-wrapped internally).
    pub(crate) shared_mapping: SharedMappingHandle,

    /// Bloom filter for (head_symbol, arity) pairs — enables O(1) match_space() rejection.
    /// Arc-wrapped so fork is O(1) (Arc::clone).
    pub(crate) head_arity_bloom: std::sync::Arc<RwLock<HeadArityBloomFilter>>,

    /// Fallback store for large expressions (arity >= 64) that can't be MORK-encoded.
    /// Stores V directly (zero-conversion).
    pub(crate) large_expr_pathmap: RwLock<Option<PathMap<V>>>,

    /// O(1) total atom count (sum of all multiplicities across ground + variable atoms).
    pub(crate) total_atoms: AtomicUsize,

    /// Atoms containing variables, stored separately from the PathMap trie.
    /// Each entry is `(value, multiplicity)`. Expected to be very small (< 10 typically).
    ///
    /// ## Why Separate?
    ///
    /// MORK's `query_multi()` uses directional trie traversal: when the query has a
    /// concrete value at some position, it descends to that exact byte — missing any
    /// stored atoms with variables at that position. By storing variable atoms in a
    /// Vec and matching them with `space_match_bidirectional_generic()`, we get correct
    /// bidirectional matching semantics.
    pub(crate) variable_atoms: RwLock<Vec<(V, usize)>>,
}

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> AtomSpace<V> {
    /// Create a new empty AtomSpace with the given MORK symbol interning handle
    /// and expected entry count for the Bloom filter.
    pub fn new(shared_mapping: SharedMappingHandle, expected_entries: usize) -> Self {
        AtomSpace {
            btm: RwLock::new(PathMap::new()),
            shared_mapping,
            head_arity_bloom: std::sync::Arc::new(RwLock::new(
                HeadArityBloomFilter::new(expected_entries),
            )),
            large_expr_pathmap: RwLock::new(None),
            total_atoms: AtomicUsize::new(0),
            variable_atoms: RwLock::new(Vec::new()),
        }
    }

    /// Fork this AtomSpace for nondeterministic branch isolation.
    ///
    /// PathMap clone is O(1) CoW. Bloom filter is Arc-cloned (shared until mutation).
    /// Variable atoms Vec is cloned (expected to be very small).
    pub fn fork(&self) -> Self {
        AtomSpace {
            btm: RwLock::new(self.btm.read().clone()),
            shared_mapping: self.shared_mapping.clone(),
            head_arity_bloom: std::sync::Arc::clone(&self.head_arity_bloom),
            large_expr_pathmap: RwLock::new(self.large_expr_pathmap.read().clone()),
            total_atoms: AtomicUsize::new(self.total_atoms.load(Ordering::Acquire)),
            variable_atoms: RwLock::new(self.variable_atoms.read().clone()),
        }
    }

    /// Get the total atom count (O(1)).
    #[inline]
    pub fn atom_count(&self) -> usize {
        self.total_atoms.load(Ordering::Acquire)
    }

    /// Collect all GC-traceable MettaValues from this AtomSpace.
    ///
    /// Must be called during GC root collection to keep variable atoms and
    /// large expression PathMap values alive.
    pub fn collect_gc_roots(&self, roots: &mut Vec<V>) {
        // Variable atoms hold V values directly
        {
            let var_atoms = self.variable_atoms.read();
            roots.reserve(var_atoms.len());
            for (val, _mult) in var_atoms.iter() {
                roots.push(val.clone());
            }
        }

        // Large expression PathMap values
        {
            let large_pm = self.large_expr_pathmap.read();
            if let Some(ref pm) = *large_pm {
                roots.extend(pm.iter().map(|(_, val)| val.clone()));
            }
        }
    }
}
