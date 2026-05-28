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
//! Wide expressions (arity ≥ 64) are stored in `wide_btm` using Wide MORK
//! encoding (tag-byte + LEB128).  Same `PathMap<Multiplicity>` type as `btm`,
//! values reconstructed on-demand via `wide_bytes_to_generic_value()`.
//!
//! ## GC Safety
//!
//! `variable_atoms` holds `MettaValue` references that MUST be traced by the GC.
//! `btm` and `wide_btm` store only byte keys + Multiplicity — no V references,
//! no GC tracing needed.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use mork_interning::SharedMappingHandle;
use parking_lot::RwLock;
use pathmap::PathMap;

use super::act_tiered::ActBase;
use super::bloom::{AtomicBloomFilter, HeadArityBloomFilter};
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

    /// PathMap trie for wide expressions (arity ≥ 64).  Same type as `btm`.
    /// Keyed by Wide MORK storage bytes.  Values reconstructed on-demand via
    /// `wide_bytes_to_generic_value()` — no duplicate value storage.
    pub(crate) wide_btm: RwLock<PathMap<Multiplicity>>,

    /// Dedicated PathMap for type assertions `(: name type)`.
    /// Updated incrementally on every add_type/remove_type — no lazy `restrict()` rebuild.
    /// O(1) CoW fork via `PathMap::clone()`.
    pub(crate) type_btm: RwLock<PathMap<Multiplicity>>,

    /// Dedicated PathMap for subtype relations `(:< sub super)`.
    /// Updated incrementally alongside the `subtypes` HashMap.
    /// O(1) CoW fork via `PathMap::clone()`.
    pub(crate) subtype_btm: RwLock<PathMap<Multiplicity>>,

    /// Phase 10.1: MORK PathMap for inferred type assertions `(: name inferred_type)`.
    /// Stores inferred (not declared) types from rule RHS analysis. Enables structural
    /// pattern queries like "find all functions returning Number".
    /// RwLock justified: PathMap trie mutations require exclusive access.
    pub(crate) inferred_type_btm: RwLock<PathMap<Multiplicity>>,

    /// Phase 10.1: Atomic bloom filter for inferred function types — fully lock-free.
    /// Reads: `load(Relaxed)` + bit test — zero synchronization.
    /// Writes: `fetch_or(Relaxed, bit_mask)` — lock-free CAS insertion.
    /// False positives harmless (fall through to DashMap lookup).
    pub(crate) inferred_type_bloom: Arc<AtomicBloomFilter>,

    /// MORK symbol interning handle. Shared across all forks (Arc-wrapped internally).
    pub(crate) shared_mapping: SharedMappingHandle,

    /// Bloom filter for (head_symbol, arity) pairs — enables O(1) match_space() rejection.
    /// Contains BOTH rule heads AND data atom heads. Arc-wrapped so fork is O(1).
    pub(crate) head_arity_bloom: std::sync::Arc<RwLock<HeadArityBloomFilter>>,

    /// Bloom filter for rule heads ONLY — used by `is_normal_form_bounded` to distinguish
    /// data constructors (not reducible) from rule heads (potentially reducible).
    /// Only updated by `add_rule()`, never by `add-atom()` for non-rule atoms.
    pub(crate) rule_head_bloom: std::sync::Arc<RwLock<HeadArityBloomFilter>>,

    /// Bloom filter for atom names with type declarations — enables O(1) rejection
    /// in get_type()/get_types_generic() for untyped atoms.
    /// Arc-wrapped so fork is O(1) (Arc::clone).
    pub(crate) type_bloom: std::sync::Arc<RwLock<super::bloom::TypeBloomFilter>>,

    /// O(1) total atom count (sum of all multiplicities across ground + variable atoms).
    pub(crate) total_atoms: AtomicUsize,

    /// Monotonic high-water counter of variable-CONTAINING non-rule atom additions to
    /// `btm` (literal encoding). MORK `query_multi` is directional and cannot match a
    /// stored atom whose variable sits where the query is concrete (see the
    /// `variable_atoms` doc below), so the conjunction ProductZipper fast path
    /// (`match_conjunction_query_multi`) is only COMPLETE when no variable-containing
    /// fact exists in `btm`. This counter is the gate: `== 0` ⟹ safe to use the fast
    /// path; `> 0` ⟹ fall back to the bidirectional iterative join.
    ///
    /// Incremented by `add_to_space`/`add_to_space_shared` when `has_variables_fast()`
    /// (rules are De-Bruijn-encoded and excluded — they early-return before the count
    /// site; the conjunction gate separately rejects `=`-headed goals). It is
    /// deliberately MONOTONIC (never decremented), exactly like the space's bloom
    /// filters: the load-bearing safety invariant is "> 0 whenever a variable fact is
    /// or was present", and a conservative over-estimate only ever causes the optional
    /// fast path to (correctly) fall back. Variable-containing facts are rare in the
    /// target workloads (ground-fact KBs, PLN), so a fresh space stays at 0 and gets
    /// the fast path; a space that ever stores one accepts the always-correct iterative
    /// path for its lifetime. Per-fact decrement would re-enable the fast path after
    /// removal but is unnecessary for correctness and is intentionally omitted.
    pub(crate) variable_fact_count: AtomicUsize,

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
    /// changing — i.e., re-inference produces the same type set as the previous
    /// iteration. That stable state is the fixpoint of the type inference function.
    /// State-based cycle detection guarantees termination even if types oscillate.
    pub(crate) fixpoint_generation: AtomicU64,

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

    /// Monotonic epoch for MORK byte-conversion cache invalidation.
    /// Allocated once from `next_mork_epoch()` at construction time.
    /// Propagated unchanged on `fork()` (same symbol mapping, same epoch).
    pub(crate) mork_cache_epoch: u64,

    // ========================================================================
    // LSM-tiered ACT base (Stage 5a "next layer" — see `act_tiered.rs`)
    // ========================================================================
    /// The attached immutable out-of-core ACT base, or `None` when the space is
    /// purely in-memory (no tiering). `Some(_)` makes `match_space` overlay the
    /// in-memory `btm` on top of `(base − tombstones)`. Held behind `Arc<ActBase>`
    /// so `fork`/`make_owned` clone it with a cheap `Arc` bump and compaction can
    /// RCU-swap a fresh base in. See [`ActBase`] for why the `ACTMmap` is NOT cached.
    pub(crate) act_base: RwLock<Option<Arc<ActBase>>>,

    /// Per-base-key SUPPRESSION COUNT over base facts. The stored `Multiplicity(n)`
    /// at a base-`sm`-encoded fact key means "hide `n` copies of this base fact" on
    /// the tiered read (`effective_base_mult = max(0, base_leaf_mult − n)`). Kept
    /// SEPARATE from `btm` because the overlay's `remove_atom` auto-prunes 0-count
    /// entries — a tombstone must persist a positive count to suppress base copies.
    /// Tombstone keys live in the base's `sm` space so they byte-match base keys.
    /// O(1) CoW clone on `fork`/`make_owned`.
    pub(crate) tombstones: RwLock<PathMap<Multiplicity>>,

    /// Fast no-base gate, mirroring `variable_fact_count`'s monotonic-flag idiom: a
    /// single relaxed-load avoids taking the `act_base` lock on the hot read path when
    /// no base is attached. Set `true` on attach/compact-reattach, `false` on detach.
    /// (Unlike `variable_fact_count` this is NOT monotonic — detach clears it — because
    /// it gates a correctness-preserving overlay, and a stale `true` would only cost an
    /// extra empty-base read while a stale `false` after attach is prevented by setting
    /// it under the same critical section as the `act_base` write.)
    pub(crate) has_act_base: AtomicBool,
}

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> AtomSpace<V> {
    /// Create a new empty AtomSpace with the given MORK symbol interning handle
    /// and expected entry count for the Bloom filter.
    pub fn new(shared_mapping: SharedMappingHandle, expected_entries: usize) -> Self {
        AtomSpace {
            btm: RwLock::new(PathMap::new()),
            wide_btm: RwLock::new(PathMap::new()),
            type_btm: RwLock::new(PathMap::new()),
            subtype_btm: RwLock::new(PathMap::new()),
            inferred_type_btm: RwLock::new(PathMap::new()),
            inferred_type_bloom: Arc::new(AtomicBloomFilter::new(expected_entries / 10)),
            shared_mapping,
            head_arity_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(
                expected_entries,
            ))),
            rule_head_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(
                expected_entries / 2,
            ))),
            type_bloom: std::sync::Arc::new(RwLock::new(super::bloom::TypeBloomFilter::new(
                expected_entries / 10,
            ))),
            total_atoms: AtomicUsize::new(0),
            variable_fact_count: AtomicUsize::new(0),
            // Phase 10.5: both start at 0 — no fixpoint needed until types are registered
            inferred_type_generation: AtomicU64::new(0),
            fixpoint_generation: AtomicU64::new(0),
            variable_atoms: RwLock::new(Vec::new()),
            mork_cache_epoch: crate::backend::mork_convert::next_mork_epoch(),
            // Fresh space: no tiering, empty tombstones, base gate off.
            act_base: RwLock::new(None),
            tombstones: RwLock::new(PathMap::new()),
            has_act_base: AtomicBool::new(false),
        }
    }

    /// Fork this AtomSpace for nondeterministic branch isolation.
    ///
    /// PathMap clone is O(1) CoW. Bloom filter is Arc-cloned (shared until mutation).
    /// Variable atoms Vec is cloned (expected to be very small).
    pub fn fork(&self) -> Self {
        AtomSpace {
            btm: RwLock::new(self.btm.read().clone()),
            wide_btm: RwLock::new(self.wide_btm.read().clone()),
            type_btm: RwLock::new(self.type_btm.read().clone()),
            subtype_btm: RwLock::new(self.subtype_btm.read().clone()),
            // Phase 10.1: inferred_type_btm is CoW-cloned (same as type_btm)
            inferred_type_btm: RwLock::new(self.inferred_type_btm.read().clone()),
            // Phase 10.1: bloom is append-only, safe to share via Arc::clone
            inferred_type_bloom: Arc::clone(&self.inferred_type_bloom),
            shared_mapping: self.shared_mapping.clone(),
            head_arity_bloom: std::sync::Arc::clone(&self.head_arity_bloom),
            rule_head_bloom: std::sync::Arc::clone(&self.rule_head_bloom),
            type_bloom: std::sync::Arc::clone(&self.type_bloom),
            total_atoms: AtomicUsize::new(self.total_atoms.load(Ordering::Acquire)),
            variable_fact_count: AtomicUsize::new(self.variable_fact_count.load(Ordering::Acquire)),
            // Phase 10.5: snapshot generation counters into forked AtomSpace
            inferred_type_generation: AtomicU64::new(
                self.inferred_type_generation.load(Ordering::Acquire),
            ),
            fixpoint_generation: AtomicU64::new(self.fixpoint_generation.load(Ordering::Acquire)),
            variable_atoms: RwLock::new(self.variable_atoms.read().clone()),
            // Same symbol mapping → same epoch (cache entries remain valid)
            mork_cache_epoch: self.mork_cache_epoch,
            // Tiering carries to the fork: `Arc<ActBase>` clone is a cheap bump
            // (read-only base sharing), and `tombstones` clones O(1) via PathMap CoW.
            // Both inputs/output observe the same base + suppression state, so a
            // forked branch's tiered reads agree with the parent's until either
            // side mutates (overlay/tombstone writes are CoW-isolated thereafter).
            act_base: RwLock::new(self.act_base.read().clone()),
            tombstones: RwLock::new(self.tombstones.read().clone()),
            has_act_base: AtomicBool::new(self.has_act_base.load(Ordering::Acquire)),
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
    /// `btm` and `wide_btm` store only byte keys + Multiplicity — no V references,
    /// so they require no GC tracing.
    pub fn collect_gc_roots(&self, roots: &mut Vec<V>) {
        // Variable atoms hold V values directly
        let var_atoms = self.variable_atoms.read();
        roots.reserve(var_atoms.len());
        for (val, _mult) in var_atoms.iter() {
            roots.push(val.clone());
        }
    }
}
