//! Environment module for MeTTa evaluation.
//!
//! The Environment contains the fact database, type assertions, rules, and
//! various registries for MeTTa evaluation. Uses MORK PathMap for efficient
//! trie-based storage with pattern matching support.
//!
//! # Architecture
//!
//! - `Environment` - Main struct with Copy-on-Write (CoW) semantics
//! - `EnvironmentShared` - Consolidated shared state (single Arc instead of 17)
//! - `HeadArityBloomFilter` - O(1) rejection for match_space()
//! - `ScopeTracker` - Hierarchical scope tracking for "Did you mean?" suggestions
//!
//! # Thread Safety
//!
//! All shared state uses RwLock for concurrent read/exclusive write access.
//! Clone operations are O(1) via Arc sharing until first mutation.

mod bloom;
mod fact_storage;
mod grounded_ops;
mod module_ops;
mod mork_encoding;
mod mutable_state;
mod named_spaces;
mod pattern_matching;
mod rule_management;
mod scope;
mod scope_ops;
mod suggestions;
mod symbol_bindings;
#[cfg(test)]
mod tests;
mod type_system;

pub(crate) use bloom::HeadArityBloomFilter;
pub use scope::ScopeTracker;

use lru::LruCache;
use mork::space::Space;
use mork_interning::SharedMappingHandle;
use pathmap::PathMap;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tracing::trace;

use super::fuzzy_match::FuzzyMatcher;
use super::grounded::{GroundedRegistry, GroundedRegistryTCO};
use super::modules::{ModuleRegistry, Tokenizer};
use super::symbol::Symbol;
use super::{MettaValue, Rule};

/// Shared state across all Environment clones.
/// Consolidates 17 Arc<RwLock<T>> fields into a single Arc<EnvironmentShared>
/// for O(1) clone operations (1 atomic increment instead of 17).
///
/// Thread-safe: All fields use RwLock for concurrent read/exclusive write access.
pub(crate) struct EnvironmentShared {
    /// PathMap trie for fact storage
    pub(crate) btm: RwLock<PathMap<()>>,

    /// Rule index: Maps (head_symbol, arity) -> Vec<Rule> for O(1) rule lookup
    /// Uses Symbol for O(1) comparison when symbol-interning feature is enabled
    #[allow(clippy::type_complexity)]
    pub(crate) rule_index: RwLock<HashMap<(Symbol, usize), Vec<Rule>>>,

    /// Wildcard rules: Rules without a clear head symbol
    pub(crate) wildcard_rules: RwLock<Vec<Rule>>,

    /// Fast flag: true if any wildcard rules exist (avoids lock acquisition when empty)
    pub(crate) has_wildcard_rules: AtomicBool,

    /// Multiplicities: tracks how many times each rule is defined
    pub(crate) multiplicities: RwLock<HashMap<String, usize>>,

    /// Pattern cache: LRU cache for MORK serialization results
    pub(crate) pattern_cache: RwLock<LruCache<MettaValue, Vec<u8>>>,

    /// Type index: Lazy-initialized subtrie containing only type assertions
    pub(crate) type_index: RwLock<Option<PathMap<()>>>,

    /// Type index invalidation flag
    pub(crate) type_index_dirty: RwLock<bool>,

    /// Named spaces registry: Maps space_id -> (name, atoms)
    #[allow(clippy::type_complexity)]
    pub(crate) named_spaces: RwLock<HashMap<u64, (String, Vec<MettaValue>)>>,

    /// Counter for generating unique space IDs
    pub(crate) next_space_id: RwLock<u64>,

    /// Mutable state cells registry
    pub(crate) states: RwLock<HashMap<u64, MettaValue>>,

    /// Counter for generating unique state IDs
    pub(crate) next_state_id: RwLock<u64>,

    /// Symbol bindings registry
    pub(crate) bindings: RwLock<HashMap<String, MettaValue>>,

    /// Module registry
    pub(crate) module_registry: RwLock<ModuleRegistry>,

    /// Per-module tokenizer
    pub(crate) tokenizer: RwLock<Tokenizer>,

    /// Grounded operations registry (legacy)
    pub(crate) grounded_registry: RwLock<GroundedRegistry>,

    /// TCO-compatible grounded operations registry
    pub(crate) grounded_registry_tco: RwLock<GroundedRegistryTCO>,

