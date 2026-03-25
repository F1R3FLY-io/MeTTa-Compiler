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
//!         ├── atom_space: AtomSpace<V>  (MettaTrie-backed unified atom storage)
//!         ├── named_spaces: RwLock<HashMap<u64, (String, Vec<V>)>>
//!         ├── bindings: RwLock<HashMap<String, V>>
//!         └── ... (type-agnostic fields: symbols, states, etc.)
//! ```
//!
//! ## Thread Safety
//!
//! Uses non-blocking concurrent data structures for maximum parallelism:
//! - `parking_lot::RwLock<HashMap>` for concurrent map access (single-threaded workloads)
//! - `parking_lot::RwLock` for structures requiring exclusive access (MettaTrie)
//! - `AtomicBool`/`AtomicUsize` for simple flags and counters
//!
//! Clone operations are O(1) via Arc sharing until first mutation (CoW).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use metta_trie::Multiplicity;
use parking_lot::RwLock;
use tracing::trace;

use crate::backend::decompose::{decompose, decompose_literal};
use crate::backend::hash_utils::IdentityU64BuildHasher;
use super::bloom::HeadArityBloomFilter;
use super::multiplicity::{trie_add_atom, trie_get_multiplicity, trie_remove_atom};
use super::rule_management::extract_rule_parts;
use super::scope::ScopeTracker;
use crate::backend::eval::bindings_generic::{apply_bindings_generic, pattern_match_generic};
use crate::backend::fuzzy_match::FuzzyMatcher;
use crate::backend::grounded::{GenericGroundedRegistry, GroundedRegistry};
use crate::backend::models::gc_allocator::{try_register_env_roots, RootProvider};
use crate::backend::models::{
    GcFactory, MettaValue, MettaValueFactory, MettaValueTrait, SpaceHandle,
};
use crate::backend::modules::ModuleRegistry;

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
/// - `parking_lot::RwLock`: For MettaTrie and other non-HashMap structures
/// - `AtomicU64`/`AtomicBool`/`AtomicUsize`: Lock-free counters and flags
pub struct GenericEnvironmentShared<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> {
    // ========================================================================
    // Unified Atom Storage (AtomSpace)
    // ========================================================================
    /// Unified atom storage: MettaTrie (ground atoms) + variable atom Vec.
    /// Contains btm, type_btm, subtype_btm, inferred_type_btm,
    /// head_arity_bloom, type_bloom, total_atoms, and variable_atoms.
    pub(crate) atom_space: super::atom_space::AtomSpace<V>,

    // ========================================================================
    // Mutable State
    // ========================================================================
    /// Mutable state cells registry (stores V directly - no serialization)
    /// Uses RwLock<HashMap> — protected by CoW semantics
    pub(crate) states: RwLock<HashMap<u64, V, IdentityU64BuildHasher>>,

    /// Counter for generating unique state IDs (lock-free atomic)
    pub(crate) next_state_id: AtomicU64,

    // ========================================================================
    // Generic Named Spaces (parameterized over V)
    // ========================================================================
    /// Named spaces registry: Maps space_id -> (name, atoms)
    /// Uses RwLock<HashMap> — protected by CoW semantics
    #[allow(clippy::type_complexity)]
    pub(crate) named_spaces: RwLock<HashMap<u64, (String, Vec<V>), IdentityU64BuildHasher>>,

    /// Counter for generating unique space IDs (lock-free atomic)
    pub(crate) next_space_id: AtomicU64,

    // ========================================================================
    // Generic Symbol Bindings (parameterized over V)
    // ========================================================================
    /// Symbol bindings registry: Maps name -> V
    /// Uses RwLock<HashMap> — protected by CoW semantics
    pub(crate) bindings: RwLock<HashMap<String, V>>,

    /// Type assertions: Maps symbol name -> all declared type values V (nondeterministic)
    /// HE parity: an atom can have multiple types declared via separate `(: name type)` assertions.
    /// Uses RwLock<HashMap<String, Vec<V>>> — protected by CoW semantics.
    pub(crate) types: RwLock<HashMap<String, Vec<V>>>,

    /// Subtype relations: Maps sub-type name -> list of direct super-type names.
    /// HE parity: supports `(:< Sub Super)` declarations with transitive closure.
    /// Uses RwLock<HashMap<String, Vec<String>>> — protected by CoW semantics.
    pub(crate) subtypes: RwLock<HashMap<String, Vec<String>>>,

    // ========================================================================
    // Type-Agnostic Registries and Caches
    // ========================================================================
    /// Module registry (type-agnostic)
    /// Arc-wrapped so fork_for_nondeterminism is O(1) (Arc::clone) instead of
    /// deep-cloning the entire module registry. Writes go through make_owned().
    pub(crate) module_registry: Arc<RwLock<ModuleRegistry>>,

    /// Per-module tokenizer (type-agnostic)
    /// Arc-wrapped so fork_for_nondeterminism is O(1) (Arc::clone) instead of
    /// deep-cloning all tokenizer entries. Writes go through make_owned().
    pub(crate) tokenizer: Arc<RwLock<crate::backend::modules::GenericTokenizer<V>>>,

    /// Grounded operations registry (legacy, used by proptests only)
    /// Arc-wrapped so fork_for_nondeterminism is O(1) (Arc::clone) instead of
    /// deep-cloning the registry. Writes go through make_owned().
    pub(crate) grounded_registry: Arc<RwLock<GroundedRegistry>>,

    /// Generic grounded operations registry (type-parameterized, zero-conversion)
    /// Stateless and Clone, no lock needed
    pub(crate) generic_grounded_registry: GenericGroundedRegistry,

    /// Type index: Lazy-initialized snapshot of `type_btm` MettaTrie.
    /// Rebuilt when `type_index_dirty` is true.
    /// Uses RwLock (MettaTrie requires exclusive access for mutation)
    pub(crate) type_index: RwLock<Option<metta_trie::MettaTrie<V, Multiplicity>>>,

    /// Type index invalidation flag (lock-free atomic)
    pub(crate) type_index_dirty: AtomicBool,

    /// Fuzzy matcher for "Did you mean?" suggestions.
    /// Arc-wrapped so fork_for_nondeterminism is O(1) (Arc::clone) instead of
    /// deep-cloning the entire DashSet of head symbols (~4% wall time saved).
    /// Writes go through make_owned() which deep-clones into a new Arc.
    pub(crate) fuzzy_matcher: Arc<RwLock<FuzzyMatcher>>,

    /// Hierarchical scope tracker for context-aware symbol resolution
    /// Arc-wrapped so fork_for_nondeterminism is O(1) (Arc::clone) instead of
    /// deep-cloning the scope tree. Writes go through make_owned().
    pub(crate) scope_tracker: Arc<RwLock<ScopeTracker>>,

