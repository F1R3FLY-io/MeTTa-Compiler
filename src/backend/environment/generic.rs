//! Generic Environment for Zero-Conversion Evaluation
//!
//! This module provides generic environment types parameterized over value types,
//! enabling zero-conversion evaluation with both heap and arena allocation strategies.
//!
//! ## Design
//!
//! The key insight is to parameterize the environment over `V: MettaValueTrait` instead
//! of storing serialized bytes. This eliminates all conversions:
//!
//! - `MettaEnvironment = GenericEnvironment<MettaValue>` (O(1) pointer clone)
//!
//! ## Architecture
//!
//! ```ignore
//! GenericEnvironment<V>
//!   └── Arc<GenericEnvironmentShared<V>>
//!         ├── btm: RwLock<PathMap<Multiplicity>>  (rules + facts as MORK bytes)
//!         ├── named_spaces: RwLock<HashMap<u64, (String, Vec<V>)>>
//!         ├── bindings: RwLock<HashMap<String, V>>
//!         └── ... (type-agnostic fields: symbols, states, etc.)
//! ```
//!
//! ## Thread Safety
//!
//! Uses non-blocking concurrent data structures for maximum parallelism:
//! - `parking_lot::RwLock<HashMap>` for concurrent map access (single-threaded workloads)
//! - `parking_lot::RwLock` for structures requiring exclusive access (PathMap, LruCache)
//! - `AtomicBool`/`AtomicUsize` for simple flags and counters
//!
//! Clone operations are O(1) via Arc sharing until first mutation (CoW).

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use lru::LruCache;
use mork_interning::SharedMappingHandle;
use parking_lot::RwLock;
use pathmap::PathMap;
use tracing::trace;

use super::bloom::HeadArityBloomFilter;
use super::multiplicity::Multiplicity;
use super::scope::ScopeTracker;
use crate::backend::fuzzy_match::FuzzyMatcher;
use crate::backend::grounded::{GenericGroundedRegistry, GroundedRegistry};
use crate::backend::models::{
    MettaValue, MettaValueFactory, MettaValueTrait, SpaceHandle,
};
use crate::backend::modules::ModuleRegistry;
use crate::backend::models::GcFactory;

// ============================================================================
// Static Sentinel for Unmodified Environments
// ============================================================================


// ============================================================================
// Helper Functions for Environment Operations
// ============================================================================

/// Merge two PathMaps by taking the maximum multiplicity for each path.
///
/// This is used by environment union to combine facts from two environments.
/// For each path present in either PathMap, the result contains that path with
/// the maximum of the two multiplicities (or the single multiplicity if only in one).
fn merge_pathmaps_max(
    a: &PathMap<Multiplicity>,
    b: &PathMap<Multiplicity>,
) -> PathMap<Multiplicity> {
    use pathmap::zipper::*;

    // Start with a clone of 'a'
    let mut result = a.clone();

    // Iterate through 'b' and take max for each path
    let mut rz = b.read_zipper();
    while rz.to_next_val() {
        let path = rz.path();
        let b_count = rz.val().map(|m| m.count()).unwrap_or(0);

        // Check if path exists in result
        if let Some(a_mult) = result.get(path) {
            // Take max of multiplicities
            let max_count = a_mult.count().max(b_count);
            result.insert(path, Multiplicity::new(max_count));
        } else {
            // Path only in b, add it
            result.insert(path, Multiplicity::new(b_count));
        }
    }

    result
}

// ============================================================================
// Multiplicity Match - Generic for any value type
// ============================================================================

/// Generic multiplicity match that works with any value type.
///
/// Used by `match_space` to return matches with their multiplicities,
/// enabling lazy expansion for high-multiplicity matches.
#[derive(Debug, Clone)]
pub struct MultiplicityMatch<V> {
    /// The matched value
    pub value: V,
    /// The number of times this value appears
    pub count: usize,
}

impl<V: Clone> MultiplicityMatch<V> {
    /// Create a new multiplicity match.
    #[inline]
    pub fn new(value: V, count: usize) -> Self {
        Self { value, count }
    }

    /// Expand into an iterator of cloned values.
    ///
    /// This defers the cloning until the iterator is actually consumed,
    /// enabling lazy evaluation of high-multiplicity matches.
    ///
    /// # Example
    /// ```ignore
    /// let m = MultiplicityMatch::new(atom, 1000);
    /// // Only clones when iterated:
    /// for value in m.expand().take(10) {
    ///     // Only 10 clones happen, not 1000
    /// }
    /// ```
    #[inline]
    pub fn expand(self) -> impl Iterator<Item = V> {
        std::iter::repeat(self.value).take(self.count)
    }

    /// Check if this is a single match (count == 1).
    #[inline]
    pub fn is_single(&self) -> bool {
        self.count == 1
    }
}

/// Shared state across all GenericEnvironment clones.
///
/// Parameterized over `V: MettaValueTrait` to enable zero-conversion evaluation.
/// Values are stored natively in their concrete type (MettaValue or MettaValue).
///
/// ## Thread Safety
///
/// Uses `parking_lot::RwLock<HashMap>` for environment-owned maps (bindings, types,
/// named_spaces, states). These maps are protected by CoW semantics:
/// after `make_owned()`, only a single writer accesses the new HashMap.
///
/// Other structures use:
/// - `parking_lot::RwLock`: For PathMap, LruCache, and other non-HashMap structures
/// - `AtomicU64`/`AtomicBool`/`AtomicUsize`: Lock-free counters and flags
pub struct GenericEnvironmentShared<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> {
    // ========================================================================
    // Type-Agnostic Storage (bytes/MORK)
    // ========================================================================
    /// PathMap trie for fact storage (value = atom multiplicity).
    /// Rules are stored as `(= lhs rhs)` MORK byte keys — no separate index.
    /// Uses parking_lot::RwLock (PathMap has no concurrent alternative)
    pub(crate) btm: RwLock<PathMap<Multiplicity>>,

    /// Mutable state cells registry (stores V directly - no serialization)
    /// Uses RwLock<HashMap> — protected by CoW semantics
    pub(crate) states: RwLock<HashMap<u64, V>>,

    /// Counter for generating unique state IDs (lock-free atomic)
    pub(crate) next_state_id: AtomicU64,

    // ========================================================================
    // Generic Named Spaces (parameterized over V)
    // ========================================================================
    /// Named spaces registry: Maps space_id -> (name, atoms)
    /// Uses RwLock<HashMap> — protected by CoW semantics
    #[allow(clippy::type_complexity)]
    pub(crate) named_spaces: RwLock<HashMap<u64, (String, Vec<V>)>>,

    /// Counter for generating unique space IDs (lock-free atomic)
    pub(crate) next_space_id: AtomicU64,

    // ========================================================================
    // Generic Symbol Bindings (parameterized over V)
    // ========================================================================
    /// Symbol bindings registry: Maps name -> V
    /// Uses RwLock<HashMap> — protected by CoW semantics
    pub(crate) bindings: RwLock<HashMap<String, V>>,

    /// Type assertions: Maps symbol name -> type value V (no serialization)
    /// Uses RwLock<HashMap> — protected by CoW semantics
    pub(crate) types: RwLock<HashMap<String, V>>,

    // ========================================================================
    // Type-Agnostic Registries and Caches
    // ========================================================================
    /// Module registry (type-agnostic)
    /// Uses RwLock (ModuleRegistry has internal state)
    pub(crate) module_registry: RwLock<ModuleRegistry>,

