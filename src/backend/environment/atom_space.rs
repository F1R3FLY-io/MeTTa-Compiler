//! AtomSpace: Unified atom storage with MettaTrie + variable atom support.
//!
//! Extracted from `GenericEnvironmentShared` to provide a shared storage backend
//! for both environment-based spaces (&self) and standalone spaces (new-space).
//!
//! ## Architecture
//!
//! All atoms (ground and variable) are stored in `MettaTrie<V, Multiplicity>`.
//! MettaTrie handles arbitrary arity natively, so no separate `wide_btm` is needed.
//! Variables are stored as `TrieKey::Atom("$x")` via `decompose_literal()`,
//! which preserves variable names as atoms for storage. Pattern queries use
//! `decompose()` which converts variables to `TrieKey::Variable` for wildcard matching.
//!
//! ## GC Safety
//!
//! `variable_atoms` holds `MettaValue` references that MUST be traced by the GC.
//! The MettaTrie stores original expressions at leaves — these are also GC roots
//! when V is a GC-managed type.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use metta_trie::{MettaTrie, Multiplicity};
use parking_lot::RwLock;

use super::bloom::{AtomicBloomFilter, HeadArityBloomFilter};
use crate::backend::models::MettaValueTrait;

/// Unified atom storage backed by MettaTrie + variable atom Vec.
///
/// All atoms are stored in MettaTrie keyed by decomposed TrieKey sequences.
/// Ground atoms use `decompose_literal()` for storage (variables kept as `Atom("$x")`).
/// Pattern queries use `decompose()` (variables become `Variable` for wildcard matching).
/// Bloom filters provide O(1) match rejection.
pub struct AtomSpace<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> {
    /// MettaTrie for atom storage (value = atom multiplicity).
    /// Rules are stored as `(= lhs rhs)` decomposed into TrieKey sequences.
    /// O(1) clone via Arc-based CoW structural sharing.
    pub(crate) btm: RwLock<MettaTrie<V, Multiplicity>>,

    /// Dedicated MettaTrie for type assertions `(: name type)`.
    /// Updated incrementally on every add_type/remove_type.
    /// O(1) CoW fork via `MettaTrie::clone()`.
    pub(crate) type_btm: RwLock<MettaTrie<V, Multiplicity>>,

    /// Dedicated MettaTrie for subtype relations `(:< sub super)`.
    /// Updated incrementally alongside the `subtypes` HashMap.
    /// O(1) CoW fork via `MettaTrie::clone()`.
    pub(crate) subtype_btm: RwLock<MettaTrie<V, Multiplicity>>,

    /// Phase 10.1: MettaTrie for inferred type assertions `(: name inferred_type)`.
    /// Stores inferred (not declared) types from rule RHS analysis. Enables structural
    /// pattern queries like "find all functions returning Number".
    /// RwLock justified: MettaTrie mutations require exclusive access.
    pub(crate) inferred_type_btm: RwLock<MettaTrie<V, Multiplicity>>,

    /// Phase 10.1: Atomic bloom filter for inferred function types -- fully lock-free.
    /// Reads: `load(Relaxed)` + bit test -- zero synchronization.
    /// Writes: `fetch_or(Relaxed, bit_mask)` -- lock-free CAS insertion.
    /// False positives harmless (fall through to DashMap lookup).
    pub(crate) inferred_type_bloom: Arc<AtomicBloomFilter>,

    /// Bloom filter for (head_symbol, arity) pairs -- enables O(1) match_space() rejection.
    /// Arc-wrapped so fork is O(1) (Arc::clone).
    pub(crate) head_arity_bloom: std::sync::Arc<RwLock<HeadArityBloomFilter>>,

    /// Bloom filter for atom names with type declarations -- enables O(1) rejection
    /// in get_type()/get_types_generic() for untyped atoms.
    /// Arc-wrapped so fork is O(1) (Arc::clone).
    pub(crate) type_bloom: std::sync::Arc<RwLock<super::bloom::TypeBloomFilter>>,

    /// O(1) total atom count (sum of all multiplicities across ground + variable atoms).
    pub(crate) total_atoms: AtomicUsize,

    /// Phase 10.5: Generation counter for inferred type changes.
    /// Incremented by `register_inferred_type()` on each new type registration.
    /// Used with `fixpoint_generation` to detect when new types have been registered
    /// since the last fixpoint run, triggering re-inference at the next eval boundary.
    pub(crate) inferred_type_generation: AtomicU64,