    /// Fallback store for large expressions
    pub(crate) large_expr_pathmap: RwLock<Option<PathMap<MettaValue>>>,

    /// Fuzzy matcher for "Did you mean?" suggestions
    pub(crate) fuzzy_matcher: RwLock<FuzzyMatcher>,

    /// Hierarchical scope tracker for context-aware symbol resolution
    pub(crate) scope_tracker: RwLock<ScopeTracker>,

    /// Bloom filter for (head_symbol, arity) pairs - enables O(1) match_space() rejection
    /// when the pattern's (head, arity) definitely doesn't exist in the space.
    pub(crate) head_arity_bloom: RwLock<HeadArityBloomFilter>,
}

/// The environment contains the fact database and type assertions
/// All facts (rules, atoms, s-expressions, type assertions) are stored in MORK PathMap
///
/// Thread-safe with Copy-on-Write (CoW) semantics:
/// - Clones share data until first modification (owns_data = false)
/// - First mutation triggers deep copy via make_owned() (owns_data = true)
/// - RwLock enables concurrent reads (4× improvement over Mutex)
/// - Modifications tracked via Arc<AtomicBool> for fast union() paths
///
/// Performance optimization: All shared state is consolidated into a single
/// Arc<EnvironmentShared> for O(1) clone operations (1 atomic increment instead of 17).
pub struct Environment {
    /// Consolidated shared state - single Arc for O(1) cloning
    /// Contains all RwLock-wrapped fields that can be shared across clones
    pub(crate) shared: Arc<EnvironmentShared>,

    /// THREAD-SAFE: SharedMappingHandle for symbol interning (string → u64)
    /// Can be cloned and shared across threads (Send + Sync)
    /// Kept separate as it has its own sharing semantics
    pub(crate) shared_mapping: SharedMappingHandle,

    /// CoW: Tracks if this clone owns its data (true = can modify in-place, false = must deep copy first)
    /// Set to true on new(), false on clone(), true after make_owned()
    pub(crate) owns_data: bool,

    /// CoW: Tracks if this environment has been modified since creation/clone
    /// Used for fast-path union() optimization (unmodified clones can skip deep merge)
    /// Arc-wrapped to allow independent tracking per clone
    pub(crate) modified: Arc<AtomicBool>,

    /// Current module path: Directory of the currently-executing module
    /// Used for relative path resolution (self:child notation)
    /// None when not inside a module evaluation
    /// Kept separate as it's per-clone state
    pub(crate) current_module_path: Option<PathBuf>,
}

impl Environment {
    pub fn new() -> Self {
        use mork_interning::SharedMapping;

        let shared = Arc::new(EnvironmentShared {
            btm: RwLock::new(PathMap::new()),
            rule_index: RwLock::new(HashMap::with_capacity(128)),
            wildcard_rules: RwLock::new(Vec::new()),
            has_wildcard_rules: AtomicBool::new(false),
            multiplicities: RwLock::new(HashMap::new()),
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(None),
            type_index_dirty: RwLock::new(true),
            named_spaces: RwLock::new(HashMap::new()),
            next_space_id: RwLock::new(1), // Start from 1, 0 reserved for self
            states: RwLock::new(HashMap::new()),
            next_state_id: RwLock::new(1), // Start from 1
            bindings: RwLock::new(HashMap::new()),
            module_registry: RwLock::new(ModuleRegistry::new()),
            tokenizer: RwLock::new(Tokenizer::new()),
            grounded_registry: RwLock::new(GroundedRegistry::with_standard_ops()),
            grounded_registry_tco: RwLock::new(GroundedRegistryTCO::with_standard_ops()),
            large_expr_pathmap: RwLock::new(None), // Lazy: not allocated until needed
            fuzzy_matcher: RwLock::new(FuzzyMatcher::new()),
            scope_tracker: RwLock::new(ScopeTracker::new()),
            head_arity_bloom: RwLock::new(HeadArityBloomFilter::new(10000)), // ~10KB for 10k expected entries
        });

        Environment {
            shared,
            shared_mapping: SharedMapping::new(),
            owns_data: true, // CoW: new environments own their data
            modified: Arc::new(AtomicBool::new(false)), // CoW: track modifications
            current_module_path: None,
        }
    }