    /// Per-module tokenizer (type-agnostic)
    /// Uses RwLock (Tokenizer has internal state)
    pub(crate) tokenizer: RwLock<crate::backend::modules::GenericTokenizer<V>>,

    /// Grounded operations registry (legacy, used by proptests only)
    /// Uses RwLock (rarely modified after init)
    pub(crate) grounded_registry: RwLock<GroundedRegistry>,

    /// Generic grounded operations registry (type-parameterized, zero-conversion)
    /// Stateless and Clone, no lock needed
    pub(crate) generic_grounded_registry: GenericGroundedRegistry,

    /// Pattern cache for MORK serialization (keyed by MettaValue for heap mode)
    /// Uses RwLock (LruCache requires exclusive access for get/put)
    pub(crate) pattern_cache: RwLock<LruCache<MettaValue, Vec<u8>>>,

    /// Type index: Lazy-initialized subtrie containing only type assertions
    /// Uses RwLock (PathMap has no concurrent alternative)
    pub(crate) type_index: RwLock<Option<PathMap<Multiplicity>>>,

    /// Type index invalidation flag (lock-free atomic)
    pub(crate) type_index_dirty: AtomicBool,

    /// Fallback store for large expressions (arity >= 64)
    /// Uses RwLock (PathMap has no concurrent alternative)
    /// Stores V directly (zero-conversion)
    pub(crate) large_expr_pathmap: RwLock<Option<PathMap<V>>>,

    /// Fuzzy matcher for "Did you mean?" suggestions
    /// Uses RwLock (FuzzyMatcher has internal state)
    pub(crate) fuzzy_matcher: RwLock<FuzzyMatcher>,

    /// Hierarchical scope tracker for context-aware symbol resolution
    /// Uses RwLock (ScopeTracker has internal state)
    pub(crate) scope_tracker: RwLock<ScopeTracker>,

    /// Bloom filter for (head_symbol, arity) pairs - enables O(1) match_space() rejection
    /// Uses RwLock (HeadArityBloomFilter has internal state)
    pub(crate) head_arity_bloom: RwLock<HeadArityBloomFilter>,

    /// O(1) total atom count (sum of all multiplicities)
    pub(crate) total_atoms: AtomicUsize,

}