    /// Phase 10.5: Generation at which the last type fixpoint completed.
    /// Compared against `inferred_type_generation` to detect if fixpoint is needed.
    /// Updated via CAS to prevent concurrent/duplicate fixpoint runs.
    ///
    /// ## What is a fixpoint?
    ///
    /// A fixpoint (fixed point) is a value *x* that is unchanged by a function
    /// application: *f(x) = x*. In this type system, we iteratively re-infer the
    /// return types of mutually recursive functions until the inferred types stop
    /// changing -- i.e., re-inference produces the same type set as the previous
    /// iteration. That stable state is the fixpoint of the type inference function.
    /// State-based cycle detection guarantees termination even if types oscillate.
    pub(crate) fixpoint_generation: AtomicU64,

    /// Atoms containing variables, stored separately from the MettaTrie.
    /// Each entry is `(value, multiplicity)`. Expected to be very small (< 10 typically).
    ///
    /// ## Why Separate?
    ///
    /// MettaTrie stores variable atoms with their literal names (e.g., `Atom("$x")`),
    /// which means `query()` with `Variable` at the same position WILL match them.
    /// However, bidirectional matching (where stored variables can also match concrete
    /// query values) requires the separate Vec + `space_match_bidirectional_generic()`.
    pub(crate) variable_atoms: RwLock<Vec<(V, usize)>>,
}

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> AtomSpace<V> {
    /// Create a new empty AtomSpace with expected entry count for the Bloom filter.
    pub fn new(expected_entries: usize) -> Self {
        AtomSpace {
            btm: RwLock::new(MettaTrie::new()),
            type_btm: RwLock::new(MettaTrie::new()),
            subtype_btm: RwLock::new(MettaTrie::new()),
            inferred_type_btm: RwLock::new(MettaTrie::new()),
            inferred_type_bloom: Arc::new(AtomicBloomFilter::new(expected_entries / 10)),
            head_arity_bloom: std::sync::Arc::new(RwLock::new(
                HeadArityBloomFilter::new(expected_entries),
            )),
            type_bloom: std::sync::Arc::new(RwLock::new(
                super::bloom::TypeBloomFilter::new(expected_entries / 10),
            )),
            total_atoms: AtomicUsize::new(0),
            // Phase 10.5: both start at 0 -- no fixpoint needed until types are registered
            inferred_type_generation: AtomicU64::new(0),
            fixpoint_generation: AtomicU64::new(0),
            variable_atoms: RwLock::new(Vec::new()),
        }
    }

    /// Fork this AtomSpace for nondeterministic branch isolation.
    ///
    /// MettaTrie clone is O(1) via Arc-based CoW structural sharing.
    /// Bloom filter is Arc-cloned (shared until mutation).
    /// Variable atoms Vec is cloned (expected to be very small).
    pub fn fork(&self) -> Self {
        AtomSpace {
            btm: RwLock::new(self.btm.read().clone()),
            type_btm: RwLock::new(self.type_btm.read().clone()),
            subtype_btm: RwLock::new(self.subtype_btm.read().clone()),
            // Phase 10.1: inferred_type_btm is CoW-cloned (same as type_btm)
            inferred_type_btm: RwLock::new(self.inferred_type_btm.read().clone()),
            // Phase 10.1: bloom is append-only, safe to share via Arc::clone
            inferred_type_bloom: Arc::clone(&self.inferred_type_bloom),
            head_arity_bloom: std::sync::Arc::clone(&self.head_arity_bloom),
            type_bloom: std::sync::Arc::clone(&self.type_bloom),
            total_atoms: AtomicUsize::new(self.total_atoms.load(Ordering::Acquire)),
            // Phase 10.5: snapshot generation counters into forked AtomSpace
            inferred_type_generation: AtomicU64::new(
                self.inferred_type_generation.load(Ordering::Acquire),
            ),
            fixpoint_generation: AtomicU64::new(
                self.fixpoint_generation.load(Ordering::Acquire),
            ),
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
    /// Must be called during GC root collection to keep variable atoms alive.
    /// MettaTrie entries also store V values at leaves -- collect those too.
    pub fn collect_gc_roots(&self, roots: &mut Vec<V>) {
        // Variable atoms hold V values directly
        let var_atoms = self.variable_atoms.read();
        roots.reserve(var_atoms.len());
        for (val, _mult) in var_atoms.iter() {
            roots.push(val.clone());
        }

        // MettaTrie entries store V values at leaves
        let btm = self.btm.read();
        for (expr, _mult) in btm.iter() {
            roots.push(expr.clone());
        }
    }
}