    /// CoW: Make this environment own its data (deep copy if sharing)
    /// Called automatically on first mutation of a cloned environment
    /// No-op if already owns data (owns_data == true)
    fn make_owned(&mut self) {
        // Fast path: already own data
        if self.owns_data {
            return;
        }
        trace!(target: "mettatron::environment::make_owned", "Deep copying CoW data");

        // Deep copy the entire shared state structure
        // Clone the data first to avoid borrowing issues
        let new_shared = Arc::new(EnvironmentShared {
            btm: RwLock::new(self.shared.btm.read().expect("btm lock poisoned").clone()),
            rule_index: RwLock::new(
                self.shared
                    .rule_index
                    .read()
                    .expect("rule_index lock poisoned")
                    .clone(),
            ),
            wildcard_rules: RwLock::new(
                self.shared
                    .wildcard_rules
                    .read()
                    .expect("wildcard_rules lock poisoned")
                    .clone(),
            ),
            has_wildcard_rules: AtomicBool::new(
                self.shared.has_wildcard_rules.load(Ordering::Acquire),
            ),
            multiplicities: RwLock::new(
                self.shared
                    .multiplicities
                    .read()
                    .expect("multiplicities lock poisoned")
                    .clone(),
            ),
            pattern_cache: RwLock::new(
                self.shared
                    .pattern_cache
                    .read()
                    .expect("pattern_cache lock poisoned")
                    .clone(),
            ),
            type_index: RwLock::new(
                self.shared
                    .type_index
                    .read()
                    .expect("type_index lock poisoned")
                    .clone(),
            ),
            type_index_dirty: RwLock::new(
                *self
                    .shared
                    .type_index_dirty
                    .read()
                    .expect("type_index_dirty lock poisoned"),
            ),
            named_spaces: RwLock::new(
                self.shared
                    .named_spaces
                    .read()
                    .expect("named_spaces lock poisoned")
                    .clone(),
            ),
            next_space_id: RwLock::new(
                *self
                    .shared
                    .next_space_id
                    .read()
                    .expect("next_space_id lock poisoned"),
            ),
            states: RwLock::new(
                self.shared
                    .states
                    .read()
                    .expect("states lock poisoned")
                    .clone(),
            ),
            next_state_id: RwLock::new(
                *self
                    .shared
                    .next_state_id
                    .read()
                    .expect("next_state_id lock poisoned"),
            ),
            bindings: RwLock::new(
                self.shared
                    .bindings
                    .read()
                    .expect("bindings lock poisoned")
                    .clone(),
            ),
            module_registry: RwLock::new(
                self.shared
                    .module_registry
                    .read()
                    .expect("module_registry lock poisoned")
                    .clone(),
            ),
            tokenizer: RwLock::new(
                self.shared
                    .tokenizer
                    .read()
                    .expect("tokenizer lock poisoned")
                    .clone(),
            ),
            grounded_registry: RwLock::new(
                self.shared
                    .grounded_registry
                    .read()
                    .expect("grounded_registry lock poisoned")
                    .clone(),
            ),
            grounded_registry_tco: RwLock::new(
                self.shared
                    .grounded_registry_tco
                    .read()
                    .expect("grounded_registry_tco lock poisoned")
                    .clone(),
            ),
            large_expr_pathmap: RwLock::new(
                self.shared
                    .large_expr_pathmap
                    .read()
                    .expect("large_expr_pathmap lock poisoned")
                    .clone(),
            ),
            fuzzy_matcher: RwLock::new(
                self.shared
                    .fuzzy_matcher
                    .read()
                    .expect("fuzzy_matcher lock poisoned")
                    .clone(),
            ),
            scope_tracker: RwLock::new(
                self.shared
                    .scope_tracker
                    .read()
                    .expect("scope_tracker lock poisoned")
                    .clone(),
            ),
            head_arity_bloom: RwLock::new(
                self.shared
                    .head_arity_bloom
                    .read()
                    .expect("head_arity_bloom lock poisoned")
                    .clone(),
            ),
        });

        self.shared = new_shared;

        // Mark as owning data and modified
        self.owns_data = true;
        self.modified.store(true, Ordering::Release);
    }