/// Generic environment parameterized over value type and factory.
///
/// This is the main entry point for zero-conversion evaluation. Use type aliases
/// for convenience:
///
/// - `MettaEnvironment` = `GenericEnvironment<MettaValue, GcFactory>`
///
/// ## Copy-on-Write (CoW) Semantics
///
/// Clones share data via Arc until first modification:
/// - `owns_data = false`: Clone is sharing, must call `make_owned()` before mutation
/// - `owns_data = true`: Clone owns its data, can mutate in-place
///
/// ## Performance
///
/// - Clone: O(1) - single Arc increment
/// - First mutation after clone: O(n) deep copy via `make_owned()`
/// - Subsequent mutations: O(1) in-place
pub struct GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Consolidated shared state - single Arc for O(1) cloning
    pub(crate) shared: Arc<GenericEnvironmentShared<V>>,

    /// Factory for creating V values (used by match_space, etc.)
    pub(crate) factory: F,

    /// SharedMappingHandle for MORK symbol interning
    pub(crate) shared_mapping: SharedMappingHandle,

    /// CoW: Tracks if this clone owns its data
    pub(crate) owns_data: bool,

    /// CoW: Tracks if this environment has been modified
    pub(crate) modified: AtomicBool,

    /// Current module path for relative path resolution
    pub(crate) current_module_path: Option<PathBuf>,
}

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Create a new generic environment.
    ///
    /// # Parameters
    ///
    /// The factory is stored and used for creating V values during operations
    /// like `match_space` that need to construct values from MORK bytes.
    pub fn new(factory: F) -> Self {
        use mork_interning::SharedMapping;

        // Create the shared mapping for MORK symbol interning.
        let shared_mapping = SharedMapping::new();

        // Warm up SharedMapping to pre-initialize all 128 internal PathMap
        // buckets (to_symbol). PathMap::ensure_root() uses UnsafeCell without
        // synchronization — a TOCTOU race where concurrent threads both enter
        // do_init_root() causes data races on the trie root. Pre-inserting one
        // byte per bucket (0..128 = MAX_WRITER_THREADS) forces eager root
        // allocation while we have exclusive write permission, eliminating the
        // race window before any multi-threaded access occurs.
        if let Ok(permit) = shared_mapping.try_aquire_permission() {
            for i in 0..128u8 {
                let _ = permit.get_sym_or_insert(&[i]);
            }
        }

        let shared = Arc::new(GenericEnvironmentShared {
            // Type-agnostic storage
            btm: RwLock::new(PathMap::new()),
            states: RwLock::new(HashMap::new()),
            next_state_id: AtomicU64::new(1),

            // Generic named spaces
            named_spaces: RwLock::new(HashMap::new()),
            next_space_id: AtomicU64::new(1),

            // Generic symbol bindings
            bindings: RwLock::new(HashMap::new()),

            // Type assertions storage
            types: RwLock::new(HashMap::new()),

            // Type-agnostic registries
            module_registry: RwLock::new(ModuleRegistry::new()),
            tokenizer: RwLock::new(crate::backend::modules::GenericTokenizer::<V>::new()),
            grounded_registry: RwLock::new(GroundedRegistry::new()),
            generic_grounded_registry: GenericGroundedRegistry::with_standard_ops(),
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(None),
            type_index_dirty: AtomicBool::new(true),
            large_expr_pathmap: RwLock::new(None),
            fuzzy_matcher: RwLock::new(FuzzyMatcher::new()),
            scope_tracker: RwLock::new(ScopeTracker::new()),
            head_arity_bloom: RwLock::new(HeadArityBloomFilter::new(10000)),
            total_atoms: AtomicUsize::new(0),
        });

        // Register as GC root provider (no-op if V != MettaValue)
        crate::backend::models::gc_allocator::try_register_env_roots(&shared);

        GenericEnvironment {
            shared,
            factory,
            shared_mapping,
            owns_data: true,
            modified: AtomicBool::new(false),
            current_module_path: None,
        }
    }

    /// Get the factory for creating V values.
    #[inline]
    pub fn factory(&self) -> &F {
        &self.factory
    }

    /// Mark this environment as modified.
    #[inline]
    fn mark_modified(&self) {
        self.modified.store(true, Ordering::Release);
    }

    /// CoW: Make this environment own its data (deep copy if sharing).
    ///
    /// Called automatically on first mutation of a cloned environment.
    /// No-op if already owns data (owns_data == true).
    pub(crate) fn make_owned(&mut self) {
        if self.owns_data {
            return;
        }
        trace!(target: "mettatron::generic_environment::make_owned", "Deep copying CoW data");

        let new_shared = Arc::new(GenericEnvironmentShared {
            // Type-agnostic storage - deep copy (parking_lot::RwLock doesn't use Result)
            btm: RwLock::new(self.shared.btm.read().clone()),
            // RwLock<HashMap> - read lock + clone
            states: RwLock::new(self.shared.states.read().clone()),
            // Atomic - load and create new
            next_state_id: AtomicU64::new(self.shared.next_state_id.load(Ordering::Acquire)),

            // Generic named spaces - RwLock<HashMap>
            named_spaces: RwLock::new(self.shared.named_spaces.read().clone()),
            next_space_id: AtomicU64::new(self.shared.next_space_id.load(Ordering::Acquire)),

            // Generic symbol bindings - RwLock<HashMap>
            bindings: RwLock::new(self.shared.bindings.read().clone()),

            // Type assertions - RwLock<HashMap>
            types: RwLock::new(self.shared.types.read().clone()),

            // Type-agnostic registries - parking_lot::RwLock (no .expect())
            module_registry: RwLock::new(self.shared.module_registry.read().clone()),
            tokenizer: RwLock::new(self.shared.tokenizer.read().clone()),
            grounded_registry: RwLock::new(self.shared.grounded_registry.read().clone()),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),
            pattern_cache: RwLock::new(self.shared.pattern_cache.read().clone()),
            type_index: RwLock::new(self.shared.type_index.read().clone()),
            type_index_dirty: AtomicBool::new(
                self.shared.type_index_dirty.load(Ordering::Acquire),
            ),
            large_expr_pathmap: RwLock::new(self.shared.large_expr_pathmap.read().clone()),
            fuzzy_matcher: RwLock::new(self.shared.fuzzy_matcher.read().clone()),
            scope_tracker: RwLock::new(self.shared.scope_tracker.read().clone()),
            head_arity_bloom: RwLock::new(self.shared.head_arity_bloom.read().clone()),
            total_atoms: AtomicUsize::new(self.shared.total_atoms.load(Ordering::Acquire)),
        });

        // Register new shared state as GC root provider
        crate::backend::models::gc_allocator::try_register_env_roots(&new_shared);

        self.shared = new_shared;
        self.owns_data = true;
        self.mark_modified();
    }

    /// Create a forked environment for nondeterministic branch isolation.
    ///
    /// Uses O(1) PathMap CoW clone for fork isolation.
    pub fn fork_for_nondeterminism(&self) -> Self {
        trace!(target: "mettatron::generic_environment::fork", "Forking environment for nondeterminism");

        let new_shared = Arc::new(GenericEnvironmentShared {
            // Type-agnostic storage (parking_lot::RwLock - no .expect())
            btm: RwLock::new(self.shared.btm.read().clone()),
            states: RwLock::new(self.shared.states.read().clone()),
            next_state_id: AtomicU64::new(self.shared.next_state_id.load(Ordering::Acquire)),

            // Generic named spaces - RwLock<HashMap>
            named_spaces: RwLock::new(self.shared.named_spaces.read().clone()),
            next_space_id: AtomicU64::new(self.shared.next_space_id.load(Ordering::Acquire)),

            // Generic symbol bindings - RwLock<HashMap>
            bindings: RwLock::new(self.shared.bindings.read().clone()),

            // Type assertions - RwLock<HashMap>
            types: RwLock::new(self.shared.types.read().clone()),

            // Type-agnostic registries (parking_lot::RwLock - no .expect())
            module_registry: RwLock::new(self.shared.module_registry.read().clone()),
            tokenizer: RwLock::new(self.shared.tokenizer.read().clone()),
            grounded_registry: RwLock::new(self.shared.grounded_registry.read().clone()),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),
            // Clear pattern cache instead of copying
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(self.shared.type_index.read().clone()),
            type_index_dirty: AtomicBool::new(
                self.shared.type_index_dirty.load(Ordering::Acquire),
            ),
            large_expr_pathmap: RwLock::new(self.shared.large_expr_pathmap.read().clone()),
            fuzzy_matcher: RwLock::new(self.shared.fuzzy_matcher.read().clone()),
            scope_tracker: RwLock::new(self.shared.scope_tracker.read().clone()),
            head_arity_bloom: RwLock::new(self.shared.head_arity_bloom.read().clone()),
            total_atoms: AtomicUsize::new(self.shared.total_atoms.load(Ordering::Acquire)),
        });

        // Register forked shared state as GC root provider
        crate::backend::models::gc_allocator::try_register_env_roots(&new_shared);

        GenericEnvironment {
            shared: new_shared,
            factory: self.factory.clone(),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: true,
            modified: AtomicBool::new(false),
            current_module_path: self.current_module_path.clone(),
        }
    }

    /// Union two environments (monotonic merge).
    ///
    /// This implements proper environment union semantics by merging state from
    /// both environments:
    ///
    /// 1. **Fast path**: If both share the same underlying Arc, return a shared clone.
    /// 2. **Fast path**: If neither was modified, share self's state.
    /// 3. **Merge path**: Actually merge state from both environments:
    ///    - PathMap facts: take max multiplicity for each path
    ///    - Rules: combine and deduplicate by structural equality
    ///    - Bindings/Types: combine (other's values take precedence on conflict)
    ///    - States: combine by ID (other's values take precedence)
    ///    - Named spaces: combine by ID
    ///
    /// This is used by the Rholang language server for combining environment
    /// state after parallel or alternative evaluations.
    pub fn union(&self, other: &Self) -> Self {
        trace!(target: "mettatron::generic_environment::union", "Unioning environments");

        // Fast path: same underlying data
        if Arc::ptr_eq(&self.shared, &other.shared) {
            return GenericEnvironment {
                shared: Arc::clone(&self.shared),
                factory: self.factory.clone(),
                shared_mapping: self.shared_mapping.clone(),
                owns_data: false,
                modified: AtomicBool::new(false),
                current_module_path: self.current_module_path.clone(),
            };
        }

        let self_modified = self.owns_data && self.modified.load(Ordering::Acquire);
        let other_modified = other.owns_data && other.modified.load(Ordering::Acquire);

        // Fast path: neither modified, share self's state
        if !self_modified && !other_modified {
            return GenericEnvironment {
                shared: Arc::clone(&self.shared),
                factory: self.factory.clone(),
                shared_mapping: self.shared_mapping.clone(),
                owns_data: false,
                modified: AtomicBool::new(false),
                current_module_path: self.current_module_path.clone(),
            };
        }

        // Fast path: only self modified, use self's state
        if self_modified && !other_modified {
            return GenericEnvironment {
                shared: Arc::clone(&self.shared),
                factory: self.factory.clone(),
                shared_mapping: self.shared_mapping.clone(),
                owns_data: false,
                modified: AtomicBool::new(false),
                current_module_path: self.current_module_path.clone(),
            };
        }

        // Fast path: only other modified, use other's state
        if !self_modified && other_modified {
            return GenericEnvironment {
                shared: Arc::clone(&other.shared),
                factory: self.factory.clone(),
                shared_mapping: other.shared_mapping.clone(),
                owns_data: false,
                modified: AtomicBool::new(false),
                current_module_path: other.current_module_path.clone(),
            };
        }

        // Both modified: perform actual merge
        trace!(target: "mettatron::generic_environment::union", "Both environments modified, performing merge");

        // Merge PathMaps by taking max multiplicity
        let merged_btm = {
            let self_btm = self.shared.btm.read();
            let other_btm = other.shared.btm.read();
            merge_pathmaps_max(&self_btm, &other_btm)
        };

        // Calculate total atoms from merged PathMap
        let merged_total_atoms = {
            use pathmap::zipper::*;
            let mut rz = merged_btm.read_zipper();
            let mut total = 0usize;
            while rz.to_next_val() {
                if let Some(mult) = rz.val() {
                    total += mult.count() as usize;
                }
            }
            total
        };

        // Rules are stored as (= lhs rhs) MORK bytes in PathMap — merged via merge_pathmaps_max above.

        // Merge bindings (other takes precedence)
        let merged_bindings: HashMap<String, V> = {
            let mut merged = self.shared.bindings.read().clone();
            for (k, v) in other.shared.bindings.read().iter() {
                merged.insert(k.clone(), v.clone());
            }
            merged
        };

        // Merge types (other takes precedence)
        let merged_types: HashMap<String, V> = {
            let mut merged = self.shared.types.read().clone();
            for (k, v) in other.shared.types.read().iter() {
                merged.insert(k.clone(), v.clone());
            }
            merged
        };

        // Merge states (other takes precedence)
        let merged_states: HashMap<u64, V> = {
            let mut merged = self.shared.states.read().clone();
            for (k, v) in other.shared.states.read().iter() {
                merged.insert(*k, v.clone());
            }
            merged
        };

        // Merge named spaces (combine atoms within same space)
        let merged_named_spaces: HashMap<u64, (String, Vec<V>)> = {
            let mut merged = self.shared.named_spaces.read().clone();
            for (id, (name, atoms)) in other.shared.named_spaces.read().iter() {
                merged
                    .entry(*id)
                    .and_modify(|(_, existing_atoms)| {
                        existing_atoms.extend(atoms.iter().cloned());
                    })
                    .or_insert_with(|| (name.clone(), atoms.clone()));
            }
            merged
        };

        // Take max of ID counters to avoid collisions
        let max_state_id = self.shared.next_state_id.load(Ordering::Acquire)
            .max(other.shared.next_state_id.load(Ordering::Acquire));
        let max_space_id = self.shared.next_space_id.load(Ordering::Acquire)
            .max(other.shared.next_space_id.load(Ordering::Acquire));

        // Merge fuzzy matchers by cloning self and inserting other's terms
        let merged_fuzzy = {
            let self_fuzzy = self.shared.fuzzy_matcher.read();
            let other_fuzzy = other.shared.fuzzy_matcher.read();
            // Clone self's fuzzy matcher (gets a fresh dictionary OnceLock)
            let merged = self_fuzzy.clone();
            // Insert all terms from other's pending set
            for term in other_fuzzy.pending_iter() {
                merged.insert(&term);
            }
            merged
        };

        // Create new shared state with merged data
        let new_shared = Arc::new(GenericEnvironmentShared {
            btm: RwLock::new(merged_btm),
            states: RwLock::new(merged_states),
            next_state_id: AtomicU64::new(max_state_id),

            named_spaces: RwLock::new(merged_named_spaces),
            next_space_id: AtomicU64::new(max_space_id),

            bindings: RwLock::new(merged_bindings),
            types: RwLock::new(merged_types),

            // Share from self (these are typically static after initialization)
            module_registry: RwLock::new(self.shared.module_registry.read().clone()),
            tokenizer: RwLock::new(self.shared.tokenizer.read().clone()),
            grounded_registry: RwLock::new(self.shared.grounded_registry.read().clone()),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),

            // Clear/reset caches after merge
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(None), // Invalidate
            type_index_dirty: AtomicBool::new(true),
            large_expr_pathmap: RwLock::new(None), // TODO: merge these too

            fuzzy_matcher: RwLock::new(merged_fuzzy),
            scope_tracker: RwLock::new(other.shared.scope_tracker.read().clone()), // Use other's scope
            head_arity_bloom: RwLock::new(HeadArityBloomFilter::new(10000)), // Reset (will be rebuilt)
            total_atoms: AtomicUsize::new(merged_total_atoms),
        });

        // Register merged shared state as GC root provider
        crate::backend::models::gc_allocator::try_register_env_roots(&new_shared);

        GenericEnvironment {
            shared: new_shared,
            factory: self.factory.clone(),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: true,
            modified: AtomicBool::new(true),
            current_module_path: other.current_module_path.clone().or_else(|| self.current_module_path.clone()),
        }
    }

    /// Union multiple environments in a single pass.
    ///
    /// This is an optimized batch version of `union()` that handles the common
    /// case of unioning many child environments after parallel/nondeterministic
    /// evaluation. Instead of N sequential `union()` calls with N allocations,
    /// this method:
    ///
    /// 1. **Early exit**: If no environment was modified, returns a shared clone
    ///    of self with zero allocations (the common case for pure evaluation).
    ///
    /// 2. **Single-modified fast path**: If only one environment was modified,
    ///    returns a shared clone of that environment (one Arc clone, zero allocations).
    ///
    /// 3. **Batch merge**: For multiple modified environments, performs a single
    ///    merged union instead of N binary merges.
    ///
    /// # Performance
    ///
    /// For N child environments:
    /// - Common case (no modifications): O(N) checks, 0 allocations
    /// - Single modification: O(N) checks, 1 Arc clone
    /// - Multiple modifications: O(N) checks + single batch merge
    ///
    /// Compare to the naive loop pattern:
    /// ```ignore
    /// let mut unified = original;
    /// for e in envs {
    ///     unified = unified.union(&e); // N allocations even when nothing modified!
    /// }
    /// ```
    pub fn union_all<'a, I>(&self, others: I) -> Self
    where
        I: IntoIterator<Item = &'a Self>,
        Self: 'a,
    {
        trace!(target: "mettatron::generic_environment::union_all", "Batch unioning environments");

        let others: Vec<&Self> = others.into_iter().collect();

        // Fast path: empty iterator - return shared clone
        if others.is_empty() {
            return self.shared_clone();
        }

        // Collect modification status
        let self_modified = self.owns_data && self.is_modified();
        let modified_others: Vec<&Self> = others
            .iter()
            .filter(|e| e.owns_data && e.is_modified())
            .copied()
            .collect();

        match (self_modified, modified_others.len()) {
            // No modifications anywhere - return shared clone of self
            (false, 0) => {
                trace!(target: "mettatron::generic_environment::union_all", "Fast path: no modifications");
                self.shared_clone()
            }

            // Only self modified - return shared clone of self
            (true, 0) => {
                trace!(target: "mettatron::generic_environment::union_all", "Fast path: only self modified");
                self.shared_clone()
            }

            // Only one other modified - return shared clone of that
            (false, 1) => {
                trace!(target: "mettatron::generic_environment::union_all", "Fast path: single other modified");
                modified_others[0].shared_clone()
            }

            // Self + one other both modified - delegate to binary union
            (true, 1) => {
                trace!(target: "mettatron::generic_environment::union_all", "Two modified: delegating to binary union");
                self.union(modified_others[0])
            }

            // Multiple modified - batch merge
            (self_mod, n) if n > 1 || (self_mod && n == 1) => {
                trace!(target: "mettatron::generic_environment::union_all", "Batch merge: {} environments", n + if self_mod { 1 } else { 0 });
                self.merge_all_modified(&modified_others, self_modified)
            }

            // Catch-all (shouldn't be reached, but be defensive)
            _ => self.shared_clone(),
        }
    }

    /// Create a shared clone with unmodified flag.
    ///
    /// This is an internal helper that creates a clone sharing the same Arc data
    /// with `owns_data = false` and a fresh `modified = false` flag.
    #[inline]
    fn shared_clone(&self) -> Self {
        GenericEnvironment {
            shared: Arc::clone(&self.shared),
            factory: self.factory.clone(),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false,
            modified: AtomicBool::new(false),
            current_module_path: self.current_module_path.clone(),
        }
    }

    /// Merge multiple modified environments in a single pass.
    ///
    /// This is the batch version of the merge logic in `union()`, optimized for
    /// when we know we have multiple modified environments to combine.
    fn merge_all_modified(&self, others: &[&Self], include_self: bool) -> Self {
        trace!(target: "mettatron::generic_environment::merge_all_modified",
               "Merging {} environments (include_self={})", others.len(), include_self);

        // Start with self's state or first other's state as base
        let base = if include_self { self } else { others[0] };
        let merge_start_idx = if include_self { 0 } else { 1 };

        // Merge PathMaps by taking max multiplicity
        let merged_btm = {
            let mut result = base.shared.btm.read().clone();
            for other in &others[merge_start_idx..] {
                let other_btm = other.shared.btm.read();
                result = merge_pathmaps_max(&result, &other_btm);
            }
            // If we started from self and include_self is true, we already have self's data
            // Otherwise merge self's data too
            if !include_self {
                let self_btm = self.shared.btm.read();
                result = merge_pathmaps_max(&result, &self_btm);
            }
            result
        };

        // Calculate total atoms from merged PathMap
        let merged_total_atoms = {
            use pathmap::zipper::*;
            let mut rz = merged_btm.read_zipper();
            let mut total = 0usize;
            while rz.to_next_val() {
                if let Some(mult) = rz.val() {
                    total += mult.count() as usize;
                }
            }
            total
        };

        // Rules are stored as (= lhs rhs) MORK bytes in PathMap — merged via merge_pathmaps_max above.

        // Merge bindings (later environments take precedence)
        let merged_bindings: HashMap<String, V> = {
            let mut base_bindings = base.shared.bindings.read().clone();
            for other in &others[merge_start_idx..] {
                for (k, v) in other.shared.bindings.read().iter() {
                    base_bindings.insert(k.clone(), v.clone());
                }
            }
            base_bindings
        };

        // Merge types (later environments take precedence)
        let merged_types: HashMap<String, V> = {
            let mut base_types = base.shared.types.read().clone();
            for other in &others[merge_start_idx..] {
                for (k, v) in other.shared.types.read().iter() {
                    base_types.insert(k.clone(), v.clone());
                }
            }
            base_types
        };

        // Merge states (later environments take precedence)
        let merged_states: HashMap<u64, V> = {
            let mut base_states = base.shared.states.read().clone();
            for other in &others[merge_start_idx..] {
                for (k, v) in other.shared.states.read().iter() {
                    base_states.insert(*k, v.clone());
                }
            }
            base_states
        };

        // Merge named spaces
        let merged_named_spaces: HashMap<u64, (String, Vec<V>)> = {
            let mut base_spaces = base.shared.named_spaces.read().clone();
            for other in &others[merge_start_idx..] {
                for (id, (name, atoms)) in other.shared.named_spaces.read().iter() {
                    base_spaces
                        .entry(*id)
                        .and_modify(|(_, existing_atoms)| {
                            existing_atoms.extend(atoms.iter().cloned());
                        })
                        .or_insert_with(|| (name.clone(), atoms.clone()));
                }
            }
            base_spaces
        };

        // Take max of ID counters
        let mut max_state_id = base.shared.next_state_id.load(Ordering::Acquire);
        let mut max_space_id = base.shared.next_space_id.load(Ordering::Acquire);
        for other in &others[merge_start_idx..] {
            max_state_id = max_state_id.max(other.shared.next_state_id.load(Ordering::Acquire));
            max_space_id = max_space_id.max(other.shared.next_space_id.load(Ordering::Acquire));
        }
        if !include_self {
            max_state_id = max_state_id.max(self.shared.next_state_id.load(Ordering::Acquire));
            max_space_id = max_space_id.max(self.shared.next_space_id.load(Ordering::Acquire));
        }

        // Merge fuzzy matchers
        let merged_fuzzy = {
            let base_fuzzy = base.shared.fuzzy_matcher.read();
            let merged = base_fuzzy.clone();
            for other in &others[merge_start_idx..] {
                let other_fuzzy = other.shared.fuzzy_matcher.read();
                for term in other_fuzzy.pending_iter() {
                    merged.insert(&term);
                }
            }
            merged
        };

        // Get the last environment for scope tracker (later takes precedence)
        let last_env = others.last().unwrap_or(&self);

        // Create new shared state with merged data
        let new_shared = Arc::new(GenericEnvironmentShared {
            btm: RwLock::new(merged_btm),
            states: RwLock::new(merged_states),
            next_state_id: AtomicU64::new(max_state_id),

            named_spaces: RwLock::new(merged_named_spaces),
            next_space_id: AtomicU64::new(max_space_id),

            bindings: RwLock::new(merged_bindings),
            types: RwLock::new(merged_types),

            // Share from self (typically static after init)
            module_registry: RwLock::new(self.shared.module_registry.read().clone()),
            tokenizer: RwLock::new(self.shared.tokenizer.read().clone()),
            grounded_registry: RwLock::new(self.shared.grounded_registry.read().clone()),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),

            // Clear/reset caches after merge
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(None), // Invalidate
            type_index_dirty: AtomicBool::new(true),
            large_expr_pathmap: RwLock::new(None), // TODO: merge these too

            fuzzy_matcher: RwLock::new(merged_fuzzy),
            scope_tracker: RwLock::new(last_env.shared.scope_tracker.read().clone()),
            head_arity_bloom: RwLock::new(HeadArityBloomFilter::new(10000)), // Reset (will be rebuilt)
            total_atoms: AtomicUsize::new(merged_total_atoms),
        });

        // Register batch-merged shared state as GC root provider
        crate::backend::models::gc_allocator::try_register_env_roots(&new_shared);

        GenericEnvironment {
            shared: new_shared,
            factory: self.factory.clone(),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: true,
            modified: AtomicBool::new(true),
            current_module_path: last_env.current_module_path.clone().or_else(|| self.current_module_path.clone()),
        }
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    /// Get the generic grounded registry.
    pub fn generic_grounded_registry(&self) -> &GenericGroundedRegistry {
        &self.shared.generic_grounded_registry
    }

    /// Check if the environment has been modified.
    pub fn is_modified(&self) -> bool {
        self.modified.load(Ordering::Acquire)
    }

    /// Get the current module path.
    pub fn current_module_path(&self) -> Option<&PathBuf> {
        self.current_module_path.as_ref()
    }

    // Note: set_current_module_path is defined in module_ops.rs

    /// Collect all GC root values from this environment.
    ///
    /// Traverses all structures that hold `V` values:
    /// - `named_spaces`: All atoms in named spaces
    /// - `bindings`: All symbol binding values
    /// - `types`: All type assertion values
    /// - `states`: All mutable state cell values
    ///
    /// Note: Rules are stored as MORK bytes in `btm` (PathMap), not as `V` values.
    /// They hold no slab pointers and thus are NOT GC roots.
    /// `large_expr_pathmap` stores `V` and IS collected (handled by RootProvider impl).
    pub fn gc_roots(&self, roots: &mut Vec<V>) {
        // Named spaces: collect all atoms
        {
            let named_spaces = self.shared.named_spaces.read();
            for (_id, (_name, atoms)) in named_spaces.iter() {
                roots.extend(atoms.iter().cloned());
            }
        }

        // Symbol bindings
        {
            let bindings = self.shared.bindings.read();
            roots.extend(bindings.values().cloned());
        }

        // Type assertions
        {
            let types = self.shared.types.read();
            roots.extend(types.values().cloned());
        }

        // Mutable state cells
        {
            let states = self.shared.states.read();
            roots.extend(states.values().cloned());
        }
    }
}