    /// In-memory rule index for O(1) lookup + pattern matching.
    /// Populated at `add_rule()` time. Authoritative for rule queries.
    /// MettaTrie remains the storage-of-record (for match_space, serialization).
    /// Arc-wrapped so fork_for_nondeterminism is O(1) (Arc::clone) instead of
    /// deep-cloning the entire HashMap of rule entries (~2.3% wall time saved).
    /// Writes go through make_owned() which deep-clones into a new Arc.
    pub(crate) rule_index: Arc<RwLock<super::rule_management::RuleIndex<V>>>,

    /// Phase 10.1: Inferred function return types from rule RHS analysis.
    /// Maps function name → Vec of inferred return types (nondeterministic).
    /// Separate from `types` to distinguish declared vs inferred.
    ///
    /// DashMap: lock-free per-shard reads/writes — no reader blocking.
    /// Reads (during type inference) are high-frequency; writes (at add_rule)
    /// are low-frequency. DashMap avoids writer-blocks-readers stalls.
    /// Fork: iterate + clone into new DashMap (infrequent operation).
    /// Arc-wrapped for O(1) fork in `fork_for_nondeterminism()`.
    /// Safe to share because `inferred_fn_types` is only written during
    /// rule addition (setup), never during nondeterministic evaluation.
    pub(crate) inferred_fn_types: Arc<DashMap<String, Vec<V>>>,
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

    /// CoW: Tracks if this clone owns its data
    pub(crate) owns_data: bool,

    /// CoW: Tracks if this environment has been modified
    pub(crate) modified: AtomicBool,