    /// Create a forked environment for nondeterministic branch isolation.
    ///
    /// This is critical for correct evaluation of nondeterministic MeTTa programs.
    /// When evaluation forks (e.g., from `match` returning multiple results),
    /// each branch needs its own isolated view of mutable state.
    ///
    /// This method:
    /// 1. Clones the environment (CoW for states)
    /// 2. Forks all SpaceHandles in bindings for branch isolation
    ///
    /// # Example
    /// ```ignore
    /// // Original env has &stack bound to a space
    /// let forked = env.fork_for_nondeterminism();
    /// // forked's &stack is isolated from original's
    /// ```
    pub fn fork_for_nondeterminism(&self) -> Environment {
        let mut forked = self.clone();
        forked.make_owned(); // Ensure we have our own copy of state

        // Fork all SpaceHandles in bindings
        let mut forked_bindings = forked
            .shared
            .bindings
            .write()
            .expect("bindings lock poisoned");
        for (_name, value) in forked_bindings.iter_mut() {
            Self::fork_spaces_in_value(value);
        }
        drop(forked_bindings);

        forked
    }

    /// Recursively fork all SpaceHandles in a MettaValue.
    fn fork_spaces_in_value(value: &mut MettaValue) {
        match value {
            MettaValue::Space(handle) => {
                // Fork the space handle for isolation
                *handle = handle.fork();
            }
            MettaValue::SExpr(items) => {
                for item in items.iter_mut() {
                    Self::fork_spaces_in_value(item);
                }
            }
            MettaValue::Conjunction(goals) => {
                for goal in goals.iter_mut() {
                    Self::fork_spaces_in_value(goal);
                }
            }
            MettaValue::Type(_) => {
                // Arc<MettaValue> - can't mutate through Arc, but types rarely contain spaces
            }
            MettaValue::Error(_, _) => {
                // Arc<MettaValue> - can't mutate, but errors rarely contain spaces
            }
            // Primitives don't contain spaces
            MettaValue::Atom(_)
            | MettaValue::Bool(_)
            | MettaValue::Long(_)
            | MettaValue::Float(_)
            | MettaValue::String(_)
            | MettaValue::Nil
            | MettaValue::State(_)
            | MettaValue::Unit
            | MettaValue::Memo(_)
            | MettaValue::Empty => {}
        }
    }

    /// Create a thread-local Space for operations
    /// Following the Rholang LSP pattern: cheap clone via structural sharing
    ///
    /// This is useful for advanced operations that need direct access to the Space,
    /// such as debugging or custom MORK queries.
    pub fn create_space(&self) -> Space {
        let btm = self.shared.btm.read().expect("btm lock poisoned").clone(); // CoW: read lock for concurrent reads
        Space {
            btm,
            sm: self.shared_mapping.clone(),
            mmaps: HashMap::new(),
        }
    }

    /// Update PathMap and shared mapping after Space modifications (write operations)
    /// This updates both the PathMap (btm) and the SharedMappingHandle (sm)
    pub(crate) fn update_pathmap(&mut self, space: Space) {
        self.make_owned(); // CoW: ensure we own data before modifying
        *self.shared.btm.write().expect("btm lock poisoned") = space.btm; // CoW: write lock for exclusive access
        self.shared_mapping = space.sm;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Union two environments (monotonic merge)
    /// PathMap and shared_mapping are shared via Arc, so facts (including type assertions) are automatically merged
    /// Multiplicities and rule indices are also merged via shared Arc
    pub fn union(&self, _other: &Environment) -> Environment {
        trace!(target: "mettatron::environment::union", "Unioning environments");

        // All shared state is now consolidated into single Arc<EnvironmentShared>
        // Clone is O(1) - just one atomic increment instead of 17
        Environment {
            shared: Arc::clone(&self.shared),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false, // CoW: union creates a new shared environment
            modified: Arc::new(AtomicBool::new(false)), // CoW: fresh modification tracker
            current_module_path: self.current_module_path.clone(),
        }
    }
}

/// CoW: Manual Clone implementation
/// Clones share data (owns_data = false) until first modification triggers make_owned()
///
/// Performance: O(1) clone via single Arc increment instead of 17 separate Arc clones
impl Clone for Environment {
    fn clone(&self) -> Self {
        Environment {
            // O(1): Single atomic increment for all shared state
            shared: Arc::clone(&self.shared),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false, // CoW: clones do not own data initially
            modified: Arc::new(AtomicBool::new(false)), // CoW: fresh modification tracker
            current_module_path: self.current_module_path.clone(),
        }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("space", &"<MORK Space>")
            .finish()
    }
}