impl<V, F> Clone for GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    fn clone(&self) -> Self {
        GenericEnvironment {
            shared: Arc::clone(&self.shared),
            factory: self.factory.clone(),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false, // CoW: clones do not own data initially
            modified: AtomicBool::new(false),            current_module_path: self.current_module_path.clone(),
        }
    }
}

impl<V, F> std::fmt::Debug for GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericEnvironment")
            .field("owns_data", &self.owns_data)
            .field("modified", &self.modified.load(Ordering::Relaxed))
            .finish()
    }
}

// ============================================================================
// RootProvider — GC Root Collection for Arena Environments
// ============================================================================

use crate::backend::models::gc_allocator::RootProvider;

impl RootProvider for GenericEnvironmentShared<MettaValue> {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        // Named spaces: collect all atoms
        {
            let named_spaces = self.named_spaces.read();
            for (_id, (_name, atoms)) in named_spaces.iter() {
                roots.extend(atoms.iter().copied());
            }
        }

        // Symbol bindings
        {
            let bindings = self.bindings.read();
            roots.extend(bindings.values().copied());
        }

        // Type assertions
        {
            let types = self.types.read();
            roots.extend(types.values().copied());
        }

        // Mutable state cells
        {
            let states = self.states.read();
            roots.extend(states.values().copied());
        }