    /// Current module path for relative path resolution.
    /// Arc-wrapped for O(1) clone — module path is semantically immutable after construction.
    pub(crate) current_module_path: Option<Arc<PathBuf>>,
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
    /// like `match_space` that need to construct values.
    pub fn new(factory: F) -> Self {
        let shared = Arc::new(GenericEnvironmentShared {
            // Unified atom storage
            atom_space: super::atom_space::AtomSpace::new(10000),

            // Mutable state
            states: RwLock::new(HashMap::with_hasher(IdentityU64BuildHasher)),
            next_state_id: AtomicU64::new(1),

            // Generic named spaces
            named_spaces: RwLock::new(HashMap::with_hasher(IdentityU64BuildHasher)),
            next_space_id: AtomicU64::new(1),

            // Generic symbol bindings
            bindings: RwLock::new(HashMap::new()),

            // Type assertions storage
            types: RwLock::new(HashMap::new()),
            // Subtype relations storage
            subtypes: RwLock::new(HashMap::new()),

            // Type-agnostic registries (Arc-wrapped for O(1) fork)
            module_registry: Arc::new(RwLock::new(ModuleRegistry::new())),
            tokenizer: Arc::new(RwLock::new(crate::backend::modules::GenericTokenizer::<V>::new())),
            grounded_registry: Arc::new(RwLock::new(GroundedRegistry::new())),
            generic_grounded_registry: GenericGroundedRegistry::with_standard_ops(),
            type_index: RwLock::new(None),
            type_index_dirty: AtomicBool::new(true),
            fuzzy_matcher: Arc::new(RwLock::new(FuzzyMatcher::new())),
            scope_tracker: Arc::new(RwLock::new(ScopeTracker::new())),
            rule_index: Arc::new(RwLock::new(super::rule_management::RuleIndex::new())),
            // Phase 10.1: Inferred function return types (initially empty)
            inferred_fn_types: Arc::new(DashMap::new()),
        });

        // Register as GC root provider (no-op if V != MettaValue)
        try_register_env_roots(&shared);

        GenericEnvironment {
            shared,
            factory,
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
            // Deep-copy atom storage (make_owned needs exclusive copies for mutation)
            atom_space: {
                let forked = self.shared.atom_space.fork();
                // Extract values before constructing (avoid borrow-of-moved issues)
                let forked_btm = forked.btm.read().clone();
                let forked_count = forked.total_atoms.load(Ordering::Acquire);
                let forked_var_atoms = forked.variable_atoms.read().clone();
                let forked_type_btm = forked.type_btm.read().clone();
                let forked_subtype_btm = forked.subtype_btm.read().clone();
                let forked_inferred_type_btm = forked.inferred_type_btm.read().clone();
                // Deep-clone bloom filter into a new Arc for exclusive mutation
                super::atom_space::AtomSpace {
                    btm: RwLock::new(forked_btm),
                    type_btm: RwLock::new(forked_type_btm),
                    subtype_btm: RwLock::new(forked_subtype_btm),
                    // Phase 10.1: deep-clone inferred type MettaTrie and bloom for exclusive mutation
                    inferred_type_btm: RwLock::new(forked_inferred_type_btm),
                    inferred_type_bloom: std::sync::Arc::new(
                        self.shared.atom_space.inferred_type_bloom.snapshot(),
                    ),
                    // Phase 10.5: snapshot generation counters for exclusive mutation
                    inferred_type_generation: AtomicU64::new(
                        self.shared.atom_space.inferred_type_generation.load(Ordering::Acquire),
                    ),
                    fixpoint_generation: AtomicU64::new(
                        self.shared.atom_space.fixpoint_generation.load(Ordering::Acquire),
                    ),
                    head_arity_bloom: std::sync::Arc::new(RwLock::new(
                        self.shared.atom_space.head_arity_bloom.read().clone(),
                    )),
                    type_bloom: std::sync::Arc::new(RwLock::new(
                        self.shared.atom_space.type_bloom.read().clone(),
                    )),
                    total_atoms: AtomicUsize::new(forked_count),
                    variable_atoms: RwLock::new(forked_var_atoms),
                }
            },
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
            // Subtype relations - RwLock<HashMap>
            subtypes: RwLock::new(self.shared.subtypes.read().clone()),

            // Type-agnostic registries — deep-clone into new Arcs so this
            // owned env has exclusive copies for mutation
            module_registry: Arc::new(RwLock::new(self.shared.module_registry.read().clone())),
            tokenizer: Arc::new(RwLock::new(self.shared.tokenizer.read().clone())),
            grounded_registry: Arc::new(RwLock::new(self.shared.grounded_registry.read().clone())),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),
            type_index: RwLock::new(self.shared.type_index.read().clone()),
            type_index_dirty: AtomicBool::new(
                self.shared.type_index_dirty.load(Ordering::Acquire),
            ),
            // Deep-clone into new Arcs so this owned env has exclusive copies
            fuzzy_matcher: Arc::new(RwLock::new(self.shared.fuzzy_matcher.read().clone())),
            scope_tracker: Arc::new(RwLock::new(self.shared.scope_tracker.read().clone())),
            // Deep-clone into new Arc so this owned env has an exclusive copy
            rule_index: Arc::new(RwLock::new(self.shared.rule_index.read().clone())),
            // Phase 10.1: deep-clone DashMap into independent copy for exclusive mutation
            inferred_fn_types: Arc::new(DashMap::from_iter(
                self.shared.inferred_fn_types.iter().map(|e| (e.key().clone(), e.value().clone()))
            )),
        });

        // Register new shared state as GC root provider
        try_register_env_roots(&new_shared);

        self.shared = new_shared;
        self.owns_data = true;
        self.mark_modified();
    }

    /// Create a forked environment for nondeterministic branch isolation.
    ///
    /// Uses O(1) MettaTrie CoW clone for fork isolation.
    pub fn fork_for_nondeterminism(&self) -> Self {
        trace!(target: "mettatron::generic_environment::fork", "Forking environment for nondeterminism");

        let new_shared = Arc::new(GenericEnvironmentShared {
            // Fork atom storage (MettaTrie CoW + bloom Arc::clone)
            atom_space: self.shared.atom_space.fork(),

            states: RwLock::new(self.shared.states.read().clone()),
            next_state_id: AtomicU64::new(self.shared.next_state_id.load(Ordering::Acquire)),

            // Generic named spaces - RwLock<HashMap>
            named_spaces: RwLock::new(self.shared.named_spaces.read().clone()),
            next_space_id: AtomicU64::new(self.shared.next_space_id.load(Ordering::Acquire)),

            // Generic symbol bindings - RwLock<HashMap>
            bindings: RwLock::new(self.shared.bindings.read().clone()),

            // Type assertions - RwLock<HashMap>
            types: RwLock::new(self.shared.types.read().clone()),
            // Subtype relations - RwLock<HashMap>
            subtypes: RwLock::new(self.shared.subtypes.read().clone()),

            // O(1) Arc::clone for all read-only registries — forked envs
            // don't modify these during evaluation, so sharing is safe.
            module_registry: Arc::clone(&self.shared.module_registry),
            tokenizer: Arc::clone(&self.shared.tokenizer),
            grounded_registry: Arc::clone(&self.shared.grounded_registry),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),
            type_index: RwLock::new(self.shared.type_index.read().clone()),
            type_index_dirty: AtomicBool::new(
                self.shared.type_index_dirty.load(Ordering::Acquire),
            ),
            // O(1) Arc::clone for all read-only state during evaluation
            fuzzy_matcher: Arc::clone(&self.shared.fuzzy_matcher),
            scope_tracker: Arc::clone(&self.shared.scope_tracker),
            rule_index: Arc::clone(&self.shared.rule_index),
            // O(1) Arc::clone — inferred_fn_types is only written during
            // rule addition (setup), never during nondeterministic evaluation.
            inferred_fn_types: Arc::clone(&self.shared.inferred_fn_types),
        });

        // Register forked shared state as GC root provider
        try_register_env_roots(&new_shared);

        GenericEnvironment {
            shared: new_shared,
            factory: self.factory.clone(),
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
    ///    - MettaTrie facts: take max multiplicity for each path via merge_max()
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
                owns_data: false,
                modified: AtomicBool::new(false),
                current_module_path: other.current_module_path.clone(),
            };
        }

        // Both modified: perform actual merge
        trace!(target: "mettatron::generic_environment::union", "Both environments modified, performing merge");

        // Merge MettaTries by taking max multiplicity
        let merged_btm = {
            let self_btm = self.shared.atom_space.btm.read();
            let other_btm = other.shared.atom_space.btm.read();
            self_btm.merge_max(&other_btm)
        };

        // Calculate total atoms from merged MettaTrie
        let merged_total_atoms: usize = merged_btm
            .iter()
            .map(|(_, m)| m.count() as usize)
            .sum();

        // Rules are stored as (= lhs rhs) in MettaTrie — merged via merge_max above.

        // Merge bindings (other takes precedence)
        let merged_bindings: HashMap<String, V> = {
            let mut merged = self.shared.bindings.read().clone();
            for (k, v) in other.shared.bindings.read().iter() {
                merged.insert(k.clone(), v.clone());
            }
            merged
        };

        // Merge types (union of type Vecs per key, with dedup)
        let merged_types: HashMap<String, Vec<V>> = {
            let mut merged = self.shared.types.read().clone();
            for (k, other_types) in other.shared.types.read().iter() {
                let vec = merged.entry(k.clone()).or_default();
                for t in other_types {
                    if !vec.contains(t) {
                        vec.push(t.clone());
                    }
                }
            }
            merged
        };

        // Merge subtypes (union of super-type Vecs per key, with dedup)
        let merged_subtypes: HashMap<String, Vec<String>> = {
            let mut merged = self.shared.subtypes.read().clone();
            for (k, other_supers) in other.shared.subtypes.read().iter() {
                let vec = merged.entry(k.clone()).or_default();
                for s in other_supers {
                    if !vec.contains(s) {
                        vec.push(s.clone());
                    }
                }
            }
            merged
        };

        // Merge states (other takes precedence)
        let merged_states: HashMap<u64, V, IdentityU64BuildHasher> = {
            let mut merged = self.shared.states.read().clone();
            for (k, v) in other.shared.states.read().iter() {
                merged.insert(*k, v.clone());
            }
            merged
        };

        // Merge named spaces (combine atoms within same space)
        let merged_named_spaces: HashMap<u64, (String, Vec<V>), IdentityU64BuildHasher> = {
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
            atom_space: super::atom_space::AtomSpace {
                btm: RwLock::new(merged_btm),
                head_arity_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(10000))), // Reset (will be rebuilt)
                type_bloom: std::sync::Arc::new(RwLock::new(super::bloom::TypeBloomFilter::new(1000))), // Reset (will be rebuilt from type_btm)
                // Merge type and subtype MettaTries
                type_btm: RwLock::new(
                    self.shared.atom_space.type_btm.read()
                        .merge_max(&other.shared.atom_space.type_btm.read()),
                ),
                subtype_btm: RwLock::new(
                    self.shared.atom_space.subtype_btm.read()
                        .merge_max(&other.shared.atom_space.subtype_btm.read()),
                ),
                // Phase 10.1: merge inferred type MettaTries
                inferred_type_btm: RwLock::new(
                    self.shared.atom_space.inferred_type_btm.read()
                        .merge_max(&other.shared.atom_space.inferred_type_btm.read()),
                ),
                // Phase 10.1: merge inferred type bloom via bitwise OR
                inferred_type_bloom: {
                    let merged = std::sync::Arc::new(
                        self.shared.atom_space.inferred_type_bloom.snapshot(),
                    );
                    merged.merge_from(&other.shared.atom_space.inferred_type_bloom);
                    merged
                },
                // Phase 10.5: max(generation) forces fixpoint to see all new types;
                // min(fixpoint_gen) forces re-fixpoint if either side had unprocessed types.
                inferred_type_generation: AtomicU64::new(
                    self.shared.atom_space.inferred_type_generation.load(Ordering::Acquire)
                        .max(other.shared.atom_space.inferred_type_generation.load(Ordering::Acquire)),
                ),
                fixpoint_generation: AtomicU64::new(
                    self.shared.atom_space.fixpoint_generation.load(Ordering::Acquire)
                        .min(other.shared.atom_space.fixpoint_generation.load(Ordering::Acquire)),
                ),
                total_atoms: AtomicUsize::new(merged_total_atoms),
                variable_atoms: RwLock::new(Vec::new()),
            },
            states: RwLock::new(merged_states),
            next_state_id: AtomicU64::new(max_state_id),

            named_spaces: RwLock::new(merged_named_spaces),
            next_space_id: AtomicU64::new(max_space_id),

            bindings: RwLock::new(merged_bindings),
            types: RwLock::new(merged_types),
            subtypes: RwLock::new(merged_subtypes),

            // Share from self (these are typically static after initialization)
            module_registry: Arc::new(RwLock::new(self.shared.module_registry.read().clone())),
            tokenizer: Arc::new(RwLock::new(self.shared.tokenizer.read().clone())),
            grounded_registry: Arc::new(RwLock::new(self.shared.grounded_registry.read().clone())),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),

            // Invalidate type index after merge
            type_index: RwLock::new(None),
            type_index_dirty: AtomicBool::new(true),

            fuzzy_matcher: Arc::new(RwLock::new(merged_fuzzy)),
            scope_tracker: Arc::new(RwLock::new(other.shared.scope_tracker.read().clone())), // Use other's scope
            // Merge rule indices from both environments
            rule_index: {
                let mut merged = self.shared.rule_index.read().clone();
                for entry in other.shared.rule_index.read().get_all_rules() {
                    let head = entry.lhs.get_head_symbol().map(|s| s.to_string());
                    let arity = entry.lhs.get_arity();
                    let alloc = crate::backend::models::gc_allocator::global_allocator();
                    let first_arg_head = super::rule_management::get_first_arg_head(&entry.lhs)
                        .map(|s| alloc.alloc_str(s));
                    merged.add_rule(head.as_deref(), arity, first_arg_head, entry.clone());
                }
                Arc::new(RwLock::new(merged))
            },
            // Phase 10.1: merge inferred function types (DashMap union with dedup)
            inferred_fn_types: if Arc::ptr_eq(&self.shared.inferred_fn_types, &other.shared.inferred_fn_types) {
                // Both forks share the same DashMap — O(1) clone
                Arc::clone(&self.shared.inferred_fn_types)
            } else {
                let merged: DashMap<String, Vec<V>> = DashMap::from_iter(
                    self.shared.inferred_fn_types.iter().map(|e| (e.key().clone(), e.value().clone()))
                );
                for entry in other.shared.inferred_fn_types.iter() {
                    let mut vec = merged.entry(entry.key().clone()).or_default();
                    for t in entry.value() {
                        if !vec.contains(t) {
                            vec.push(t.clone());
                        }
                    }
                }
                Arc::new(merged)
            },
        });

        // Repopulate type bloom filter from merged types HashMap
        {
            let types = new_shared.types.read();
            let mut bloom = new_shared.atom_space.type_bloom.write();
            for name in types.keys() {
                bloom.insert(name);
            }
        }

        // Register merged shared state as GC root provider
        try_register_env_roots(&new_shared);

        GenericEnvironment {
            shared: new_shared,
            factory: self.factory.clone(),
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

        // Merge MettaTries by taking max multiplicity
        let merged_btm = {
            let mut result = base.shared.atom_space.btm.read().clone();
            for other in &others[merge_start_idx..] {
                let other_btm = other.shared.atom_space.btm.read();
                result = result.merge_max(&other_btm);
            }
            // If we started from self and include_self is true, we already have self's data
            // Otherwise merge self's data too
            if !include_self {
                let self_btm = self.shared.atom_space.btm.read();
                result = result.merge_max(&self_btm);
            }
            result
        };

        // Calculate total atoms from merged MettaTrie
        let merged_total_atoms: usize = merged_btm
            .iter()
            .map(|(_, m)| m.count() as usize)
            .sum();

        // Rules are stored as (= lhs rhs) in MettaTrie — merged via merge_max above.

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

        // Merge types (union of type Vecs per key, with dedup)
        let merged_types: HashMap<String, Vec<V>> = {
            let mut base_types = base.shared.types.read().clone();
            for other in &others[merge_start_idx..] {
                for (k, other_types) in other.shared.types.read().iter() {
                    let vec = base_types.entry(k.clone()).or_default();
                    for t in other_types {
                        if !vec.contains(t) {
                            vec.push(t.clone());
                        }
                    }
                }
            }
            base_types
        };

        // Merge subtypes (union of super-type Vecs per key, with dedup)
        let merged_subtypes: HashMap<String, Vec<String>> = {
            let mut base_subtypes = base.shared.subtypes.read().clone();
            for other in &others[merge_start_idx..] {
                for (k, other_supers) in other.shared.subtypes.read().iter() {
                    let vec = base_subtypes.entry(k.clone()).or_default();
                    for s in other_supers {
                        if !vec.contains(s) {
                            vec.push(s.clone());
                        }
                    }
                }
            }
            base_subtypes
        };

        // Merge states (later environments take precedence)
        let merged_states: HashMap<u64, V, IdentityU64BuildHasher> = {
            let mut base_states = base.shared.states.read().clone();
            for other in &others[merge_start_idx..] {
                for (k, v) in other.shared.states.read().iter() {
                    base_states.insert(*k, v.clone());
                }
            }
            base_states
        };

        // Merge named spaces
        let merged_named_spaces: HashMap<u64, (String, Vec<V>), IdentityU64BuildHasher> = {
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
            atom_space: super::atom_space::AtomSpace {
                btm: RwLock::new(merged_btm),
                head_arity_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(10000))), // Reset (will be rebuilt)
                type_bloom: std::sync::Arc::new(RwLock::new(super::bloom::TypeBloomFilter::new(1000))), // Reset (will be rebuilt from type_btm)
                // Merge type MettaTries from all environments
                type_btm: RwLock::new({
                    let mut merged_types_trie = self.shared.atom_space.type_btm.read().clone();
                    for other_env in others.iter() {
                        merged_types_trie = merged_types_trie.merge_max(&other_env.shared.atom_space.type_btm.read());
                    }
                    merged_types_trie
                }),
                // Merge subtype MettaTries from all environments
                subtype_btm: RwLock::new({
                    let mut merged_subs = self.shared.atom_space.subtype_btm.read().clone();
                    for other_env in others.iter() {
                        merged_subs = merged_subs.merge_max(&other_env.shared.atom_space.subtype_btm.read());
                    }
                    merged_subs
                }),
                // Phase 10.1: merge inferred type MettaTries from all environments
                inferred_type_btm: RwLock::new({
                    let mut merged_inf = self.shared.atom_space.inferred_type_btm.read().clone();
                    for other_env in others.iter() {
                        merged_inf = merged_inf.merge_max(&other_env.shared.atom_space.inferred_type_btm.read());
                    }
                    merged_inf
                }),
                // Phase 10.1: merge inferred type bloom filters via bitwise OR
                inferred_type_bloom: {
                    let merged_bloom = std::sync::Arc::new(
                        self.shared.atom_space.inferred_type_bloom.snapshot(),
                    );
                    for other_env in others.iter() {
                        merged_bloom.merge_from(&other_env.shared.atom_space.inferred_type_bloom);
                    }
                    merged_bloom
                },
                // Phase 10.5: max(generation) across all envs; min(fixpoint_gen) forces re-fixpoint
                inferred_type_generation: AtomicU64::new({
                    let mut max_gen = self.shared.atom_space.inferred_type_generation.load(Ordering::Acquire);
                    for other_env in others.iter() {
                        max_gen = max_gen.max(other_env.shared.atom_space.inferred_type_generation.load(Ordering::Acquire));
                    }
                    max_gen
                }),
                fixpoint_generation: AtomicU64::new({
                    let mut min_gen = self.shared.atom_space.fixpoint_generation.load(Ordering::Acquire);
                    for other_env in others.iter() {
                        min_gen = min_gen.min(other_env.shared.atom_space.fixpoint_generation.load(Ordering::Acquire));
                    }
                    min_gen
                }),
                total_atoms: AtomicUsize::new(merged_total_atoms),
                variable_atoms: RwLock::new(Vec::new()),
            },
            states: RwLock::new(merged_states),
            next_state_id: AtomicU64::new(max_state_id),

            named_spaces: RwLock::new(merged_named_spaces),
            next_space_id: AtomicU64::new(max_space_id),

            bindings: RwLock::new(merged_bindings),
            types: RwLock::new(merged_types),
            subtypes: RwLock::new(merged_subtypes),

            // Share from self (typically static after init)
            module_registry: Arc::new(RwLock::new(self.shared.module_registry.read().clone())),
            tokenizer: Arc::new(RwLock::new(self.shared.tokenizer.read().clone())),
            grounded_registry: Arc::new(RwLock::new(self.shared.grounded_registry.read().clone())),

            generic_grounded_registry: self.shared.generic_grounded_registry.clone(),

            // Invalidate type index after merge
            type_index: RwLock::new(None),
            type_index_dirty: AtomicBool::new(true),

            fuzzy_matcher: Arc::new(RwLock::new(merged_fuzzy)),
            scope_tracker: Arc::new(RwLock::new(last_env.shared.scope_tracker.read().clone())),
            // Merge rule indices from all environments
            rule_index: {
                let mut merged = self.shared.rule_index.read().clone();
                for other_env in others {
                    for entry in other_env.shared.rule_index.read().get_all_rules() {
                        let head = entry.lhs.get_head_symbol().map(|s| s.to_string());
                        let arity = entry.lhs.get_arity();
                        let alloc = crate::backend::models::gc_allocator::global_allocator();
                        let first_arg_head = super::rule_management::get_first_arg_head(&entry.lhs)
                            .map(|s| alloc.alloc_str(s));
                        merged.add_rule(head.as_deref(), arity, first_arg_head, entry.clone());
                    }
                }
                Arc::new(RwLock::new(merged))
            },
            // Phase 10.1: merge inferred function types from all environments
            inferred_fn_types: {
                // Fast path: if all environments share the same DashMap, just Arc::clone
                let all_same = others.iter().all(|o| Arc::ptr_eq(&self.shared.inferred_fn_types, &o.shared.inferred_fn_types));
                if all_same {
                    Arc::clone(&self.shared.inferred_fn_types)
                } else {
                    let merged: DashMap<String, Vec<V>> = DashMap::from_iter(
                        self.shared.inferred_fn_types.iter().map(|e| (e.key().clone(), e.value().clone()))
                    );
                    for other_env in others {
                        for entry in other_env.shared.inferred_fn_types.iter() {
                            let mut vec = merged.entry(entry.key().clone()).or_default();
                            for t in entry.value() {
                                if !vec.contains(t) {
                                    vec.push(t.clone());
                                }
                            }
                        }
                    }
                    Arc::new(merged)
                }
            },
        });

        // Repopulate type bloom filter from merged types HashMap
        {
            let types = new_shared.types.read();
            let mut bloom = new_shared.atom_space.type_bloom.write();
            for name in types.keys() {
                bloom.insert(name);
            }
        }

        // Register batch-merged shared state as GC root provider
        try_register_env_roots(&new_shared);

        GenericEnvironment {
            shared: new_shared,
            factory: self.factory.clone(),
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
    pub fn current_module_path(&self) -> Option<&Path> {
        self.current_module_path.as_deref().map(|p| p.as_path())
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
    /// Note: MettaTrie stores original expressions at leaves — these are collected
    /// via `atom_space.collect_gc_roots()`.
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

        // Type assertions (flatten Vec<V> per key)
        {
            let types = self.shared.types.read();
            for type_vec in types.values() {
                roots.extend(type_vec.iter().cloned());
            }
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
            owns_data: false, // CoW: clones do not own data initially
            modified: AtomicBool::new(false),
            current_module_path: self.current_module_path.clone(),
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

impl RootProvider for GenericEnvironmentShared<MettaValue> {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        // Pre-estimate capacity from all sources to eliminate Vec reallocations.
        // Read locks under quiescent GC are uncontended (ACTIVE_EVALUATORS == 0).
        {
            let estimated =
                self.named_spaces.read().values().map(|(_, a)| a.len()).sum::<usize>()
                + self.bindings.read().len()
                + self.types.read().len()
                + self.states.read().len()
                + self.rule_index.read().len() * 2 // lhs + rhs per entry
                + 64; // buffer for tokenizer + variable_atoms
            roots.reserve(estimated);
        }

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

        // Type assertions (flatten Vec<MettaValue> per key)
        {
            let types = self.types.read();
            for type_vec in types.values() {
                roots.extend(type_vec.iter().copied());
            }
        }

        // Mutable state cells
        {
            let states = self.states.read();
            roots.extend(states.values().copied());
        }

        // AtomSpace GC roots: variable atoms + MettaTrie leaf values.
        // These hold slab-allocated MettaValues that must be kept alive by GC.
        self.atom_space.collect_gc_roots(roots);

        // Tokenizer values: bind! stores MettaValues inside closures.
        // Without collecting these, GC frees Space handles (e.g., &kb, &stack)
        // and State values (e.g., &sp) that are still looked up via token resolution.
        {
            let tokenizer = self.tokenizer.read();
            tokenizer.collect_gc_values_into(roots);
        }

        // RuleIndex: cached LHS/RHS/rhs_type MettaValues for rule matching.
        // Without collecting these, GC frees slab slots still referenced by
        // RuleEntry fields, causing use-after-free when match_rules_native()
        // applies bindings to the RHS template or branch pruning reads rhs_type.
        {
            let rule_index = self.rule_index.read();
            roots.extend(rule_index.get_all_rules().flat_map(|e| {
                let mut vals = vec![e.lhs, e.rhs];
                if let Some(rt) = &e.rhs_type {
                    vals.push(rt.clone());
                }
                vals
            }));
        }

        // Inferred function types: Phase 10.1 caches return types from rule RHS analysis.
        // Without collecting these, GC frees slab-allocated type atoms still referenced
        // by type inference lookups (e.g., the $a atom from let*'s (-> Bindings $a $a)).
        for entry in self.inferred_fn_types.iter() {
            roots.extend(entry.value().iter().cloned());
        }
    }
}


// ============================================================================
// Space Access Methods
// ============================================================================

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Get the total atom count (O(1)).
    pub fn total_atoms(&self) -> usize {
        self.shared.atom_space.total_atoms.load(Ordering::Acquire)
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
// Generic Space Operations via MettaTrie - Zero-Conversion Architecture
// ============================================================================
//
// These methods use MettaTrie decomposition functions that work directly
// with any V: MettaValueTrait, avoiding intermediate conversions.
//
// Data flow:
//   V → decompose_literal() → TrieKey sequence → MettaTrie storage
//   MettaTrie::iter() → (&V, &Multiplicity) directly — no deserialization needed
//   MettaTrie::query() → QueryMatch { expr, value, bindings } — direct V access

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Add a fact to the MettaTrie Space for pattern matching.
    ///
    /// ## Unified Routing
    ///
    /// Automatically detects and routes special atom types:
    /// - Rules `(= lhs rhs)` → `add_rule()` for RuleIndex + MettaTrie population
    /// - Type assertions `(: name type)` → MettaTrie + types HashMap registration
    /// - All other atoms → literal MettaTrie encoding
    ///
    /// ## Multiplicity Tracking
    ///
    /// Uses MeTTa HE semantics: each `add_to_space` call increments the atom's multiplicity.
    pub fn add_to_space(&mut self, value: &V) {
        self.make_owned();

        // Check if this is a rule (= lhs rhs) — route through add_rule() which handles
        // BOTH MettaTrie insertion AND RuleIndex population (including bloom filter for "=").
        if let Some((lhs, rhs)) = extract_rule_parts(value) {
            self.add_rule(lhs, rhs);
            return;
        }

        // Check if this is a type assertion (: name type) or subtype declaration (:< sub super)
        // — also register in the types/subtypes HashMap for fast lookup.
        // Track whether this is a type or subtype atom for incremental MettaTrie updates.
        let mut is_type_atom = false;
        let mut is_subtype_atom = false;
        let mut type_atom_name: Option<String> = None;
        if let Some(items) = value.as_sexpr() {
            if items.len() == 3 {
                if let Some(op) = items[0].as_atom() {
                    match op {
                        ":" => {
                            if let Some(name) = items[1].as_atom() {
                                let typ = items[2].clone();
                                let mut types = self.shared.types.write();
                                let vec = types.entry(name.to_string()).or_default();
                                if !vec.contains(&typ) {
                                    vec.push(typ);
                                }
                                drop(types);
                                self.shared.type_index_dirty.store(true, Ordering::Release);
                                is_type_atom = true;
                                type_atom_name = Some(name.to_string());
                            }
                        }
                        ":<" => {
                            // Subtype declaration: (:< SubType SuperType)
                            if let (Some(sub), Some(sup)) = (items[1].as_atom(), items[2].as_atom()) {
                                let mut subtypes = self.shared.subtypes.write();
                                let vec = subtypes.entry(sub.to_string()).or_default();
                                if !vec.contains(&sup.to_string()) {
                                    vec.push(sup.to_string());
                                }
                                is_subtype_atom = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Decompose value to TrieKey sequence for literal storage
        let keys = decompose_literal(value);

        {
            let mut btm = self.shared.atom_space.btm.write();
            trie_add_atom(&mut btm, &keys, value.clone());
        }

        // Incrementally update type/subtype dedicated MettaTries + type bloom filter
        if is_type_atom {
            let mut type_btm = self.shared.atom_space.type_btm.write();
            trie_add_atom(&mut type_btm, &keys, value.clone());
            // Insert atom name into type bloom filter for O(1) early rejection
            if let Some(ref name) = type_atom_name {
                self.shared.atom_space.type_bloom.write().insert(name);
            }
        } else if is_subtype_atom {
            let mut subtype_btm = self.shared.atom_space.subtype_btm.write();
            trie_add_atom(&mut subtype_btm, &keys, value.clone());
        }

        self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

        // Use trait method for head symbol extraction
        if let Some(head) = value.get_head_symbol() {
            let arity = value.get_arity() as u8;
            self.shared.atom_space.head_arity_bloom.write().insert(head, arity);
        }
    }

    /// Remove a fact from MettaTrie Space by exact match.
    ///
    /// ## Unified Routing
    ///
    /// Automatically detects and routes special atom types:
    /// - Rules `(= lhs rhs)` → MettaTrie removal + RuleIndex sync
    /// - Type assertions `(: name type)` → MettaTrie removal + types HashMap removal
    /// - All other atoms → literal MettaTrie removal
    ///
    /// ## Multiplicity Tracking
    ///
    /// Decrements the atom's multiplicity. If multiplicity reaches 0, the atom is removed.
    pub fn remove_from_space(&mut self, value: &V) {
        self.make_owned();

        // Check if this is a type assertion (: name type) or subtype declaration (:< sub super)
        // — remove from the types/subtypes HashMap so queries stay consistent.
        // Track for incremental MettaTrie updates.
        let mut is_type_removal = false;
        let mut is_subtype_removal = false;
        if let Some(items) = value.as_sexpr() {
            if items.len() == 3 {
                if let Some(op) = items[0].as_atom() {
                    match op {
                        ":" => {
                            if let Some(name) = items[1].as_atom() {
                                let typ = &items[2];
                                let mut types = self.shared.types.write();
                                if let Some(vec) = types.get_mut(name) {
                                    vec.retain(|t| t != typ);
                                    if vec.is_empty() {
                                        types.remove(name);
                                    }
                                }
                                drop(types);
                                self.shared.type_index_dirty.store(true, Ordering::Release);
                                is_type_removal = true;
                            }
                        }
                        ":<" => {
                            // Remove subtype declaration: (:< SubType SuperType)
                            if let (Some(sub), Some(sup)) = (items[1].as_atom(), items[2].as_atom()) {
                                let mut subtypes = self.shared.subtypes.write();
                                if let Some(vec) = subtypes.get_mut(sub) {
                                    vec.retain(|s| s != sup);
                                    if vec.is_empty() {
                                        subtypes.remove(sub);
                                    }
                                }
                                is_subtype_removal = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Decompose value to TrieKey sequence for storage lookup
        let keys = decompose_literal(value);

        // Check if this is a rule (= lhs rhs)
        if let Some((lhs, rhs)) = extract_rule_parts(value) {
            // Rule removal: use literal encoding to match MettaTrie entry
            {
                let mut btm = self.shared.atom_space.btm.write();
                let current_count = trie_get_multiplicity(&btm, &keys);
                if current_count == 0 {
                    // Not found — nothing to remove
                    return;
                }

                let new_count = trie_remove_atom(&mut btm, &keys);

                if new_count == 0 {
                    self.shared.atom_space.head_arity_bloom.write().note_deletion();
                }
            }

            self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);

            // Sync RuleIndex: decrement or remove the rule entry
            self.shared.rule_index.write().remove_rule(&lhs, &rhs);
            return;
        }

        // Non-rule: use literal encoding
        {
            let mut btm = self.shared.atom_space.btm.write();
            let current_count = trie_get_multiplicity(&btm, &keys);
            if current_count == 0 {
                // Not found — nothing to remove
                return;
            }

            let new_count = trie_remove_atom(&mut btm, &keys);

            if new_count == 0 {
                self.shared.atom_space.head_arity_bloom.write().note_deletion();
            }

            drop(btm);

            // Incrementally update type/subtype dedicated MettaTries
            if is_type_removal {
                let mut type_btm = self.shared.atom_space.type_btm.write();
                trie_remove_atom(&mut type_btm, &keys);
                drop(type_btm);
                self.shared.atom_space.type_bloom.write().note_deletion();
            } else if is_subtype_removal {
                let mut subtype_btm = self.shared.atom_space.subtype_btm.write();
                trie_remove_atom(&mut subtype_btm, &keys);
            }
        }

        self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);

        // Invalidate eval memo and match result caches — removed rules/facts change evaluation results
        crate::backend::eval::trampoline::invalidate_normal_form_memo();
        crate::backend::eval::trampoline::clear_eval_memo();
        crate::backend::eval::trampoline::clear_match_result_cache();
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

    /// Add a fact to MettaTrie Space using interior mutability (no CoW copy).
    ///
    /// This method uses `&self` (not `&mut self`) and operates directly on the
    /// shared state via interior mutability. This is critical for arena mode
    /// where environments are cloned but should share space state updates.
    ///
    /// # Thread Safety
    ///
    /// Uses `RwLock::write()` for MettaTrie access and atomic operations for counters.
    /// Safe to call from multiple clones of the same environment.
    ///
    /// # When to Use
    ///
    /// - When you have multiple environment clones that should share space state
    /// - In loops where calling `add_to_space()` would trigger repeated CoW copies
    /// - In arena mode evaluation where state must persist across cloned environments
    pub fn add_to_space_shared(&self, value: &V) {
        // Decompose value to TrieKey sequence for literal storage
        let keys = decompose_literal(value);

        // Check if this is a rule (= lhs rhs) — rules must use literal encoding
        // to be consistent with add_rule() which stores in RuleIndex + MettaTrie.
        if extract_rule_parts(value).is_some() {
            {
                let mut btm = self.shared.atom_space.btm.write();
                trie_add_atom(&mut btm, &keys, value.clone());
            }

            self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

            if let Some(head) = value.get_head_symbol() {
                let arity = value.get_arity() as u8;
                self.shared.atom_space.head_arity_bloom.write().insert(head, arity);
            }

            self.mark_modified();
            return;
        }

        // Non-rule: use literal encoding
        {
            let mut btm = self.shared.atom_space.btm.write();
            trie_add_atom(&mut btm, &keys, value.clone());
        }

        self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

        // Use trait method for head symbol extraction
        if let Some(head) = value.get_head_symbol() {
            let arity = value.get_arity() as u8;
            self.shared.atom_space.head_arity_bloom.write().insert(head, arity);
        }

        // Mark as modified for union() fast-path detection
        self.mark_modified();
    }

    /// Remove a fact from MettaTrie Space using interior mutability (no CoW copy).
    ///
    /// This method uses `&self` (not `&mut self`) and operates directly on the
    /// shared state via interior mutability.
    ///
    /// # Thread Safety
    ///
    /// Uses `RwLock::write()` for MettaTrie access and atomic operations for counters.
    /// Safe to call from multiple clones of the same environment.
    pub fn remove_from_space_shared(&self, value: &V) {
        // Decompose value to TrieKey sequence for storage lookup
        let keys = decompose_literal(value);

        // Check if this is a rule (= lhs rhs)
        if let Some((lhs, rhs)) = extract_rule_parts(value) {
            {
                let mut btm = self.shared.atom_space.btm.write();
                let current_count = trie_get_multiplicity(&btm, &keys);
                if current_count == 0 {
                    return;
                }

                let new_count = trie_remove_atom(&mut btm, &keys);

                if new_count == 0 {
                    self.shared.atom_space.head_arity_bloom.write().note_deletion();
                }
            }

            self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
            self.shared.rule_index.write().remove_rule(&lhs, &rhs);
            self.mark_modified();
            return;
        }

        // Non-rule: use literal encoding
        {
            let mut btm = self.shared.atom_space.btm.write();
            let current_count = trie_get_multiplicity(&btm, &keys);
            if current_count == 0 {
                return;
            }

            let new_count = trie_remove_atom(&mut btm, &keys);

            if new_count == 0 {
                self.shared.atom_space.head_arity_bloom.write().note_deletion();
            }
        }

        self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);

        // Invalidate eval memo and match result caches — removed rules/facts change evaluation results
        crate::backend::eval::trampoline::invalidate_normal_form_memo();
        crate::backend::eval::trampoline::clear_eval_memo();
        crate::backend::eval::trampoline::clear_match_result_cache();

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
    /// MettaTrie stores original expressions at leaves — iteration yields `(&V, &Multiplicity)`
    /// directly with no deserialization. Pattern matching and template instantiation operate
    /// directly on V with no intermediate conversions.
    ///
    /// Check if there may be rules with the given head symbol and arity.
    /// Uses bloom filter: O(1), no false negatives. False positives cause
    /// harmless extra evaluation (data constructors evaluate to themselves).
    #[inline]
    pub fn may_have_rules_for(&self, head: &str, arity: usize) -> bool {
        self.shared
            .atom_space
            .head_arity_bloom
            .read()
            .may_contain(head, arity as u8)
    }

    pub fn match_space(&self, pattern: &V, template: &V) -> Vec<MultiplicityMatch<V>> {
        // Bloom filter check using trait methods (no conversion)
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self.shared.atom_space.head_arity_bloom.read()
                .may_contain(expected_head, pattern_arity);
            if !bloom_result {
                return Vec::new();
            }
        }

        let mut results = Vec::new();

        // Query MettaTrie with decomposed pattern keys (Variable positions act as wildcards)
        let pattern_keys = decompose(pattern);
        let btm = self.shared.atom_space.btm.read();
        for qm in btm.query(&pattern_keys) {
            // Freshen stored expression variables to prevent capture (MeTTa HE semantics).
            // Without this, variables like $x in different matched rules collide,
            // causing incorrect unification and infinite loops in PLN.
            let freshened = if qm.expr.contains_variables() {
                crate::backend::eval::freshening::freshen_variables_generic(&qm.expr, &self.factory)
            } else {
                qm.expr.clone()
            };
            if let Some(bindings) = pattern_match_generic(pattern, &freshened) {
                let instantiated = apply_bindings_generic(template, &bindings, &self.factory);
                results.push(MultiplicityMatch::new(instantiated, qm.value.count() as usize));
            }
        }

        // Also check variable atoms (bidirectional matching with freshening)
        {
            let var_atoms = self.shared.atom_space.variable_atoms.read();
            for (atom, multiplicity) in var_atoms.iter() {
                let freshened = crate::backend::eval::freshening::freshen_variables_generic(atom, &self.factory);
                if let Some(bindings) = pattern_match_generic(pattern, &freshened) {
                    let instantiated = apply_bindings_generic(template, &bindings, &self.factory);
                    results.push(MultiplicityMatch::new(instantiated, *multiplicity));
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
    /// MettaTrie stores original expressions at leaves — query yields them directly.
    /// Pattern matching operates directly on V with no conversion.
    pub fn match_space_exists(&self, pattern: &V) -> bool {
        // Bloom filter check using trait methods (no conversion)
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self.shared.atom_space.head_arity_bloom.read()
                .may_contain(expected_head, pattern_arity)
            {
                return false;
            }
        }

        // Query MettaTrie with decomposed pattern keys
        let pattern_keys = decompose(pattern);
        let btm = self.shared.atom_space.btm.read();
        for qm in btm.query(&pattern_keys) {
            if pattern_match_generic(pattern, &qm.expr).is_some() {
                return true;
            }
        }

        // Check variable atoms (bidirectional matching)
        {
            let var_atoms = self.shared.atom_space.variable_atoms.read();
            for (atom, _multiplicity) in var_atoms.iter() {
                if pattern_match_generic(pattern, atom).is_some() {
                    return true;
                }
            }
        }

        false
    }

    /// Get all atoms from the Space (MettaTrie iteration).
    ///
    /// This iterates the same data as `match_space()` but without pattern filtering,
    /// returning every stored atom as-is.
    ///
    /// ## Zero-Conversion Architecture
    ///
    /// MettaTrie stores original expressions at leaves — iteration yields `(&V, &Multiplicity)`
    /// directly with no deserialization.
    pub fn get_all_atoms(&self) -> Vec<V> {
        let btm = self.shared.atom_space.btm.read();
        let mut atoms = Vec::with_capacity(btm.val_count());

        // Iterate through MettaTrie — yields (&V, &Multiplicity) directly
        for (expr, _mult) in btm.iter() {
            atoms.push(expr.clone());
        }

        // Include variable atoms
        {
            let var_atoms = self.shared.atom_space.variable_atoms.read();
            for (atom, _multiplicity) in var_atoms.iter() {
                atoms.push(atom.clone());
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
    fn test_match_space_finds_rules_added_via_add_rule() {
        // Regression test: add_rule() must insert ("=", 3) into the bloom filter
        // so that match_space() queries with pattern (= $a $b) are not rejected.
        let mut env: MettaEnvironment = MettaEnvironment::default();

        // Add rule: (= (f $x) (g $x))
        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom("f".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let rhs = MettaValue::SExpr(vec![
            MettaValue::Atom("g".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        env.add_rule(lhs, rhs);

        // Query: match_space with pattern (= $a $b) should find the rule
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("$b".to_string()),
        ]);
        let template = MettaValue::SExpr(vec![
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("$b".to_string()),
        ]);
        let results = env.match_space(&pattern, &template);

        assert!(
            !results.is_empty(),
            "match_space should find rules stored via add_rule; bloom filter must include (\"=\", 3)"
        );
    }

    #[test]
    fn test_match_space_freshens_variables() {
        // Regression test: match_space must freshen variables in stored expressions
        // to prevent variable capture across different matched rules.
        let mut env: MettaEnvironment = MettaEnvironment::default();

        // Add two rules with the same variable name $x
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("f".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("g".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        );
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("h".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("k".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        );

        // Query all rules — match_space should return freshened variables
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("$b".to_string()),
        ]);
        let template = MettaValue::Atom("$b".to_string());
        let results = env.match_space(&pattern, &template);

        // The two results should have DIFFERENT variable names (freshened)
        assert_eq!(results.len(), 2);
        let r0 = &results[0].value;
        let r1 = &results[1].value;
        // Variables in r0 and r1 should NOT be the same $x — they should be freshened
        assert_ne!(r0, r1, "Freshening should make variables unique across matches");
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