        // Pattern cache keys: LruCache<MettaValue, Vec<u8>>
        // The keys are MettaValues whose inner pointers reference slab slots.
        // Without collecting these, GC could free slots still referenced by
        // cached keys, causing use-after-free on next cache lookup (Hash/Eq).
        {
            let cache = self.pattern_cache.read();
            for (key, _) in cache.iter() {
                roots.push(*key);
            }
        }

        // Large expression PathMap values: PathMap<MettaValue>
        // Stores MettaValues for expressions with arity >= 64. These values
        // are slab-allocated and must be kept alive by the GC.
        {
            let large_pm = self.large_expr_pathmap.read();
            if let Some(ref pm) = *large_pm {
                for (_key, val) in pm.iter() {
                    roots.push(*val);
                }
            }
        }

        // Tokenizer values: bind! stores MettaValues inside closures.
        // Without collecting these, GC frees Space handles (e.g., &kb, &stack)
        // and State values (e.g., &sp) that are still looked up via token resolution.
        {
            let tokenizer = self.tokenizer.read();
            roots.extend(tokenizer.collect_gc_values());
        }
    }

}






// ============================================================================
// MORK Space Access Methods
// ============================================================================

use mork::space::Space;

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Create a thread-local Space for operations.
    /// Following the Rholang LSP pattern: cheap clone via structural sharing.
    pub fn create_space(&self) -> Space<Multiplicity> {
        let btm = self.shared.btm.read().clone();
        Space {
            btm,
            sm: self.shared_mapping.clone(),
            mmaps: std::collections::HashMap::new(),
        }
    }

    /// Update PathMap and shared mapping after Space modifications (write operations).
    /// This updates both the PathMap (btm) and the SharedMappingHandle (sm).
    pub(crate) fn update_pathmap(&mut self, space: Space<Multiplicity>) {
        self.make_owned(); // CoW: ensure we own data before modifying
        *self.shared.btm.write() = space.btm;
        self.shared_mapping = space.sm;
        self.mark_modified(); // CoW: mark as modified
    }

    /// Get the total atom count (O(1)).
    pub fn total_atoms(&self) -> usize {
        self.shared.total_atoms.load(Ordering::Acquire)
    }

    /// Get the "self" space handle.
    ///
    /// Returns a SpaceHandle for the current module's space.
    pub fn self_space(&self) -> SpaceHandle {
        // Use ID 0 for the default "self" space
        SpaceHandle::new(0, "self".to_string())
    }

    /// Register a token with a value in the tokenizer.
    pub fn register_token(&mut self, token: &str, value: V) {
        self.make_owned();
        self.shared.tokenizer.write().register_token_value(token, value);
        self.shared.fuzzy_matcher.write().insert(token);
        self.mark_modified();
    }
}

// ============================================================================
// Generic Space Operations via MORK - Zero-Conversion Architecture
// ============================================================================
//
// These methods use the generic MORK conversion functions that work directly
// with any V: MettaValueTrait, avoiding intermediate MettaValue conversions.
//
// Data flow:
//   V → value_to_mork_bytes_generic() → MORK bytes → PathMap storage
//   PathMap → MORK bytes → mork_bytes_to_generic_value() → V
//
// Pattern matching and binding application also use generic versions:
//   pattern_match_generic() - works directly on V
//   apply_bindings_generic() - works directly on V
// ============================================================================

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Add a fact to the MORK Space for pattern matching.
    ///
    /// ## Zero-Conversion Architecture
    ///
    /// Uses `value_to_mork_bytes_generic()` which operates directly on V via
    /// `MettaValueTrait` methods. No intermediate `MettaValue` conversion occurs.
    ///
    /// ## Multiplicity Tracking
    ///
    /// Uses MeTTa HE semantics: each `add_to_space` call increments the atom's multiplicity.
    pub fn add_to_space(&mut self, value: &V) {
        use crate::backend::mork_convert::with_mork_bytes;
        use crate::backend::varint_encoding::value_to_varint_key_generic;
        use super::multiplicity::add_atom;

        self.make_owned();

        // Direct V → MORK bytes conversion via zero-copy callback
        match with_mork_bytes(value, &self.shared_mapping, |mork_bytes| {
            let mut btm = self.shared.btm.write();
            add_atom(&mut btm, mork_bytes);
            drop(btm);

            self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);

            // Use trait method for head symbol extraction
            if let Some(head) = value.get_head_symbol() {
                let arity = value.get_arity() as u8;
                self.shared.head_arity_bloom.write().insert(head.as_bytes(), arity);
            }
        }) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64)
                // Store V directly (zero-conversion)
                let key = value_to_varint_key_generic(value);

                // Lock ordering: btm before large_expr_pathmap (consistent with remove_from_space)
                {
                    let mut btm = self.shared.btm.write();
                    add_atom(&mut btm, &key);
                }

                {
                    let mut guard = self.shared.large_expr_pathmap.write();
                    let fallback = guard.get_or_insert_with(PathMap::new);
                    fallback.insert(&key, value.clone());
                }

                self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Remove a fact from MORK Space by exact match.
    ///
    /// ## Zero-Conversion Architecture
    ///
    /// Uses `value_to_mork_bytes_generic()` which operates directly on V via
    /// `MettaValueTrait` methods. No intermediate `MettaValue` conversion occurs.
    ///
    /// ## Multiplicity Tracking
    ///
    /// Decrements the atom's multiplicity. If multiplicity reaches 0, the atom is removed.
    pub fn remove_from_space(&mut self, value: &V) {
        use crate::backend::mork_convert::with_mork_bytes;
        use crate::backend::varint_encoding::value_to_varint_key_generic;
        use super::multiplicity::{get_multiplicity, remove_atom};

        self.make_owned();

        // Direct V → MORK bytes conversion via zero-copy callback
        match with_mork_bytes(value, &self.shared_mapping, |mork_bytes| {
            let mut btm = self.shared.btm.write();

            let current_count = get_multiplicity(&btm, mork_bytes);
            if current_count == 0 {
                if !btm.contains(mork_bytes) {
                    return;
                }
                btm.remove(mork_bytes);
                drop(btm);
                self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
                self.shared.head_arity_bloom.write().note_deletion();
                return;
            }

            let new_count = remove_atom(&mut btm, mork_bytes);

            if new_count == 0 {
                self.shared.head_arity_bloom.write().note_deletion();
            }

            drop(btm);
            self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
        }) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64)
                let key = value_to_varint_key_generic(value);

                {
                    let mut btm = self.shared.btm.write();
                    remove_atom(&mut btm, &key);
                }

                let mut guard = self.shared.large_expr_pathmap.write();
                if let Some(ref mut fallback) = *guard {
                    if fallback.contains(&key) {
                        fallback.remove(&key);
                        self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    // ========================================================================
    // Interior Mutability Space Operations (for CoW-safe shared access)
    // ========================================================================
    //
    // These methods use interior mutability via RwLock to mutate shared
    // state WITHOUT triggering the CoW deep copy in make_owned(). This is essential
    // for arena mode correctness where environments are cloned but should share
    // the underlying space state.

    /// Ensure this environment owns its data (CoW helper).
    ///
    /// Call this once before a batch of mutations when using the `_shared` methods.
    /// This triggers a deep copy if needed, then subsequent `_shared` operations
    /// can mutate the owned data efficiently.
    ///
    /// # Example
    ///
    /// ```ignore
    /// env.ensure_owned();
    /// for fact in facts {
    ///     env.add_to_space_shared(&fact); // No CoW copy per-fact
    /// }
    /// ```
    #[inline]
    pub fn ensure_owned(&mut self) {
        self.make_owned();
    }

    /// Add a fact to MORK Space using interior mutability (no CoW copy).
    ///
    /// This method uses `&self` (not `&mut self`) and operates directly on the
    /// shared state via interior mutability. This is critical for arena mode
    /// where environments are cloned but should share space state updates.
    ///
    /// # Thread Safety
    ///
    /// Uses `RwLock::write()` for PathMap access and atomic operations for counters.
    /// Safe to call from multiple clones of the same environment.
    ///
    /// # When to Use
    ///
    /// - When you have multiple environment clones that should share space state
    /// - In loops where calling `add_to_space()` would trigger repeated CoW copies
    /// - In arena mode evaluation where state must persist across cloned environments
    pub fn add_to_space_shared(&self, value: &V) {
        use crate::backend::mork_convert::with_mork_bytes;
        use crate::backend::varint_encoding::value_to_varint_key_generic;
        use super::multiplicity::add_atom;

        // Direct V → MORK bytes conversion via zero-copy callback
        match with_mork_bytes(value, &self.shared_mapping, |mork_bytes| {
            let mut btm = self.shared.btm.write();
            add_atom(&mut btm, mork_bytes);
            drop(btm);

            self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);

            // Use trait method for head symbol extraction
            if let Some(head) = value.get_head_symbol() {
                let arity = value.get_arity() as u8;
                self.shared.head_arity_bloom.write().insert(head.as_bytes(), arity);
            }
        }) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64)
                // Store V directly (zero-conversion)
                let key = value_to_varint_key_generic(value);

                // Lock ordering: btm before large_expr_pathmap (consistent with remove_from_space_shared)
                {
                    let mut btm = self.shared.btm.write();
                    add_atom(&mut btm, &key);
                }

                {
                    let mut guard = self.shared.large_expr_pathmap.write();
                    let fallback = guard.get_or_insert_with(PathMap::new);
                    fallback.insert(&key, value.clone());
                }

                self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Mark as modified for union() fast-path detection
        self.mark_modified();
    }

    /// Remove a fact from MORK Space using interior mutability (no CoW copy).
    ///
    /// This method uses `&self` (not `&mut self`) and operates directly on the
    /// shared state via interior mutability.
    ///
    /// # Thread Safety
    ///
    /// Uses `RwLock::write()` for PathMap access and atomic operations for counters.
    /// Safe to call from multiple clones of the same environment.
    pub fn remove_from_space_shared(&self, value: &V) {
        use crate::backend::mork_convert::with_mork_bytes;
        use crate::backend::varint_encoding::value_to_varint_key_generic;
        use super::multiplicity::{get_multiplicity, remove_atom};

        // Direct V → MORK bytes conversion via zero-copy callback
        match with_mork_bytes(value, &self.shared_mapping, |mork_bytes| {
            let mut btm = self.shared.btm.write();

            let current_count = get_multiplicity(&btm, mork_bytes);
            if current_count == 0 {
                if !btm.contains(mork_bytes) {
                    return;
                }
                btm.remove(mork_bytes);
                drop(btm);
                self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
                self.shared.head_arity_bloom.write().note_deletion();
                self.mark_modified();
                return;
            }

            let new_count = remove_atom(&mut btm, mork_bytes);

            if new_count == 0 {
                self.shared.head_arity_bloom.write().note_deletion();
            }

            drop(btm);
            self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
        }) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64)
                let key = value_to_varint_key_generic(value);

                {
                    let mut btm = self.shared.btm.write();
                    remove_atom(&mut btm, &key);
                }

                let mut guard = self.shared.large_expr_pathmap.write();
                if let Some(ref mut fallback) = *guard {
                    if fallback.contains(&key) {
                        fallback.remove(&key);
                        self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }
        }

        // Mark as modified for union() fast-path detection
        self.mark_modified();
    }

    /// Match pattern against all atoms in the Space.
    ///
    /// Returns `MultiplicityMatch` structs containing the instantiated template and its
    /// multiplicity count. This deferred expansion design avoids cloning the template N times
    /// for atoms with multiplicity N.
    ///
    /// ## Zero-Conversion Architecture
    ///
    /// Uses generic functions that operate directly on V:
    /// - `mork_bytes_to_generic_value()` - MORK bytes → V
    /// - `pattern_match_generic()` - pattern matching on V
    /// - `apply_bindings_generic()` - template instantiation on V
    ///
    /// No intermediate `MettaValue` conversions occur in the hot path.
    pub fn match_space(&self, pattern: &V, template: &V) -> Vec<MultiplicityMatch<V>> {
        use crate::backend::eval::bindings_generic::{
            apply_bindings_generic, pattern_match_generic,
        };
        use super::multiplicity::get_multiplicity;
        use super::mork_encoding::mork_bytes_to_generic_value;

        // Bloom filter check using trait methods (no conversion)
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self.shared.head_arity_bloom.read()
                .may_contain(expected_head.as_bytes(), pattern_arity);
            if !bloom_result {
                return Vec::new();
            }
        }

        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();
        let mut results = Vec::new();

        // Iterate through MORK PathMap
        while rz.to_next_val() {
            let path_bytes = rz.path();
            let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1) as usize;

            // Direct MORK bytes → V conversion (no MettaValue intermediate)
            if let Ok(atom) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                path_bytes,
                &space,
                &self.factory,
            ) {
                // Direct pattern matching on V (no conversion)
                if let Some(bindings) = pattern_match_generic(pattern, &atom) {
                    // Direct template instantiation on V (no conversion)
                    let instantiated = apply_bindings_generic(template, &bindings, &self.factory);
                    results.push(MultiplicityMatch::new(instantiated, multiplicity));
                }
            }
        }

        drop(space);

        // Check large expression fallback PathMap (stores V directly, zero-conversion)
        let guard = self.shared.large_expr_pathmap.read();
        if let Some(ref fallback) = *guard {
            let btm = self.shared.btm.read();

            for (key, stored_value) in fallback.iter() {
                // stored_value is already V (zero-conversion)
                if let Some(bindings) = pattern_match_generic(pattern, stored_value) {
                    let instantiated = apply_bindings_generic(template, &bindings, &self.factory);
                    let multiplicity = get_multiplicity(&btm, &key).max(1) as usize;
                    results.push(MultiplicityMatch::new(instantiated, multiplicity));
                }
            }
        }

        results
    }

    /// Check if any atom in the Space matches the pattern (existence check only).
    ///
    /// This is the fastest query when you only need to know IF a match exists,
    /// not what the match is. It avoids template instantiation overhead.
    ///
    /// ## Zero-Conversion Architecture
    ///
    /// Uses generic functions that operate directly on V:
    /// - `mork_bytes_to_generic_value()` - MORK bytes → V
    /// - `pattern_match_generic()` - pattern matching on V
    pub fn match_space_exists(&self, pattern: &V) -> bool {
        use crate::backend::eval::bindings_generic::pattern_match_generic;
        use super::mork_encoding::mork_bytes_to_generic_value;

        // Bloom filter check using trait methods (no conversion)
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self.shared.head_arity_bloom.read()
                .may_contain(expected_head.as_bytes(), pattern_arity)
            {
                return false;
            }
        }

        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();

        while rz.to_next_val() {
            let path_bytes = rz.path();

            // Direct MORK bytes → V conversion (no MettaValue intermediate)
            if let Ok(atom) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                path_bytes,
                &space,
                &self.factory,
            ) {
                // Direct pattern matching on V (no conversion)
                if pattern_match_generic(pattern, &atom).is_some() {
                    return true;
                }
            }
        }

        drop(space);

        // Check large expression fallback PathMap (stores V directly, zero-conversion)
        let guard = self.shared.large_expr_pathmap.read();
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                // stored_value is already V (zero-conversion)
                if pattern_match_generic(pattern, stored_value).is_some() {
                    return true;
                }
            }
        }

        false
    }

    /// Get all atoms from the Space (MORK PathMap + large expression fallback).
    ///
    /// This iterates the same data as `match_space()` but without pattern filtering,
    /// returning every stored atom as-is.
    ///
    /// ## Zero-Conversion Architecture
    ///
    /// Uses `mork_bytes_to_generic_value()` for direct MORK bytes → V conversion.
    pub fn get_all_atoms(&self) -> Vec<V> {
        use super::mork_encoding::mork_bytes_to_generic_value;

        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();
        let mut atoms = Vec::new();

        // Iterate through MORK PathMap
        while rz.to_next_val() {
            let path_bytes = rz.path();
            if let Ok(atom) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                path_bytes,
                &space,
                &self.factory,
            ) {
                atoms.push(atom);
            }
        }

        drop(space);

        // Include large expression fallback PathMap (stores V directly)
        let guard = self.shared.large_expr_pathmap.read();
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                atoms.push(stored_value.clone());
            }
        }

        atoms
    }
}

// ============================================================================
// Type Aliases for Convenience
// ============================================================================

/// Arena-allocated environment using MettaValue with GcFactory.
///
/// This environment type uses the global slab allocator for zero-conversion evaluation.
/// MettaValue is Copy (8 bytes, thin pointer).
pub type MettaEnvironment = GenericEnvironment<MettaValue, GcFactory>;

impl Default for MettaEnvironment {
    fn default() -> Self {
        GenericEnvironment::new(GcFactory::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generic_environment_new() {
        let env: MettaEnvironment = MettaEnvironment::default();
        assert!(env.owns_data);
        assert!(!env.is_modified());
    }

    #[test]
    fn test_generic_environment_clone_cow() {
        let env1: MettaEnvironment = MettaEnvironment::default();
        let env2 = env1.clone();

        // Clone should not own data
        assert!(env1.owns_data);
        assert!(!env2.owns_data);

        // Both should share the same Arc
        assert!(Arc::ptr_eq(&env1.shared, &env2.shared));
    }

    #[test]
    fn test_generic_environment_add_rule() {
        let mut env: MettaEnvironment = MettaEnvironment::default();

        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);
        let rhs = MettaValue::Long(42);

        env.add_rule(lhs.clone(), rhs);

        // Should have one rule for (add, 2)
        let rules = env.get_matching_rules_for_expr(&lhs);
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_generic_environment_bind() {
        let mut env: MettaEnvironment = MettaEnvironment::default();

        env.bind("x", MettaValue::Long(42));

        assert!(env.has_binding("x"));
        assert_eq!(env.get_binding("x"), Some(MettaValue::Long(42)));
        assert!(!env.has_binding("y"));
    }

    #[test]
    fn test_generic_environment_named_space() {
        let mut env: MettaEnvironment = MettaEnvironment::default();

        let space_id = env.create_named_space("test");
        assert!(env.has_named_space(space_id));

        env.add_to_named_space(space_id, MettaValue::Long(1));
        env.add_to_named_space(space_id, MettaValue::Long(2));

        let atoms = env.collapse_named_space(space_id);
        assert_eq!(atoms.len(), 2);
    }

    #[test]
    fn test_generic_environment_fork() {
        let mut env1: MettaEnvironment = MettaEnvironment::default();
        env1.bind("x", MettaValue::Long(1));

        let mut env2 = env1.fork_for_nondeterminism();

        // Forked env should own its data
        assert!(env2.owns_data);

        // Modify forked env
        env2.bind("x", MettaValue::Long(2));

        // Original should be unchanged
        assert_eq!(env1.get_binding("x"), Some(MettaValue::Long(1)));
        assert_eq!(env2.get_binding("x"), Some(MettaValue::Long(2)));
    }

    #[test]
    fn test_generic_environment_state() {
        let mut env: MettaEnvironment = MettaEnvironment::default();

        // Create state
        let state_id = env.create_state(&MettaValue::Long(42));
        assert!(env.has_state(state_id));

        // Get state
        let value = env.get_state(state_id);
        assert_eq!(value, Some(MettaValue::Long(42)));

        // Change state
        assert!(env.change_state(state_id, &MettaValue::Long(100)));
        let new_value = env.get_state(state_id);
        assert_eq!(new_value, Some(MettaValue::Long(100)));

        // Non-existent state
        assert!(!env.has_state(999));
        assert_eq!(env.get_state(999), None);
    }

    #[test]
    fn test_generic_environment_state_shared_across_clones() {
        let mut env1: MettaEnvironment = MettaEnvironment::default();

        // Create state in original
        let state_id = env1.create_state(&MettaValue::Long(1));

        // Clone (sharing Arc)
        let env2 = env1.clone();

        // State should be visible in clone
        assert_eq!(env2.get_state(state_id), Some(MettaValue::Long(1)));

        // Modify state - should be visible in both (states are truly mutable)
        env1.change_state(state_id, &MettaValue::Long(2));

        // Both should see the new value
        assert_eq!(env1.get_state(state_id), Some(MettaValue::Long(2)));
        assert_eq!(env2.get_state(state_id), Some(MettaValue::Long(2)));
    }
}
