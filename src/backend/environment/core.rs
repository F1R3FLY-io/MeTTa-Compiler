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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use lru::LruCache;
use mork::space::Space;
use mork_interning::{SharedMapping, SharedMappingHandle};
use parking_lot::RwLock;
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
use pathmap::PathMap;
use tracing::trace;

use super::bloom::HeadArityBloomFilter;
use super::mork_encoding::mork_bytes_to_generic_value;
use super::multiplicity::{add_atom, get_multiplicity, remove_atom, Multiplicity};
use super::rule_management::extract_rule_parts;
use super::scope::ScopeTracker;
use crate::backend::eval::bindings::{apply_bindings_generic, pattern_match_generic};
use crate::backend::fuzzy_match::FuzzyMatcher;

#[inline]
fn invalidate_space_mutation_caches() {
    crate::backend::eval::trampoline::invalidate_normal_form_memo();
    crate::backend::eval::trampoline::clear_eval_memo();
    crate::backend::eval::trampoline::clear_match_result_cache();
}

/// Recursively strip all `Lazy(...)` wrappers from a value, peeling through
/// SExpr children. PT-canonical Lazy is invisible to display/hash/MORK; this
/// helper produces a structurally clean form for rule storage where the
/// "rule-inhibitor" Lazy semantic must not persist.
fn deep_unwrap_lazy<V, F>(value: &V, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let peeled = value.unwrap_lazy();
    if let Some(items) = peeled.as_sexpr() {
        let mut any_changed = false;
        let mut new_items = Vec::with_capacity(items.len());
        for child in items.iter() {
            let new_child = deep_unwrap_lazy(child, factory);
            if !any_changed
                && !std::ptr::eq(
                    child as *const _ as *const (),
                    &new_child as *const _ as *const (),
                )
            {
                any_changed = true;
            }
            new_items.push(new_child);
        }
        let _ = any_changed; // Always rebuild for safety; ptr-eq check is heuristic
        factory.sexpr(new_items)
    } else {
        peeled
    }
}
use crate::backend::grounded::GroundedRegistry;
use crate::backend::hash_utils::IdentityU64BuildHasher;
use crate::backend::models::gc_allocator::try_register_env_roots;
#[cfg(not(feature = "index-gc"))]
use crate::backend::models::gc_allocator::RootProvider;
use crate::backend::models::{MettaValue, MettaValueFactory, MettaValueTrait, SpaceHandle};
use crate::backend::modules::ModuleRegistry;
use crate::backend::mork_convert::{with_mork_bytes, with_mork_query_bytes};
use crate::backend::wide_mork::decode::wide_bytes_to_generic_value;
use crate::backend::wide_mork::encoding::encode_wide_storage;

// ============================================================================
// Static Sentinel for Unmodified Environments
// ============================================================================

// ============================================================================
// Pragma Settings
// ============================================================================

/// Type-check mode controlling whether type errors at call sites are emitted.
///
/// HE parity (`hyperon-experimental/lib/src/metta/runner/stdlib/core.rs`):
/// `(pragma! type-check auto)` enables strict checking; the default `permissive`
/// mode emits type errors only when both the function head and the argument
/// have determinable concrete types.
///
/// S-step (2026-05-16): added for type-error emission at call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCheckMode {
    /// Default. Type errors are emitted only when the head has a declared
    /// `(-> ...)` type and arg types can be inferred concretely (no
    /// `%Undefined%`, no free variables).
    Permissive,
    /// Strict. Type errors fire whenever the declared head type contradicts
    /// the inferred argument type, even when the arg type is `%Undefined%`.
    Auto,
}

impl Default for TypeCheckMode {
    fn default() -> Self {
        TypeCheckMode::Permissive
    }
}

/// Controls how nondeterministic rule dispatch resolves when multiple rules
/// match the same call site.
///
/// HE has no specificity filter — all matching rules fire nondeterministically.
/// MeTTaTron exposes a SUPERSET opt-in: when `Specificity` is engaged, only
/// the most-structurally-specific rules in the match set fire. This makes
/// programs with overlapping rule patterns (e.g., naive Fibonacci with base
/// cases + variable-pattern recursive case) terminate cleanly.
///
/// **Score function** (see `rule_management.rs::lhs_specificity`):
/// constructor-depth-weighted with NewVar penalty. Constructor atoms / literals
/// / S-expr heads contribute `W_CONSTRUCTOR * (1 + depth)`; first occurrences
/// of variables contribute 0; repeat-var occurrences contribute `W_REPEAT_VAR`.
/// Among matching candidates, the maximum score wins; ties keep all (graceful
/// degradation to HE nondet for genuinely incomparable patterns).
///
/// Engaged via `(pragma! rule-fire-mode specificity)` or `--rule-fire-mode=specificity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuleFireMode {
    /// HE-bisim default. All matching rules fire nondeterministically.
    #[default]
    Nondet,
    /// MTT superset (opt-in). Retain only candidates whose LHS specificity
    /// score equals the maximum among matches.
    Specificity,
}

/// Per-environment pragma settings stored on `GenericEnvironmentShared`.
///
/// All known settings live here. Unknown settings are stored as raw
/// `(key, value)` pairs in `other` (for HE-bisim — pragma key validation
/// happens in the `pragma!` arm and unknown keys are accepted silently).
#[derive(Debug, Clone)]
pub struct PragmaSettings {
    /// Controls call-site type checking. Default: `Permissive`.
    pub type_check_mode: TypeCheckMode,
    /// Controls multi-rule-fire dispatch policy. Default: `Nondet` (HE-bisim).
    /// Set via `(pragma! rule-fire-mode specificity)` for the SUPERSET filter.
    pub rule_fire_mode: RuleFireMode,
    /// Maximum eval-loop depth before emitting `(Error <form> StackOverflow)`.
    /// `Some(N)` caps; `None` means unlimited. Set via
    /// `(pragma! max-stack-depth N)`. HE-bisim §06.4.6, T04/047 verifies.
    ///
    /// Default: `Some(1000)` (MTT-extension): a finite cap is required to
    /// terminate non-deterministic-overlapping recursion such as the
    /// metta-spec T07/003-factorial fixture
    /// `(= (fac 0) 1) (= (fac $n) (* $n (fac (- $n 1))))` where the recursive
    /// rule also matches `(fac 0)` and recurses through negative N without
    /// a base case. HE itself returns `[120]` for this fixture only when the
    /// user sets a non-zero cap (e.g. `(pragma! max-stack-depth 200)` per
    /// `hyperon-experimental/lib/src/metta/runner/stdlib/core.rs:471`); HE
    /// hangs identically with the default `0`. MTT chooses `Some(1000)` so
    /// these fixtures pass out of the box while still allowing
    /// `(pragma! max-stack-depth 0)` to opt into HE-style unlimited.
    pub max_stack_depth: Option<usize>,
    /// Other pragma key/value pairs (no semantic effect, but stored for
    /// observability and future use).
    pub other: HashMap<String, String>,
    /// Tracks pragma keys that have been EXPLICITLY user-set via
    /// `(pragma! key value)`. HE-bisim §9.8.2: `(pragma! key)` 1-arg read
    /// returns the documented default OR `NotReducible` when no user
    /// setting exists. MTT carries a non-trivial default (e.g.,
    /// `max-stack-depth=Some(1000)`) but reports `NotReducible` until the
    /// user opts in via the write form, matching HE T07/051.
    pub user_set_keys: std::collections::HashSet<String>,
}

impl Default for PragmaSettings {
    fn default() -> Self {
        Self {
            type_check_mode: TypeCheckMode::default(),
            rule_fire_mode: RuleFireMode::default(),
            max_stack_depth: Some(1000),
            other: HashMap::new(),
            user_set_keys: std::collections::HashSet::new(),
        }
    }
}

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
    // Unified Atom Storage (AtomSpace)
    // ========================================================================
    /// Unified atom storage: MORK PathMap (ground atoms) + variable atom Vec.
    /// Contains btm, wide_btm, shared_mapping, head_arity_bloom,
    /// total_atoms, and variable_atoms.
    // Gap A (2026-05-26, PeTTa global atomspace): the `&self` atom store is a
    // single GLOBALLY-SHARED `Arc<AtomSpace>`, mirroring `rule_index` (which is
    // already `Arc::clone`d across nondeterministic branches). `fork_for_
    // nondeterminism` and `make_owned` `Arc::clone` it (not deep-copy), so a
    // side-effecting `add-atom`/`remove-atom` in any match/superpose branch is
    // globally visible and commits to the directive — while BINDINGS/states stay
    // per-branch COW-forked (isolation preserved). AtomSpace is fully
    // interior-mutable (RwLock/atomic/Arc fields), so `&self` mutation through
    // the shared Arc is sound.
    pub(crate) atom_space: std::sync::Arc<super::atom_space::AtomSpace<V>>,

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

    /// Grounded operations registry (type-parameterized, zero-conversion)
    /// Stateless and Clone, no lock needed
    pub(crate) grounded_registry: GroundedRegistry,

    /// Pattern cache for MORK serialization (keyed by MettaValue for heap mode)
    /// Uses RwLock (LruCache requires exclusive access for get/put)
    pub(crate) pattern_cache: RwLock<LruCache<MettaValue, Vec<u8>>>,

    /// Type index: Lazy-initialized subtrie containing only type assertions
    /// Uses RwLock (PathMap has no concurrent alternative)
    pub(crate) type_index: RwLock<Option<PathMap<Multiplicity>>>,

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

    /// In-memory rule index for O(1) lookup + MORK byte-level matching.
    /// Populated at `add_rule()` time. Authoritative for rule queries.
    /// PathMap remains the storage-of-record (for match_space, serialization).
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
    pub(crate) inferred_fn_types: DashMap<String, Vec<V>>,

    /// Per-environment override bitset for grounded helpers that user rules
    /// may shadow (HE-compat helpers like `append`, `length`, `map-atom`,
    /// `car-atom`, …). Lock-free atomics — read at dispatch time via a
    /// single relaxed load + bit test.
    ///
    /// See `super::dispatch_overrides` for the design rationale and the
    /// full list of overridable names.
    /// 2026-04-23: Arc-wrapped so `make_owned()` / `fork_for_nondeterminism()`
    /// share the same atomic overridden-bits state. Previously a bare struct
    /// whose `.snapshot()` produced an INDEPENDENT copy — a user rule
    /// `(= (car-atom $list) …)` noted on one env was invisible to the later
    /// dispatch check on another env (verified via debug-print Arc-pointer
    /// addresses showing two distinct instances). Map-atom's test happened
    /// to survive the split; car-atom's did not.
    pub(crate) dispatch_overrides: Arc<super::dispatch_overrides::DispatchOverrides>,

    /// Per-environment pragma settings (S-step 2026-05-16).
    ///
    /// Updated by the `pragma!` special form. Read by call-site type
    /// checking (`type_check_mode` controls strict vs permissive checking).
    /// Arc-wrapped + RwLock so forks share the same mutable state. The
    /// `pragma!` arm makes a `make_owned()` clone implicitly when it writes;
    /// reads are lock-free `Acquire` loads on the inner atomic.
    pub(crate) pragma_settings: Arc<RwLock<PragmaSettings>>,
    // Plan Phase F (2026-05-20): the `corelib_mod` field has been removed.
    // MeTTaTron's corelib is now entirely native Rust:
    //   - Built-in helpers (if-decons-expr, if-error, return-on-error,
    //     assertIncludes, noreduce-eq) dispatch at the `'special_forms`
    //     arm in `eval/step/sexpr.rs` before rule lookup.
    //   - Built-in type decls (ErrorDescription, BadType, BadArgType,
    //     IncorrectNumberOfArguments) are registered via
    //     `MettaEnvironment::register_corelib_types()` at `new_env()` time.
    // No MeTTa source file is involved. See [[corelib-native-port]].
}

/// Byte length of the MORK-serialized rule prefix: `[Arity(3)] + [SymbolSize(8)] + [8 symbol ID bytes]`.
///
/// This is structurally constant for all environments — MORK always encodes symbols as 8-byte IDs
/// with a 1-byte size tag, and rules always have arity 3 for `(= lhs rhs)`.
///
/// Previously stored as a precomputed `Arc<[u8]>` field on `GenericEnvironment`, but the actual
/// byte content could become stale when thread-local MORK symbol caches were invalidated across
/// different `SharedMapping` epochs on the same thread. Since only the *length* is needed for
/// splitting De Bruijn bytes into LHS/RHS ranges, a compile-time constant eliminates the
/// stale-prefix problem entirely.
pub(crate) const RULE_PREFIX_LEN: usize = 10;

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

    /// Current module path for relative path resolution.
    /// Arc-wrapped for O(1) clone — module path is semantically immutable after construction.
    pub(crate) current_module_path: Option<Arc<PathBuf>>,

    /// Monotonic epoch for MORK symbol cache invalidation.
    ///
    /// Assigned from `next_mork_epoch()` at construction. Environments that share
    /// the same `SharedMapping` (clones, forks) keep the same epoch, since cached
    /// symbol IDs remain valid. A new epoch is only allocated for a truly new
    /// `SharedMapping` (i.e., `GenericEnvironment::new()`).
    ///
    /// Unlike pointer-based identity, epochs are never reused — this eliminates
    /// the ABA problem where a dropped `SharedMapping` has its heap address
    /// recycled by a new allocation.
    pub(crate) mork_cache_epoch: u64,

    /// HE runner-mode flag — Plan S0c (2026-05-13).
    ///
    /// `false` (default) = HE `MettaRunnerMode::ADD` — bare top-level S-exprs
    /// are silent side-effecting facts; the runner emits `[]` per directive.
    ///
    /// `true` = HE `MettaRunnerMode::INTERPRET` — set when a `(! expr)`
    /// directive is dispatched; the reduction's results flow into the
    /// observable output multiset. Auto-cleared after the directive completes.
    ///
    /// See `hyperon-experimental/lib/src/metta/runner/mod.rs:1076-1109`
    /// (`MettaRunnerMode { ADD, INTERPRET, TERMINATE }`).
    /// Threaded through to `BytecodeVM::interpret_mode` and
    /// `JitContext::interpret_mode` for tier-locality.
    pub(crate) interpret_mode: bool,

    /// S2 BANG-WORD / decl-atom dispatch (2026-05-13): true when we are
    /// CURRENTLY EVALUATING THE BODY of a `(! expr)` directive (the `!` arm
    /// at `eval/step/sexpr.rs` sets it, eval() resets it per directive).
    ///
    /// Distinct from `interpret_mode` because `interpret_mode` is the
    /// runner-mode coalesce flag (true even for programmatic `eval()` so
    /// bare top-level expressions return values rather than being swallowed
    /// by the ADD-mode gate). `bang_body` is the STRICTER signal — only the
    /// `!` arm sets it — used by decl-atom arms (`=`, `:`) to differentiate
    /// "register as rule/type" (ADD, bang_body=false) from "evaluate to
    /// itself as inert data" (INTERPRET-body, bang_body=true).
    ///
    /// Cleared at the start of every `eval()` call so each top-level
    /// directive starts with a clean slate (otherwise it would leak across
    /// directives via the propagated env).
    pub(crate) bang_body: bool,
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
            // Unified atom storage
            atom_space: std::sync::Arc::new(super::atom_space::AtomSpace::new(
                shared_mapping.clone(),
                10000,
            )),

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
            tokenizer: Arc::new(RwLock::new(
                crate::backend::modules::GenericTokenizer::<V>::new(),
            )),
            grounded_registry: GroundedRegistry::with_standard_ops(),
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(None),
            type_index_dirty: AtomicBool::new(true),
            fuzzy_matcher: Arc::new(RwLock::new(FuzzyMatcher::new())),
            scope_tracker: Arc::new(RwLock::new(ScopeTracker::new())),
            rule_index: Arc::new(RwLock::new(super::rule_management::RuleIndex::new())),
            // Phase 10.1: Inferred function return types (initially empty)
            inferred_fn_types: DashMap::new(),
            // Override bitset starts empty — no user rules yet
            dispatch_overrides: Arc::new(super::dispatch_overrides::DispatchOverrides::default()),
            // Pragma settings start at defaults (type_check_mode = Permissive)
            pragma_settings: Arc::new(RwLock::new(PragmaSettings::default())),
            // Corelib chain: attach if the global corelib is already loaded.
            // Returns None when:
            //   (a) we're inside `corelib::load_corelib()` on this thread
            //       (the LOADING guard prevents recursive attachment to the
            // Plan Phase F (2026-05-20): corelib_mod field deleted; corelib
            // is now entirely native Rust dispatched in step/sexpr.rs +
            // register_corelib_types(). No MettaMod attachment needed.
        });

        // Register as GC root provider (no-op if V != MettaValue)
        try_register_env_roots(&shared);

        // Use the AtomSpace's epoch — it was already allocated from next_mork_epoch()
        // during AtomSpace::new(). Reusing it ensures env and atom_space share the
        // same epoch for the same SharedMapping, preventing cache mismatches.
        let mork_cache_epoch = shared.atom_space.mork_cache_epoch;

        GenericEnvironment {
            shared,
            factory,
            shared_mapping,
            owns_data: true,
            modified: AtomicBool::new(false),
            current_module_path: None,
            mork_cache_epoch,
            interpret_mode: false,
            bang_body: false,
        }
    }

    /// Get the factory for creating V values.
    #[inline]
    pub fn factory(&self) -> &F {
        &self.factory
    }

    /// S1 TOPLEVEL (2026-05-13): HE two-mode runner accessor.
    ///
    /// Returns `true` when the environment is in HE INTERPRET mode (set by
    /// `(! expr)` directives). Returns `false` in default ADD mode where
    /// bare top-level S-exprs are silent side-effecting facts.
    #[inline]
    pub fn in_interpret_mode(&self) -> bool {
        self.interpret_mode
    }

    /// S1 TOPLEVEL (2026-05-13): set the HE runner-mode flag.
    ///
    /// The bang dispatch site saves the previous value, sets `true` to
    /// enter INTERPRET mode for the duration of the body evaluation, then
    /// restores on completion. See `eval/step/sexpr.rs` `"!"` arm.
    #[inline]
    pub fn set_interpret_mode(&mut self, mode: bool) {
        self.interpret_mode = mode;
    }

    /// S2 BANG-WORD (2026-05-13): true when we are currently evaluating
    /// the BODY of a `(! ...)` directive. Used by decl-atom arms
    /// (`=`, `:`) in `eval/step/sexpr.rs` to differentiate register-mode
    /// vs data-mode evaluation per HE §02.5 / §02.6.
    #[inline]
    pub fn in_bang_body(&self) -> bool {
        self.bang_body
    }

    /// S2 BANG-WORD (2026-05-13): set the bang_body marker.
    ///
    /// The `!` arm in `eval/step/sexpr.rs` flips this true before
    /// dispatching the body. `eval()` resets it false at the start of
    /// each per-directive evaluation so the flag never leaks across
    /// directives (the env is propagated between directives by the
    /// runner; without this reset, a prior `!` directive would taint
    /// subsequent ADD-mode directives).
    #[inline]
    pub fn set_bang_body(&mut self, mode: bool) {
        self.bang_body = mode;
    }

    /// Get the per-environment override bitset for grounded helpers.
    ///
    /// Used by the special-form dispatch arm in `eval_sexpr_step_generic` to
    /// decide whether a user rule should shadow a grounded helper. See
    /// `super::dispatch_overrides` for the design.
    #[inline]
    pub fn dispatch_overrides(&self) -> &super::dispatch_overrides::DispatchOverrides {
        &self.shared.dispatch_overrides
    }

    /// Get the monotonic epoch for MORK symbol cache invalidation.
    #[inline]
    pub fn mork_cache_epoch(&self) -> u64 {
        self.mork_cache_epoch
    }

    /// Compute the MORK byte prefix for rules: `[Arity(3)] + "=" symbol bytes`.
    ///
    /// This is computed on-the-fly from the current `SharedMapping` state rather than
    /// cached, because the MORK symbol ID for "=" can differ across thread-local cache
    /// invalidation boundaries. Used only by fallback trie-navigation paths
    /// (`get_matching_rules_for_expr`, `collect_wildcard_rules`), not the hot path.
    pub(crate) fn compute_rule_prefix(&self) -> Vec<u8> {
        let eq_atom = self.factory.atom("=");
        crate::backend::mork_convert::with_mork_bytes(
            &eq_atom,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |eq_bytes| {
                let mut prefix = Vec::with_capacity(1 + eq_bytes.len());
                prefix.push(0x03); // Arity(3) for (= lhs rhs)
                prefix.extend_from_slice(eq_bytes);
                prefix
            },
        )
        .unwrap_or_else(|_| vec![0x03])
    }

    /// Mark this environment as modified.
    ///
    /// `pub(crate)` so sibling environment modules (e.g. `act_tiered`'s
    /// `attach_act_base`/`detach_act_base`) can flag a tiering change for `union`'s
    /// fast-path detection, the same way `add_to_space` does after a store mutation.
    #[inline]
    pub(crate) fn mark_modified(&self) {
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
            // EXPLICIT clones (owns_data=false) DEEP-COPY the atom store here so
            // the Rust `clone()` + `add_rule`/`add_to_space` API stays COW-isolated
            // (the env clone-isolation invariant; tests in environment::tests).
            // The PeTTa GLOBAL-atomspace semantics for EVALUATION are achieved
            // separately: `fork_for_nondeterminism` (owns_data=true) `Arc::clone`s
            // the store, and the eval-path `add-atom`/`remove-atom`
            // (ProcessAddAtomSpace) mutate that shared store IN PLACE via
            // `add_to_space_shared`/`remove_from_space_shared` — so a branch's
            // write is globally visible, while a direct-API clone still isolates.
            atom_space: std::sync::Arc::new({
                let forked = self.shared.atom_space.fork();
                let forked_btm = forked.btm.read().clone();
                let forked_mapping = forked.shared_mapping.clone();
                let forked_wide = forked.wide_btm.read().clone();
                let forked_count = forked.total_atoms.load(Ordering::Acquire);
                let forked_var_atoms = forked.variable_atoms.read().clone();
                let forked_type_btm = forked.type_btm.read().clone();
                let forked_subtype_btm = forked.subtype_btm.read().clone();
                let forked_inferred_type_btm = forked.inferred_type_btm.read().clone();
                super::atom_space::AtomSpace {
                    btm: RwLock::new(forked_btm),
                    wide_btm: RwLock::new(forked_wide),
                    type_btm: RwLock::new(forked_type_btm),
                    subtype_btm: RwLock::new(forked_subtype_btm),
                    inferred_type_btm: RwLock::new(forked_inferred_type_btm),
                    inferred_type_bloom: std::sync::Arc::new(
                        self.shared.atom_space.inferred_type_bloom.snapshot(),
                    ),
                    inferred_type_generation: AtomicU64::new(
                        self.shared
                            .atom_space
                            .inferred_type_generation
                            .load(Ordering::Acquire),
                    ),
                    fixpoint_generation: AtomicU64::new(
                        self.shared
                            .atom_space
                            .fixpoint_generation
                            .load(Ordering::Acquire),
                    ),
                    shared_mapping: forked_mapping,
                    head_arity_bloom: std::sync::Arc::new(RwLock::new(
                        self.shared.atom_space.head_arity_bloom.read().clone(),
                    )),
                    rule_head_bloom: std::sync::Arc::new(RwLock::new(
                        self.shared.atom_space.rule_head_bloom.read().clone(),
                    )),
                    type_bloom: std::sync::Arc::new(RwLock::new(
                        self.shared.atom_space.type_bloom.read().clone(),
                    )),
                    total_atoms: AtomicUsize::new(forked_count),
                    variable_fact_count: AtomicUsize::new(
                        self.shared
                            .atom_space
                            .variable_fact_count
                            .load(Ordering::Acquire),
                    ),
                    variable_atoms: RwLock::new(forked_var_atoms),
                    mork_cache_epoch: self.shared.atom_space.mork_cache_epoch,
                    // LSM-tiered base carries through CoW make_owned: `Arc<ActBase>`
                    // clone (read-only base) + `tombstones` O(1) PathMap CoW. The owned
                    // env keeps tiering attached and starts with an isolated tombstone
                    // copy, so subsequent overlay/tombstone writes don't leak back to
                    // the source (the env clone-isolation invariant) while the immutable
                    // base is shared. `has_act_base` mirrors the source's gate.
                    act_base: RwLock::new(self.shared.atom_space.act_base.read().clone()),
                    tombstones: RwLock::new(self.shared.atom_space.tombstones.read().clone()),
                    has_act_base: AtomicBool::new(
                        self.shared.atom_space.has_act_base.load(Ordering::Acquire),
                    ),
                }
            }),
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
            grounded_registry: self.shared.grounded_registry.clone(),
            pattern_cache: RwLock::new(self.shared.pattern_cache.read().clone()),
            type_index: RwLock::new(self.shared.type_index.read().clone()),
            type_index_dirty: AtomicBool::new(self.shared.type_index_dirty.load(Ordering::Acquire)),
            // Deep-clone into new Arcs so this owned env has exclusive copies
            fuzzy_matcher: Arc::new(RwLock::new(self.shared.fuzzy_matcher.read().clone())),
            scope_tracker: Arc::new(RwLock::new(self.shared.scope_tracker.read().clone())),
            // Deep-clone into new Arc so this owned env has an exclusive copy
            rule_index: Arc::new(RwLock::new(self.shared.rule_index.read().clone())),
            // Phase 10.1: deep-clone DashMap into independent copy for exclusive mutation
            inferred_fn_types: DashMap::from_iter(
                self.shared
                    .inferred_fn_types
                    .iter()
                    .map(|e| (e.key().clone(), e.value().clone())),
            ),
            // Share override bits — all env clones see the same atomic state
            // so `note_user_rule_added` is visible to subsequent
            // `is_overridden` checks across CoW env handoffs.
            dispatch_overrides: Arc::clone(&self.shared.dispatch_overrides),
            // Share pragma settings — `pragma!` writes propagate through the
            // same Arc<RwLock> across CoW handoffs (matches dispatch_overrides
            // semantics).
            pragma_settings: Arc::clone(&self.shared.pragma_settings),
        });

        // Register new shared state as GC root provider
        try_register_env_roots(&new_shared);

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
            // Gap A (PeTTa global atomspace): SHARE the atom store across
            // nondeterministic branches via Arc::clone — mirroring `rule_index`
            // below (already Arc-shared so "rules added by one branch are
            // visible to others"). A side-effecting `add-atom`/`remove-atom` in
            // a match/superpose branch now commits to the single global store
            // and is visible to sibling branches + the directive, while each
            // branch's BINDINGS stay isolated (states/bindings/types are still
            // deep-forked below).
            atom_space: std::sync::Arc::clone(&self.shared.atom_space),

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
            grounded_registry: self.shared.grounded_registry.clone(),
            // Clear pattern cache instead of copying
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(self.shared.type_index.read().clone()),
            type_index_dirty: AtomicBool::new(self.shared.type_index_dirty.load(Ordering::Acquire)),
            // O(1) Arc::clone for all read-only state during evaluation
            fuzzy_matcher: Arc::clone(&self.shared.fuzzy_matcher),
            scope_tracker: Arc::clone(&self.shared.scope_tracker),
            rule_index: Arc::clone(&self.shared.rule_index),
            // Phase 10.1: clone DashMap into independent copy (fork isolation)
            inferred_fn_types: DashMap::from_iter(
                self.shared
                    .inferred_fn_types
                    .iter()
                    .map(|e| (e.key().clone(), e.value().clone())),
            ),
            // Share override bits across the fork — all nondet branches see
            // the same atomic state (rules added by one branch are visible to
            // others, matching the globally-shared rule_index above).
            dispatch_overrides: Arc::clone(&self.shared.dispatch_overrides),
            // Share pragma settings across the fork — same rationale as
            // dispatch_overrides (writes are rare, sharing is correct).
            pragma_settings: Arc::clone(&self.shared.pragma_settings),
        });

        // Register forked shared state as GC root provider
        try_register_env_roots(&new_shared);

        GenericEnvironment {
            shared: new_shared,
            factory: self.factory.clone(),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: true,
            modified: AtomicBool::new(false),
            current_module_path: self.current_module_path.clone(),

            mork_cache_epoch: self.mork_cache_epoch,

            // S1 TOPLEVEL (2026-05-13): preserve HE INTERPRET mode through
            // fork_for_nondeterminism. Each parallel branch must see the
            // same runner mode as the caller, otherwise downstream gates
            // would behave inconsistently between branches.
            interpret_mode: self.interpret_mode,
            // S2 BANG-WORD (2026-05-13): propagate bang_body to forks so
            // decl-atom arms (`=`/`:`) preserve the data-vs-register
            // semantics across parallel branches of `(! ...)` evaluation.
            bang_body: self.bang_body,
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

        // S1 TOPLEVEL (2026-05-13): union preserves HE INTERPRET mode if
        // EITHER input env was in interpret mode. Symmetric semantics —
        // unioning a bang-flagged env with a non-bang env should yield
        // a bang-flagged env so downstream evaluation in the merged context
        // still emits observable output.
        let merged_interpret_mode = self.interpret_mode || other.interpret_mode;

        // S2 BANG-WORD (2026-05-13): same merge rule for bang_body — if
        // EITHER side was the body of a `(! ...)` directive, the union is
        // too. Decl-atom arms downstream check this to decide register vs
        // return-as-data.
        let merged_bang_body = self.bang_body || other.bang_body;

        // Fast path: same underlying data
        if Arc::ptr_eq(&self.shared, &other.shared) {
            return GenericEnvironment {
                shared: Arc::clone(&self.shared),
                factory: self.factory.clone(),
                shared_mapping: self.shared_mapping.clone(),
                owns_data: false,
                modified: AtomicBool::new(false),
                current_module_path: self.current_module_path.clone(),

                mork_cache_epoch: self.mork_cache_epoch,

                interpret_mode: merged_interpret_mode,
                bang_body: merged_bang_body,
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

                mork_cache_epoch: self.mork_cache_epoch,

                interpret_mode: merged_interpret_mode,
                bang_body: merged_bang_body,
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

                mork_cache_epoch: self.mork_cache_epoch,

                interpret_mode: merged_interpret_mode,
                bang_body: merged_bang_body,
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

                mork_cache_epoch: other.mork_cache_epoch,

                interpret_mode: merged_interpret_mode,
                bang_body: merged_bang_body,
            };
        }

        // Both modified: perform actual merge
        trace!(target: "mettatron::generic_environment::union", "Both environments modified, performing merge");

        // Merge PathMaps by taking max multiplicity
        let merged_btm = {
            let self_btm = self.shared.atom_space.btm.read();
            let other_btm = other.shared.atom_space.btm.read();
            merge_pathmaps_max(&self_btm, &other_btm)
        };

        // Calculate total atoms from merged PathMap
        let merged_total_atoms = {
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
        let max_state_id = self
            .shared
            .next_state_id
            .load(Ordering::Acquire)
            .max(other.shared.next_state_id.load(Ordering::Acquire));
        let max_space_id = self
            .shared
            .next_space_id
            .load(Ordering::Acquire)
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
            // Gap A: atom_space is Arc<AtomSpace>; wrap the merged store in a
            // fresh Arc (full merge preserved for distinct module spaces; for
            // branch-unions both inputs share the global store → idempotent).
            atom_space: std::sync::Arc::new(super::atom_space::AtomSpace {
                btm: RwLock::new(merged_btm),
                shared_mapping: self.shared_mapping.clone(),
                head_arity_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(
                    10000,
                ))), // Reset (will be rebuilt)
                rule_head_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(5000))), // Reset (will be rebuilt)
                type_bloom: std::sync::Arc::new(RwLock::new(super::bloom::TypeBloomFilter::new(
                    1000,
                ))), // Reset (will be rebuilt from type_btm)
                // Merge wide_btm using same lattice algebra as btm
                wide_btm: RwLock::new(merge_pathmaps_max(
                    &self.shared.atom_space.wide_btm.read(),
                    &other.shared.atom_space.wide_btm.read(),
                )),
                // Merge type and subtype PathMaps
                type_btm: RwLock::new(merge_pathmaps_max(
                    &self.shared.atom_space.type_btm.read(),
                    &other.shared.atom_space.type_btm.read(),
                )),
                subtype_btm: RwLock::new(merge_pathmaps_max(
                    &self.shared.atom_space.subtype_btm.read(),
                    &other.shared.atom_space.subtype_btm.read(),
                )),
                // Phase 10.1: merge inferred type PathMaps
                inferred_type_btm: RwLock::new(merge_pathmaps_max(
                    &self.shared.atom_space.inferred_type_btm.read(),
                    &other.shared.atom_space.inferred_type_btm.read(),
                )),
                // Phase 10.1: merge inferred type bloom via bitwise OR
                inferred_type_bloom: {
                    let merged =
                        std::sync::Arc::new(self.shared.atom_space.inferred_type_bloom.snapshot());
                    merged.merge_from(&other.shared.atom_space.inferred_type_bloom);
                    merged
                },
                // Phase 10.5: max(generation) forces fixpoint to see all new types;
                // min(fixpoint_gen) forces re-fixpoint if either side had unprocessed types.
                inferred_type_generation: AtomicU64::new(
                    self.shared
                        .atom_space
                        .inferred_type_generation
                        .load(Ordering::Acquire)
                        .max(
                            other
                                .shared
                                .atom_space
                                .inferred_type_generation
                                .load(Ordering::Acquire),
                        ),
                ),
                fixpoint_generation: AtomicU64::new(
                    self.shared
                        .atom_space
                        .fixpoint_generation
                        .load(Ordering::Acquire)
                        .min(
                            other
                                .shared
                                .atom_space
                                .fixpoint_generation
                                .load(Ordering::Acquire),
                        ),
                ),
                total_atoms: AtomicUsize::new(merged_total_atoms),
                // Conservative monotonic gate (Stage 1): merged btm = union of both
                // sources, so it holds a variable fact iff either source did → max.
                variable_fact_count: AtomicUsize::new(
                    self.shared
                        .atom_space
                        .variable_fact_count
                        .load(Ordering::Acquire)
                        .max(
                            other
                                .shared
                                .atom_space
                                .variable_fact_count
                                .load(Ordering::Acquire),
                        ),
                ),
                variable_atoms: RwLock::new(Vec::new()),
                // Same SharedMapping as self → same epoch (cache entries remain valid)
                mork_cache_epoch: self.mork_cache_epoch,
                // A GENUINE merge folds all source `btm`s into one in-memory union
                // (`merge_pathmaps_max` above), so there is no longer a single immutable
                // ACT base backing the result — DROP tiering (None / empty tombstones /
                // gate off). The branch-union FAST PATHS (`Arc::ptr_eq` / only-one-
                // modified) `Arc::clone` `self.shared` and so SHARE the base for free;
                // only this both/all-modified rebuild re-decides tiering, and the safe
                // lossless choice is "fully materialized, no base". Re-`attach-act-base!`
                // / `compact-space!` if a base is wanted after a merge.
                act_base: RwLock::new(None),
                tombstones: RwLock::new(PathMap::new()),
                has_act_base: AtomicBool::new(false),
            }),
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
            grounded_registry: self.shared.grounded_registry.clone(),

            // Clear/reset caches after merge
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(None), // Invalidate
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
            inferred_fn_types: {
                let merged: DashMap<String, Vec<V>> = DashMap::from_iter(
                    self.shared
                        .inferred_fn_types
                        .iter()
                        .map(|e| (e.key().clone(), e.value().clone())),
                );
                for entry in other.shared.inferred_fn_types.iter() {
                    let mut vec = merged.entry(entry.key().clone()).or_default();
                    for t in entry.value() {
                        if !vec.contains(t) {
                            vec.push(t.clone());
                        }
                    }
                }
                merged
            },
            // Override bits: take self's snapshot, then bump for each of
            // other's user rules whose head is in the overridable set.
            // The merged rule_index above already contains other's rules,
            // so the bits stay consistent with the merged index. The result
            // is wrapped in a fresh Arc — the merged env owns a distinct
            // state from either parent; subsequent env clones share this
            // new Arc via Arc::clone.
            dispatch_overrides: {
                let merged = self.shared.dispatch_overrides.snapshot();
                for entry in other.shared.rule_index.read().get_all_rules() {
                    if let Some(head) = entry.lhs.get_head_symbol() {
                        if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                            merged.note_user_rule_added(id);
                        }
                    }
                }
                Arc::new(merged)
            },
            // Merge pragma settings — other takes precedence on conflict, like
            // bindings. Wrap in a fresh Arc<RwLock> so future writes to either
            // input env don't leak into the merged env.
            pragma_settings: {
                let merged = {
                    let self_p = self.shared.pragma_settings.read();
                    let other_p = other.shared.pragma_settings.read();
                    let mut combined = self_p.clone();
                    // other wins on type_check_mode if either is Auto
                    if other_p.type_check_mode == TypeCheckMode::Auto
                        || self_p.type_check_mode == TypeCheckMode::Auto
                    {
                        combined.type_check_mode = TypeCheckMode::Auto;
                    }
                    // rule_fire_mode: Specificity wins over Nondet on union
                    // (the stricter setting takes precedence, mirroring the
                    // TypeCheckMode::Auto precedence above).
                    if other_p.rule_fire_mode == RuleFireMode::Specificity
                        || self_p.rule_fire_mode == RuleFireMode::Specificity
                    {
                        combined.rule_fire_mode = RuleFireMode::Specificity;
                    }
                    // Merge other's keys into combined.other
                    for (k, v) in other_p.other.iter() {
                        combined.other.insert(k.clone(), v.clone());
                    }
                    combined
                };
                Arc::new(RwLock::new(merged))
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
            shared_mapping: self.shared_mapping.clone(),
            owns_data: true,
            modified: AtomicBool::new(true),
            current_module_path: other
                .current_module_path
                .clone()
                .or_else(|| self.current_module_path.clone()),

            mork_cache_epoch: self.mork_cache_epoch,

            // S1 TOPLEVEL (2026-05-13): preserve HE INTERPRET mode through
            // the binary union merge (both-modified path).
            interpret_mode: merged_interpret_mode,
            // S2 BANG-WORD: same merge rule for bang_body.
            bang_body: merged_bang_body,
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

            mork_cache_epoch: self.mork_cache_epoch,

            // S1 TOPLEVEL (2026-05-13): preserve HE INTERPRET mode.
            interpret_mode: self.interpret_mode,
            // S2 BANG-WORD (2026-05-13): preserve bang_body.
            bang_body: self.bang_body,
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
            let mut result = base.shared.atom_space.btm.read().clone();
            for other in &others[merge_start_idx..] {
                let other_btm = other.shared.atom_space.btm.read();
                result = merge_pathmaps_max(&result, &other_btm);
            }
            // If we started from self and include_self is true, we already have self's data
            // Otherwise merge self's data too
            if !include_self {
                let self_btm = self.shared.atom_space.btm.read();
                result = merge_pathmaps_max(&result, &self_btm);
            }
            result
        };

        // Calculate total atoms from merged PathMap
        let merged_total_atoms = {
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
            // Gap A: atom_space is Arc<AtomSpace>; wrap the merged store in a
            // fresh Arc (full merge preserved for distinct module spaces; for
            // branch-unions both inputs share the global store → idempotent).
            atom_space: std::sync::Arc::new(super::atom_space::AtomSpace {
                btm: RwLock::new(merged_btm),
                shared_mapping: self.shared.atom_space.shared_mapping.clone(),
                head_arity_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(
                    10000,
                ))), // Reset (will be rebuilt)
                rule_head_bloom: std::sync::Arc::new(RwLock::new(HeadArityBloomFilter::new(5000))), // Reset (will be rebuilt)
                type_bloom: std::sync::Arc::new(RwLock::new(super::bloom::TypeBloomFilter::new(
                    1000,
                ))), // Reset (will be rebuilt from type_btm)
                // Merge wide_btm from all environments using same lattice algebra as btm
                wide_btm: RwLock::new({
                    let mut merged_wide = self.shared.atom_space.wide_btm.read().clone();
                    for other_env in others.iter() {
                        merged_wide = merge_pathmaps_max(
                            &merged_wide,
                            &other_env.shared.atom_space.wide_btm.read(),
                        );
                    }
                    merged_wide
                }),
                // Merge type PathMaps from all environments
                type_btm: RwLock::new({
                    let mut merged_types = self.shared.atom_space.type_btm.read().clone();
                    for other_env in others.iter() {
                        merged_types = merge_pathmaps_max(
                            &merged_types,
                            &other_env.shared.atom_space.type_btm.read(),
                        );
                    }
                    merged_types
                }),
                // Merge subtype PathMaps from all environments
                subtype_btm: RwLock::new({
                    let mut merged_subs = self.shared.atom_space.subtype_btm.read().clone();
                    for other_env in others.iter() {
                        merged_subs = merge_pathmaps_max(
                            &merged_subs,
                            &other_env.shared.atom_space.subtype_btm.read(),
                        );
                    }
                    merged_subs
                }),
                // Phase 10.1: merge inferred type PathMaps from all environments
                inferred_type_btm: RwLock::new({
                    let mut merged_inf = self.shared.atom_space.inferred_type_btm.read().clone();
                    for other_env in others.iter() {
                        merged_inf = merge_pathmaps_max(
                            &merged_inf,
                            &other_env.shared.atom_space.inferred_type_btm.read(),
                        );
                    }
                    merged_inf
                }),
                // Phase 10.1: merge inferred type bloom filters via bitwise OR
                inferred_type_bloom: {
                    let merged_bloom =
                        std::sync::Arc::new(self.shared.atom_space.inferred_type_bloom.snapshot());
                    for other_env in others.iter() {
                        merged_bloom.merge_from(&other_env.shared.atom_space.inferred_type_bloom);
                    }
                    merged_bloom
                },
                // Phase 10.5: max(generation) across all envs; min(fixpoint_gen) forces re-fixpoint
                inferred_type_generation: AtomicU64::new({
                    let mut max_gen = self
                        .shared
                        .atom_space
                        .inferred_type_generation
                        .load(Ordering::Acquire);
                    for other_env in others.iter() {
                        max_gen = max_gen.max(
                            other_env
                                .shared
                                .atom_space
                                .inferred_type_generation
                                .load(Ordering::Acquire),
                        );
                    }
                    max_gen
                }),
                fixpoint_generation: AtomicU64::new({
                    let mut min_gen = self
                        .shared
                        .atom_space
                        .fixpoint_generation
                        .load(Ordering::Acquire);
                    for other_env in others.iter() {
                        min_gen = min_gen.min(
                            other_env
                                .shared
                                .atom_space
                                .fixpoint_generation
                                .load(Ordering::Acquire),
                        );
                    }
                    min_gen
                }),
                total_atoms: AtomicUsize::new(merged_total_atoms),
                // Conservative monotonic gate (Stage 1): merged btm = union of self +
                // all others, so it holds a variable fact iff any source did → max.
                variable_fact_count: AtomicUsize::new(
                    others
                        .iter()
                        .map(|o| {
                            o.shared
                                .atom_space
                                .variable_fact_count
                                .load(Ordering::Acquire)
                        })
                        .fold(
                            self.shared
                                .atom_space
                                .variable_fact_count
                                .load(Ordering::Acquire),
                            usize::max,
                        ),
                ),
                variable_atoms: RwLock::new(Vec::new()),
                // Same SharedMapping as self → same epoch (cache entries remain valid)
                mork_cache_epoch: self.mork_cache_epoch,
                // A GENUINE merge folds all source `btm`s into one in-memory union
                // (`merge_pathmaps_max` above), so there is no longer a single immutable
                // ACT base backing the result — DROP tiering (None / empty tombstones /
                // gate off). The branch-union FAST PATHS (`Arc::ptr_eq` / only-one-
                // modified) `Arc::clone` `self.shared` and so SHARE the base for free;
                // only this both/all-modified rebuild re-decides tiering, and the safe
                // lossless choice is "fully materialized, no base". Re-`attach-act-base!`
                // / `compact-space!` if a base is wanted after a merge.
                act_base: RwLock::new(None),
                tombstones: RwLock::new(PathMap::new()),
                has_act_base: AtomicBool::new(false),
            }),
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
            grounded_registry: self.shared.grounded_registry.clone(),

            // Clear/reset caches after merge
            pattern_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).expect("1000 is non-zero"),
            )),
            type_index: RwLock::new(None), // Invalidate
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
                let merged: DashMap<String, Vec<V>> = DashMap::from_iter(
                    self.shared
                        .inferred_fn_types
                        .iter()
                        .map(|e| (e.key().clone(), e.value().clone())),
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
                merged
            },
            // Override bits: take self's snapshot, then bump for each rule
            // from every `other` env whose head is in the overridable set.
            // Mirrors the merged rule_index above. Wrapped in a fresh Arc
            // so subsequent env clones share this merged state.
            dispatch_overrides: {
                let merged = self.shared.dispatch_overrides.snapshot();
                for other_env in others {
                    for entry in other_env.shared.rule_index.read().get_all_rules() {
                        if let Some(head) = entry.lhs.get_head_symbol() {
                            if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                                merged.note_user_rule_added(id);
                            }
                        }
                    }
                }
                Arc::new(merged)
            },
            // Merge pragma settings — same algebra as binary union
            // (Auto from any input wins; `other` keys merge into self's).
            pragma_settings: {
                let merged = {
                    let self_p = self.shared.pragma_settings.read();
                    let mut combined = self_p.clone();
                    for other_env in others {
                        let other_p = other_env.shared.pragma_settings.read();
                        if other_p.type_check_mode == TypeCheckMode::Auto {
                            combined.type_check_mode = TypeCheckMode::Auto;
                        }
                        if other_p.rule_fire_mode == RuleFireMode::Specificity {
                            combined.rule_fire_mode = RuleFireMode::Specificity;
                        }
                        for (k, v) in other_p.other.iter() {
                            combined.other.insert(k.clone(), v.clone());
                        }
                    }
                    combined
                };
                Arc::new(RwLock::new(merged))
            },
            // Union: corelib is process-wide (same Arc); prefer self's reference
            // — both envs hold identical Arc-shared corelib refs.
            // Multi-other union: corelib is process-wide (Arc-shared identical
            // across all envs); self's reference is canonical. `others` was
            // consumed earlier by `others.last()` — re-iterating would require
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
            shared_mapping: self.shared_mapping.clone(),
            owns_data: true,
            modified: AtomicBool::new(true),
            current_module_path: last_env
                .current_module_path
                .clone()
                .or_else(|| self.current_module_path.clone()),

            mork_cache_epoch: self.mork_cache_epoch,

            // S1 TOPLEVEL (2026-05-13): preserve HE INTERPRET mode across
            // batch merges. If any contributing env was in interpret mode,
            // the merged env stays in interpret mode (matches the binary
            // union semantics).
            interpret_mode: self.interpret_mode || others.iter().any(|e| e.interpret_mode),
            // S2 BANG-WORD (2026-05-13): same merge for bang_body.
            bang_body: self.bang_body || others.iter().any(|e| e.bang_body),
        }
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    /// Get the grounded registry.
    pub fn grounded_registry(&self) -> &GroundedRegistry {
        &self.shared.grounded_registry
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

    // ========================================================================
    // Pragma Accessors (S-step 2026-05-16)
    // ========================================================================

    /// Get the current type-check mode (default: `Permissive`).
    ///
    /// Used by `check_call_site_types` to decide whether to fire on
    /// `%Undefined%` arg types.
    pub fn get_type_check_mode(&self) -> TypeCheckMode {
        self.shared.pragma_settings.read().type_check_mode
    }

    /// Set the type-check mode.
    ///
    /// Called by the `pragma!` arm when handling `(pragma! type-check auto|permissive)`.
    /// Writes are propagated through the shared `Arc<RwLock<PragmaSettings>>`
    /// so all clones (forks/unioned envs) see the new value.
    pub fn set_type_check_mode(&self, mode: TypeCheckMode) {
        let mut p = self.shared.pragma_settings.write();
        p.type_check_mode = mode;
        p.user_set_keys.insert("type-check".to_string());
    }

    /// Get the current rule-fire mode (default: `Nondet` = HE-bisim).
    ///
    /// Consulted by `RuleIndex::match_rules_native` to decide whether to
    /// apply the specificity filter after candidate matching.
    pub fn get_rule_fire_mode(&self) -> RuleFireMode {
        self.shared.pragma_settings.read().rule_fire_mode
    }

    /// Set the rule-fire mode (`(pragma! rule-fire-mode specificity|nondet)`).
    ///
    /// Bumps `RULE_EPOCH` so cached match-result entries (keyed on epoch)
    /// don't serve stale results across mode changes.
    pub fn set_rule_fire_mode(&self, mode: RuleFireMode) {
        {
            let mut p = self.shared.pragma_settings.write();
            p.rule_fire_mode = mode;
            p.user_set_keys.insert("rule-fire-mode".to_string());
        }
        // Cache invalidation: bump rule epoch so trampoline/operator caches
        // re-fetch under the new mode. (Match results legitimately differ
        // when the pragma toggles even though the rule set is unchanged.)
        crate::backend::environment::rule_management::RULE_EPOCH
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// Get the configured `max-stack-depth` (None = no per-env limit).
    /// HE-bisim §06.4.6: when eval-loop depth exceeds this, the form
    /// returns `(Error <form> StackOverflow)`. T04/047 verifies.
    ///
    /// HE-bisim sentinel (interpreter.rs:392): `max_stack_depth > 0` —
    /// `Some(0)` means "no limit" (user opted into HE-default-unlimited).
    /// Normalise to `None` so the depth-check sites short-circuit.
    pub fn get_max_stack_depth(&self) -> Option<usize> {
        match self.shared.pragma_settings.read().max_stack_depth {
            Some(0) => None,
            other => other,
        }
    }

    /// Set the `max-stack-depth` pragma value.
    pub fn set_max_stack_depth(&self, depth: usize) {
        let mut p = self.shared.pragma_settings.write();
        p.max_stack_depth = Some(depth);
        p.user_set_keys.insert("max-stack-depth".to_string());
    }

    /// Has the user explicitly set this pragma key via the write form?
    pub fn pragma_user_set(&self, key: &str) -> bool {
        self.shared
            .pragma_settings
            .read()
            .user_set_keys
            .contains(key)
    }

    /// Store an arbitrary pragma key/value pair (no semantic effect, HE-bisim).
    pub fn set_pragma_other(&self, key: &str, value: &str) {
        let mut p = self.shared.pragma_settings.write();
        p.other.insert(key.to_string(), value.to_string());
        p.user_set_keys.insert(key.to_string());
    }

    /// Read an arbitrary pragma key (returns `None` if unset).
    /// HE-bisim §9.8.2: powers the `(pragma! key)` 1-arg read form.
    pub fn get_pragma_other(&self, key: &str) -> Option<String> {
        self.shared.pragma_settings.read().other.get(key).cloned()
    }

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
    /// `wide_btm` stores only byte keys + Multiplicity — no V references, no GC tracing needed.
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
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false, // CoW: clones do not own data initially
            modified: AtomicBool::new(false),
            current_module_path: self.current_module_path.clone(),

            mork_cache_epoch: self.mork_cache_epoch,

            // S1 TOPLEVEL (2026-05-13): preserve HE INTERPRET mode across
            // clones. The bang dispatch in `eval/step/sexpr.rs` flips this
            // flag, and downstream evaluation steps clone the env to pass
            // through the trampoline — if we reset to false here, the gate
            // in process_single_combination_generic would fire even inside
            // `(! expr)` bodies, swallowing the reduction's result.
            interpret_mode: self.interpret_mode,
            // S2 BANG-WORD (2026-05-13): preserve bang_body across clones
            // for the same reason — the decl-atom arms check this flag and
            // expect it to follow through trampoline-driven cloning.
            bang_body: self.bang_body,
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

impl GenericEnvironmentShared<MettaValue> {
    /// Structural E₀ root reader (CESK Phase A4 seam). The persistent global
    /// environment — named spaces, bindings, types, states, pattern cache, atom
    /// space, tokenizer, rule index, inferred fn types — is the always-live **E₀**
    /// root of the CESK machine (the part of `Reachable(⟨C,E,K⟩)` that is global,
    /// not control-flow). This inherent method is the STRUCTURAL entry point: the
    /// collector reads E₀ directly from the machine, not by discovering it through
    /// the `ROOT_REGISTRY`. The `RootProvider` impl below delegates to it
    /// (byte-identical) during the registry-bridge period; Phase A5 deletes the
    /// registry and the structural collector calls this directly.
    pub(crate) fn collect_roots_into(&self, roots: &mut Vec<MettaValue>) {
        // Pre-estimate capacity from all sources to eliminate Vec reallocations.
        // Read locks under quiescent GC are uncontended (ACTIVE_EVALUATORS == 0).
        {
            let estimated = self.named_spaces.read().values().map(|(_, a)| a.len()).sum::<usize>()
                + self.bindings.read().len()
                + self.types.read().len()
                + self.states.read().len()
                + self.pattern_cache.read().len()
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

        // Pattern cache keys: LruCache<MettaValue, Vec<u8>>
        // The keys are MettaValues whose inner pointers reference slab slots.
        // Without collecting these, GC could free slots still referenced by
        // cached keys, causing use-after-free on next cache lookup (Hash/Eq).
        {
            let cache = self.pattern_cache.read();
            roots.extend(cache.iter().map(|(key, _)| *key));
        }

        // AtomSpace GC roots: variable atoms + large expression PathMap values.
        // These hold slab-allocated MettaValues that must be kept alive by GC.
        self.atom_space.collect_gc_roots(roots);

        // Tokenizer values: bind! stores MettaValues inside closures.
        // Without collecting these, GC frees Space handles (e.g., &kb, &stack)
        // and State values (e.g., &sp) that are still looked up via token resolution.
        {
            let tokenizer = self.tokenizer.read();
            tokenizer.collect_gc_values_into(roots);
        }

        // RuleIndex: cached LHS/RHS/rhs_type MettaValues for rule matching,
        // plus compiled_rhs bytecode chunk constant pools.
        // Without collecting these, GC frees slab slots still referenced by
        // RuleEntry fields, causing use-after-free when match_rules_native()
        // applies bindings to the RHS template or the VM executes PushConstant
        // opcodes from the pre-compiled RHS chunk.
        {
            let rule_index = self.rule_index.read();
            for e in rule_index.get_all_rules() {
                roots.push(e.lhs);
                roots.push(e.rhs);
                if let Some(rt) = &e.rhs_type {
                    roots.push(rt.clone());
                }
                // Collect constants from pre-compiled bytecode chunks.
                // compiled_rhs holds Arc<GenericBytecodeChunk<MettaValue>> type-erased
                // as dyn Any. Its constants: Vec<MettaValue> pool contains slab-allocated
                // values that must be traced as GC roots.
                if let Some(ref compiled) = e.compiled_rhs {
                    if let Some(chunk) =
                        compiled.downcast_ref::<crate::backend::bytecode::chunk::BytecodeChunk>()
                    {
                        crate::backend::bytecode::cache::collect_chunk_constants(chunk, roots);
                    }
                }
            }
        }

        // Inferred function types: Phase 10.1 caches return types from rule RHS analysis.
        // Without collecting these, GC frees slab-allocated type atoms still referenced
        // by type inference lookups (e.g., the $a atom from let*'s (-> Bindings $a $a)).
        for entry in self.inferred_fn_types.iter() {
            roots.extend(entry.value().iter().cloned());
        }
    }
}

// E1-FLIP Path B V4 (B2′): expose `GenericEnvironmentShared<MettaValue>` to the
// dedicated-GC-thread driver's global live-env registry. The body is EXACTLY the
// verified-complete structural E₀ reader above — zero new traversal. `#[cfg(index-gc)]`
// because the `EnvRoots` trait + the `LIVE_ENVS` registry are index-only (in slab the
// env is walked via the structural quiescence reader, not this registry).
#[cfg(feature = "index-gc")]
impl crate::backend::models::gc_allocator::EnvRoots for GenericEnvironmentShared<MettaValue> {
    fn collect_env_roots(&self, out: &mut Vec<MettaValue>) {
        self.collect_roots_into(out);
    }
}

#[cfg(not(feature = "index-gc"))]
impl RootProvider for GenericEnvironmentShared<MettaValue> {
    /// Delegates to the inherent structural reader `collect_roots_into`, so the
    /// registry-discovered path and the structural E₀ read are the SAME code
    /// (one source of truth) throughout the bridge period. Phase A5 deletes this
    /// `RootProvider` impl once the structural collector reads E₀ directly.
    #[inline]
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        self.collect_roots_into(roots);
    }
}

// ============================================================================
// MORK Space Access Methods
// ============================================================================

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Create a thread-local Space for operations.
    /// Following the Rholang LSP pattern: cheap clone via structural sharing.
    pub fn create_space(&self) -> Space<Multiplicity> {
        let btm = self.shared.atom_space.btm.read().clone();
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
        *self.shared.atom_space.btm.write() = space.btm;
        self.shared_mapping = space.sm;
        self.mark_modified(); // CoW: mark as modified
    }

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
        self.shared
            .tokenizer
            .write()
            .register_token_value(token, value);
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
    /// ## Unified Routing
    ///
    /// Automatically detects and routes special atom types:
    /// - Rules `(= lhs rhs)` → `add_rule()` for PathMap (De Bruijn) + RuleIndex population
    /// - Type assertions `(: name type)` → literal PathMap + types HashMap registration
    /// - All other atoms → literal PathMap encoding
    ///
    /// ## Multiplicity Tracking
    ///
    /// Uses MeTTa HE semantics: each `add_to_space` call increments the atom's multiplicity.
    pub fn add_to_space(&mut self, value: &V) {
        self.make_owned();

        // Check if this is a rule (= lhs rhs) — route through add_rule() which handles
        // BOTH PathMap insertion (De Bruijn) AND RuleIndex population.
        if let Some((lhs, rhs)) = extract_rule_parts(value) {
            // PT-canonical Lazy peel (2026-05-23): when a rule is registered
            // via add-atom from inside a meta-typed rule body, the substituted
            // (lhs, rhs) come back wrapped in `Lazy(...)` because
            // `apply_bindings_with_rename_scoped_lazy` (lazy substitution path
            // used by meta-typed rules) marks each substituted variable as
            // Lazy to inhibit downstream rule firing. Lazy is documented
            // invisible for display / hash / PartialEq / MORK conversion, but
            // structural accessors (`as_sexpr`, `get_head_symbol`, `get_arity`)
            // on the underlying MettaValue type do NOT see through it. Deep-
            // unwrap recursively because lazy substitution also wraps INNER
            // substituted values (e.g. `(Truth_ModusPonens Lazy((father a b))
            // Lazy((stv 1.0 0.9)))`). Without deep peel, rule firing succeeds
            // but the RHS body's Lazy markers inhibit downstream rule firing
            // on the inner subterms when the rule body re-evaluates (see PLN-
            // main `=>` repro: rule registered correctly but `(close-relative
            // a b)` body fails to reduce `(father a b)` because the inner
            // Lazy wrapper prevents Truth_ModusPonens from receiving (stv 1.0
            // 0.9) as its argument). Strip ALL Lazy markers before storing.
            let lhs = deep_unwrap_lazy(&lhs, &self.factory);
            let rhs = deep_unwrap_lazy(&rhs, &self.factory);
            self.add_rule(lhs, rhs);
            // add_rule() inserts the LHS head/arity into the bloom filter (for match_rules_native),
            // but match_space() queries by the full expression head ("=", arity 3).
            // Insert the full rule expression head/arity so match_space() doesn't reject it.
            if let Some(head) = value.get_head_symbol() {
                let arity = value.get_arity() as u8;
                self.shared
                    .atom_space
                    .head_arity_bloom
                    .write()
                    .insert(head, arity);
            }
            return;
        }

        // LSM-tiered base (Stage 5a): if this literal fact is currently SUPPRESSED by a
        // tombstone (a base copy that was `remove`d), `add-atom` REVIVES one base copy
        // instead of writing a fresh overlay copy — keeping `add`/`remove` exact bag
        // inverses (see `act_tiered.rs`). Gated internally on `has_act_base` (no-op +
        // single relaxed load when untiered). Returning here leaves `total_atoms`/bloom
        // untouched (the +1 is realized in the base layer, not the overlay counter).
        if self.untombstone_on_add(value) {
            invalidate_space_mutation_caches();
            self.mark_modified();
            return;
        }

        // Stage 1 (MM2 ProductZipper gate): rules returned above, so every atom
        // reaching here is a non-rule fact stored literally in `btm`. A variable-
        // containing one makes the directional conjunction fast path incomplete —
        // count it so `match_conjunction_query_multi` falls back. (Over-counts on
        // re-adds / wide atoms, which only causes safe extra fallback.)
        if value.has_variables_fast() {
            self.shared
                .atom_space
                .variable_fact_count
                .fetch_add(1, Ordering::Relaxed);
        }

        // Check if this is a type assertion (: name type) or subtype declaration (:<  sub super)
        // — also register in the types/subtypes HashMap for fast lookup.
        // Track whether this is a type or subtype atom for incremental PathMap updates.
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
                            if let (Some(sub), Some(sup)) = (items[1].as_atom(), items[2].as_atom())
                            {
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

        // Non-rule: use literal encoding (existing path)
        match with_mork_bytes(
            value,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |mork_bytes| {
                let mut btm = self.shared.atom_space.btm.write();
                add_atom(&mut btm, mork_bytes);
                drop(btm);

                // Incrementally update type/subtype dedicated PathMaps + type bloom filter
                if is_type_atom {
                    let mut type_btm = self.shared.atom_space.type_btm.write();
                    add_atom(&mut type_btm, mork_bytes);
                    // Insert atom name into type bloom filter for O(1) early rejection
                    if let Some(ref name) = type_atom_name {
                        self.shared.atom_space.type_bloom.write().insert(name);
                    }
                } else if is_subtype_atom {
                    let mut subtype_btm = self.shared.atom_space.subtype_btm.write();
                    add_atom(&mut subtype_btm, mork_bytes);
                }

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);

                // Use trait method for head symbol extraction
                if let Some(head) = value.get_head_symbol() {
                    let arity = value.get_arity() as u8;
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .insert(head, arity);
                }
            },
        ) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64): Wide MORK encoding
                let mut wide_key = Vec::new();
                encode_wide_storage(value, &mut wide_key);
                {
                    let mut wbtm = self.shared.atom_space.wide_btm.write();
                    add_atom(&mut wbtm, &wide_key);
                }

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);

                if let Some(head) = value.get_head_symbol() {
                    let arity = value.get_arity() as u8;
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .insert(head, arity);
                }
            }
        }
        invalidate_space_mutation_caches();
    }

    /// Remove a fact from MORK Space by exact match.
    ///
    /// ## Unified Routing
    ///
    /// Automatically detects and routes special atom types:
    /// - Rules `(= lhs rhs)` → De Bruijn PathMap removal + RuleIndex sync
    /// - Type assertions `(: name type)` → literal PathMap removal + types HashMap removal
    /// - All other atoms → literal PathMap removal
    ///
    /// ## Multiplicity Tracking
    ///
    /// Decrements the atom's multiplicity. If multiplicity reaches 0, the atom is removed.
    pub fn remove_from_space(&mut self, value: &V) {
        self.make_owned();

        // Check if this is a type assertion (: name type) or subtype declaration (:< sub super)
        // — remove from the types/subtypes HashMap so queries stay consistent.
        // Track for incremental PathMap updates.
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
                            if let (Some(sub), Some(sup)) = (items[1].as_atom(), items[2].as_atom())
                            {
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

        // Check if this is a rule (= lhs rhs) — rules are stored with De Bruijn encoding
        if let Some((lhs, rhs)) = extract_rule_parts(value) {
            // Rule removal: use De Bruijn encoding to match PathMap entry
            let sm = self.shared_mapping.clone();
            // Capture the full De Bruijn bytes inside the callback so we can
            // also use them to sync the RuleIndex via alpha-equivalent
            // comparison (rules are stored freshened by Fix 3B, so structural
            // MettaValue equality fails).
            let mut captured_bytes: Option<Vec<u8>> = None;
            match with_mork_query_bytes(value, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
                // Save bytes for post-callback RuleIndex sync
                captured_bytes = Some(mork_bytes.to_vec());

                let mut btm = self.shared.atom_space.btm.write();

                let current_count = get_multiplicity(&btm, mork_bytes);
                if current_count == 0 {
                    if !btm.contains(mork_bytes) {
                        return;
                    }
                    btm.remove(mork_bytes);
                    drop(btm);
                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_sub(1, Ordering::Relaxed);
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                    return;
                }

                let new_count = remove_atom(&mut btm, mork_bytes);

                if new_count == 0 {
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                }

                drop(btm);
                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
            }) {
                Ok(()) => {
                    // Sync RuleIndex: use the captured full De Bruijn bytes to
                    // find and remove the matching entry via alpha-equivalent
                    // comparison. The wrapper returns `Some(_)` whenever the
                    // index was authoritatively updated (whether the entry
                    // was decremented or fully removed) and `None` only when
                    // no matching entry was found. We must check `.is_some()`
                    // — NOT pattern-match for `Some(true)` — because matching
                    // for "fully removed" would re-fire the structural
                    // fallback on a just-decremented multiplicity > 1 entry,
                    // silently corrupting it.
                    let mut idx = self.shared.rule_index.write();
                    let synced: bool = if let Some(ref bytes) = captured_bytes {
                        idx.remove_rule_by_debruijn(bytes).is_some()
                    } else {
                        false
                    };
                    let mut structural_removed = false;
                    if !synced {
                        let is_ground = !lhs.contains_variables() && !rhs.contains_variables();
                        debug_assert!(
                            is_ground || captured_bytes.is_some(),
                            "remove_from_space: variable rule had no captured De Bruijn bytes — structural fallback would re-enter the pre-fix bug"
                        );
                        if is_ground {
                            structural_removed = idx.remove_rule(&lhs, &rhs);
                        }
                        // For variable rules without captured bytes, do NOT fall
                        // back to structural removal — that path is broken for
                        // freshened rules. Leaving the rule in the index is the
                        // lesser evil (no silent corruption).
                    }
                    drop(idx);
                    // If the rule was actually removed and its head is in the
                    // overridable set, decrement the override bitset so the
                    // dispatch arm goes back to the grounded fast path when
                    // no user rules remain.
                    if synced || structural_removed {
                        if let Some(head) = lhs.get_head_symbol() {
                            if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                                self.shared.dispatch_overrides.note_user_rule_removed(id);
                            }
                        }
                    }
                }
                Err(_) => {
                    // Fallback for large expressions (arity >= 64): Wide MORK encoding
                    let mut wide_key = Vec::new();
                    encode_wide_storage(value, &mut wide_key);
                    let mut wbtm = self.shared.atom_space.wide_btm.write();
                    let count = get_multiplicity(&wbtm, &wide_key);
                    if count <= 1 {
                        wbtm.remove(&wide_key);
                    } else {
                        remove_atom(&mut wbtm, &wide_key);
                    }
                    drop(wbtm);
                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_sub(1, Ordering::Relaxed);
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();

                    // Sync RuleIndex for wide rules via wide De Bruijn bytes.
                    // Same `.is_some()` rule as the narrow path — see comment
                    // above for why we mustn't pattern-match `Some(true)`.
                    let mut full_wide_ctx =
                        crate::backend::wide_mork::encoding::WideConversionContext::new();
                    let mut wide_full_debruijn = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_debruijn(
                        value,
                        &mut full_wide_ctx,
                        &mut wide_full_debruijn,
                    );
                    let mut idx = self.shared.rule_index.write();
                    let synced: bool = idx.remove_rule_by_debruijn(&wide_full_debruijn).is_some();
                    let mut structural_removed = false;
                    if !synced {
                        let is_ground = !lhs.contains_variables() && !rhs.contains_variables();
                        debug_assert!(
                            is_ground,
                            "remove_from_space (wide path): variable rule not found via wide De Bruijn"
                        );
                        if is_ground {
                            structural_removed = idx.remove_rule(&lhs, &rhs);
                        }
                    }
                    drop(idx);
                    if synced || structural_removed {
                        if let Some(head) = lhs.get_head_symbol() {
                            if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                                self.shared.dispatch_overrides.note_user_rule_removed(id);
                            }
                        }
                    }
                }
            }
            // Invalidate evaluation caches — removing a rule changes which
            // rules fire at match time, so any memoized result referencing
            // this rule's head is stale. Same invalidation the non-rule
            // path performs at the end of this function.
            crate::backend::eval::trampoline::invalidate_normal_form_memo();
            crate::backend::eval::trampoline::clear_eval_memo();
            crate::backend::eval::trampoline::clear_match_result_cache();
            return;
        }

        // LSM-tiered base (Stage 5a): OVERLAY-FIRST removal — if a base is attached and the
        // OVERLAY holds no copy of this literal fact, suppress a base copy (tombstone)
        // instead of being a no-op. Overlay copies are removed first (the normal path
        // below); only an overlay miss tombstones the base. Gated internally on
        // `has_act_base`. See `act_tiered.rs::tombstone_on_remove`.
        if self.remove_overlay_miss_tombstone(value) {
            crate::backend::eval::trampoline::invalidate_normal_form_memo();
            crate::backend::eval::trampoline::clear_eval_memo();
            crate::backend::eval::trampoline::clear_match_result_cache();
            self.mark_modified();
            return;
        }

        // Non-rule: use literal encoding (existing path)
        match with_mork_bytes(
            value,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |mork_bytes| {
                let mut btm = self.shared.atom_space.btm.write();

                let current_count = get_multiplicity(&btm, mork_bytes);
                if current_count == 0 {
                    if !btm.contains(mork_bytes) {
                        return;
                    }
                    btm.remove(mork_bytes);
                    drop(btm);

                    // Incrementally remove from type/subtype dedicated PathMaps
                    if is_type_removal {
                        self.shared.atom_space.type_btm.write().remove(mork_bytes);
                        self.shared.atom_space.type_bloom.write().note_deletion();
                    } else if is_subtype_removal {
                        self.shared
                            .atom_space
                            .subtype_btm
                            .write()
                            .remove(mork_bytes);
                    }

                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_sub(1, Ordering::Relaxed);
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                    return;
                }

                let new_count = remove_atom(&mut btm, mork_bytes);

                if new_count == 0 {
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                }

                drop(btm);

                // Incrementally update type/subtype dedicated PathMaps
                if is_type_removal {
                    let mut type_btm = self.shared.atom_space.type_btm.write();
                    remove_atom(&mut type_btm, mork_bytes);
                    drop(type_btm);
                    self.shared.atom_space.type_bloom.write().note_deletion();
                } else if is_subtype_removal {
                    let mut subtype_btm = self.shared.atom_space.subtype_btm.write();
                    remove_atom(&mut subtype_btm, mork_bytes);
                }

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
            },
        ) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64): Wide MORK encoding
                let mut wide_key = Vec::new();
                encode_wide_storage(value, &mut wide_key);
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                let count = get_multiplicity(&wbtm, &wide_key);
                if count <= 1 {
                    wbtm.remove(&wide_key);
                } else {
                    remove_atom(&mut wbtm, &wide_key);
                }
                drop(wbtm);
                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
                self.shared
                    .atom_space
                    .head_arity_bloom
                    .write()
                    .note_deletion();
            }
        }

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
        // Check if this is a rule (= lhs rhs) — rules must use De Bruijn encoding
        // to be consistent with add_rule() which stores in RuleIndex + PathMap with De Bruijn.
        if let Some((_lhs, _rhs)) = extract_rule_parts(value) {
            let sm = self.shared_mapping.clone();
            match with_mork_query_bytes(value, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
                let mut btm = self.shared.atom_space.btm.write();
                add_atom(&mut btm, mork_bytes);
                drop(btm);

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);

                if let Some(head) = value.get_head_symbol() {
                    let arity = value.get_arity() as u8;
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .insert(head, arity);
                }
            }) {
                Ok(()) => {}
                Err(_) => {
                    // Fallback for large expressions (arity >= 64): Wide MORK encoding
                    let mut wide_key = Vec::new();
                    encode_wide_storage(value, &mut wide_key);
                    {
                        let mut wbtm = self.shared.atom_space.wide_btm.write();
                        add_atom(&mut wbtm, &wide_key);
                    }
                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            invalidate_space_mutation_caches();
            self.mark_modified();
            return;
        }

        // LSM-tiered base (Stage 5a): un-tombstone-on-add — revive a suppressed base copy
        // instead of an overlay write (interior-mutability twin of `add_to_space`). See
        // `act_tiered.rs::untombstone_on_add`.
        if self.untombstone_on_add(value) {
            invalidate_space_mutation_caches();
            self.mark_modified();
            return;
        }

        // Stage 1 (MM2 ProductZipper gate): non-rule var-containing fact in `btm`
        // makes the directional conjunction fast path incomplete — count it (see
        // `add_to_space` for rationale).
        if value.has_variables_fast() {
            self.shared
                .atom_space
                .variable_fact_count
                .fetch_add(1, Ordering::Relaxed);
        }

        // Non-rule: use literal encoding (existing path)
        match with_mork_bytes(
            value,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |mork_bytes| {
                let mut btm = self.shared.atom_space.btm.write();
                add_atom(&mut btm, mork_bytes);
                drop(btm);

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);

                // Use trait method for head symbol extraction
                if let Some(head) = value.get_head_symbol() {
                    let arity = value.get_arity() as u8;
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .insert(head, arity);
                }
            },
        ) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64): Wide MORK encoding
                let mut wide_key = Vec::new();
                encode_wide_storage(value, &mut wide_key);
                {
                    let mut wbtm = self.shared.atom_space.wide_btm.write();
                    add_atom(&mut wbtm, &wide_key);
                }
                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);

                if let Some(head) = value.get_head_symbol() {
                    let arity = value.get_arity() as u8;
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .insert(head, arity);
                }
            }
        }

        // Mark as modified for union() fast-path detection
        invalidate_space_mutation_caches();
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
        // Check if this is a rule (= lhs rhs) — rules are stored with De Bruijn encoding
        if let Some((lhs, rhs)) = extract_rule_parts(value) {
            let sm = self.shared_mapping.clone();
            // Capture full De Bruijn bytes for alpha-equivalent RuleIndex sync.
            let mut captured_bytes: Option<Vec<u8>> = None;
            match with_mork_query_bytes(value, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
                captured_bytes = Some(mork_bytes.to_vec());

                let mut btm = self.shared.atom_space.btm.write();

                let current_count = get_multiplicity(&btm, mork_bytes);
                if current_count == 0 {
                    if !btm.contains(mork_bytes) {
                        return;
                    }
                    btm.remove(mork_bytes);
                    drop(btm);
                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_sub(1, Ordering::Relaxed);
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                    self.mark_modified();
                    return;
                }

                let new_count = remove_atom(&mut btm, mork_bytes);

                if new_count == 0 {
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                }

                drop(btm);
                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
            }) {
                Ok(()) => {
                    // See `remove_from_space` for the `.is_some()` rationale —
                    // `Some(_)` means "the index was authoritatively updated"
                    // (whether decremented or fully removed). Pattern-matching
                    // for the `Some(true)` "fully removed" case alone would
                    // re-fire the structural fallback on a just-decremented
                    // multiplicity > 1 entry and corrupt it.
                    let mut idx = self.shared.rule_index.write();
                    let synced: bool = if let Some(ref bytes) = captured_bytes {
                        idx.remove_rule_by_debruijn(bytes).is_some()
                    } else {
                        false
                    };
                    let mut structural_removed = false;
                    if !synced {
                        let is_ground = !lhs.contains_variables() && !rhs.contains_variables();
                        debug_assert!(
                            is_ground || captured_bytes.is_some(),
                            "remove_from_space_shared: variable rule had no captured De Bruijn bytes"
                        );
                        if is_ground {
                            structural_removed = idx.remove_rule(&lhs, &rhs);
                        }
                    }
                    drop(idx);
                    if synced || structural_removed {
                        if let Some(head) = lhs.get_head_symbol() {
                            if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                                self.shared.dispatch_overrides.note_user_rule_removed(id);
                            }
                        }
                    }
                }
                Err(_) => {
                    // Fallback for large expressions (arity >= 64): Wide MORK encoding
                    let mut wide_key = Vec::new();
                    encode_wide_storage(value, &mut wide_key);
                    let mut wbtm = self.shared.atom_space.wide_btm.write();
                    let count = get_multiplicity(&wbtm, &wide_key);
                    if count <= 1 {
                        wbtm.remove(&wide_key);
                    } else {
                        remove_atom(&mut wbtm, &wide_key);
                    }
                    drop(wbtm);
                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_sub(1, Ordering::Relaxed);
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();

                    // Sync RuleIndex for wide rules via wide De Bruijn bytes.
                    // Same `.is_some()` rule as the narrow path.
                    let mut full_wide_ctx =
                        crate::backend::wide_mork::encoding::WideConversionContext::new();
                    let mut wide_full_debruijn = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_debruijn(
                        value,
                        &mut full_wide_ctx,
                        &mut wide_full_debruijn,
                    );
                    let mut idx = self.shared.rule_index.write();
                    let synced: bool = idx.remove_rule_by_debruijn(&wide_full_debruijn).is_some();
                    let mut structural_removed = false;
                    if !synced {
                        let is_ground = !lhs.contains_variables() && !rhs.contains_variables();
                        debug_assert!(
                            is_ground,
                            "remove_from_space_shared (wide path): variable rule not found via wide De Bruijn"
                        );
                        if is_ground {
                            structural_removed = idx.remove_rule(&lhs, &rhs);
                        }
                    }
                    drop(idx);
                    if synced || structural_removed {
                        if let Some(head) = lhs.get_head_symbol() {
                            if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                                self.shared.dispatch_overrides.note_user_rule_removed(id);
                            }
                        }
                    }
                }
            }
            // Invalidate evaluation caches — same rationale as remove_from_space.
            crate::backend::eval::trampoline::invalidate_normal_form_memo();
            crate::backend::eval::trampoline::clear_eval_memo();
            crate::backend::eval::trampoline::clear_match_result_cache();
            self.mark_modified();
            return;
        }

        // LSM-tiered base (Stage 5a): OVERLAY-FIRST removal. If a base is attached and the
        // OVERLAY holds no copy of this literal fact, the `remove-atom` must SUPPRESS a base
        // copy (tombstone) rather than be a no-op. Overlay copies are always removed first
        // (the normal path below); only an overlay miss falls through to tombstoning. Gated
        // internally on `has_act_base` (single relaxed load when untiered). See
        // `act_tiered.rs::tombstone_on_remove`.
        if self.remove_overlay_miss_tombstone(value) {
            crate::backend::eval::trampoline::invalidate_normal_form_memo();
            crate::backend::eval::trampoline::clear_eval_memo();
            crate::backend::eval::trampoline::clear_match_result_cache();
            self.mark_modified();
            return;
        }

        // Non-rule: use literal encoding (existing path)
        match with_mork_bytes(
            value,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |mork_bytes| {
                let mut btm = self.shared.atom_space.btm.write();

                let current_count = get_multiplicity(&btm, mork_bytes);
                if current_count == 0 {
                    if !btm.contains(mork_bytes) {
                        return;
                    }
                    btm.remove(mork_bytes);
                    drop(btm);
                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_sub(1, Ordering::Relaxed);
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                    self.mark_modified();
                    return;
                }

                let new_count = remove_atom(&mut btm, mork_bytes);

                if new_count == 0 {
                    self.shared
                        .atom_space
                        .head_arity_bloom
                        .write()
                        .note_deletion();
                }

                drop(btm);
                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
            },
        ) {
            Ok(()) => {}
            Err(_) => {
                // Fallback for large expressions (arity >= 64): Wide MORK encoding
                let mut wide_key = Vec::new();
                encode_wide_storage(value, &mut wide_key);
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                let count = get_multiplicity(&wbtm, &wide_key);
                if count <= 1 {
                    wbtm.remove(&wide_key);
                } else {
                    remove_atom(&mut wbtm, &wide_key);
                }
                drop(wbtm);
                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
                self.shared
                    .atom_space
                    .head_arity_bloom
                    .write()
                    .note_deletion();
            }
        }

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
    /// Uses generic functions that operate directly on V:
    /// - `mork_bytes_to_generic_value()` - MORK bytes → V
    /// - `pattern_match_generic()` - pattern matching on V
    /// - `apply_bindings_generic()` - template instantiation on V
    ///
    /// No intermediate `MettaValue` conversions occur in the hot path.
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

    /// Check if any rule for `head` (any arity) has an RHS body that
    /// contains the atom `target_atom` anywhere in its tree.
    ///
    /// Used to gate the bytecode VM path: the VM doesn't implement cut,
    /// so expressions whose rules use `(cut)` must go through the trampoline.
    ///
    /// Phase 11.A (2026-05-17) — O(1) via `PerHeadAtomIndex` in the
    /// rule index, populated incrementally at `add_rule` / `remove_rule`
    /// time. The legacy O(rules × rhs_size) scan was fatal under PLN's
    /// per-step gating from `expression_involves_impure_rules`
    /// (`eval/mod.rs:895`) where it was invoked once per eval-step × 11
    /// needles × every node of the expression tree. Removed the inner
    /// `contains_atom_recursive` walk entirely from the hot path; the
    /// recursive helper at `:3245` is now only used in tests.
    pub fn rule_rhs_contains_atom(&self, head: &str, target_atom: &str) -> bool {
        self.shared
            .rule_index
            .read()
            .rule_rhs_atoms
            .contains(head, target_atom)
    }

    /// Check if a (head, arity) pair may match a RULE definition (not data atoms).
    ///
    /// Uses a bloom filter populated only by `add_rule()`, not by `add-atom()`.
    /// This distinguishes data constructors like `(Type "$a")` from rule heads
    /// like `(treat_step $label)`. Used by `is_normal_form_bounded` to avoid
    /// sending data constructors through the full evaluation pipeline.
    #[inline]
    pub fn may_have_rule_head(&self, head: &str, arity: usize) -> bool {
        self.shared
            .atom_space
            .rule_head_bloom
            .read()
            .may_contain(head, arity as u8)
    }

    /// Match a CONJUNCTION of goals against `&self`'s space using MORK's `query_multi`
    /// / `ProductZipper` (an (n−1)-factor left-deep trie join), returning the set of
    /// consistent variable assignments — the MM2 fast path for `(, g0 g1 … gn)`.
    ///
    /// Returns `Some(bindings)` ONLY when the join is guaranteed COMPLETE; otherwise
    /// `None`, and the caller must fall back to the iterative bidirectional join. The
    /// completeness gate — `Some` requires ALL of:
    ///   * `goals` non-empty and `goals.len() < 63` (MORK 6-bit arity cap on the
    ///     synthesized `(, …)` wrapper, whose arity is `goals.len() + 1`);
    ///   * every goal is a head-applied expression (has a head symbol) whose head is
    ///     NOT `=` (rules are De-Bruijn-encoded variable atoms in `btm` that
    ///     directional matching would mishandle);
    ///   * `variable_fact_count == 0` (no variable-containing fact in `btm` that a
    ///     concrete goal position could directionally miss — see that field's doc);
    ///   * `variable_atoms` empty (the `SpaceHandle`-populated bidirectional set).
    ///
    /// Goals are wrapped in ONE `factory.conjunction(...)` so they share a single
    /// De-Bruijn `ConversionContext`: a variable recurring across goals (`$y` in
    /// `(parent $x $y)` and `(parent $y $z)`) becomes one trie variable and MORK
    /// enforces the join. Pattern variables resolve at namespace 0, so
    /// `mork_bindings_to_generic` extracts the full assignment (see
    /// `MORK/kernel/src/space.rs` `query_multi_raw`). Determinism is preserved — goals
    /// are passed in source order and all pruning is structural (byte-prefix), never
    /// cost-reordered, matching MM2's deterministic-scheduler guarantee.
    pub fn match_conjunction_query_multi(
        &self,
        goals: &[V],
    ) -> Option<Vec<crate::backend::models::GenericBindings<V>>> {
        // ── Completeness gate (return None → caller uses the iterative join) ──
        if goals.is_empty() || goals.len() >= 63 {
            return None;
        }
        for goal in goals {
            match goal.get_head_symbol() {
                Some("=") => return None, // rule head: De-Bruijn variable atom in btm
                Some(_) => {}
                None => return None, // bare variable / non-expression goal
            }
        }
        if self
            .shared
            .atom_space
            .variable_fact_count
            .load(Ordering::Acquire)
            != 0
        {
            return None;
        }
        if !self.shared.atom_space.variable_atoms.read().is_empty() {
            return None;
        }

        // ── Encode `(, g0 g1 … gn)` with one shared De-Bruijn context ──────
        let conj = self.factory.conjunction(goals.to_vec());
        let space = self.create_space();
        let sm = self.shared_mapping.clone();

        let result = crate::backend::mork_convert::with_mork_query_bytes(
            &conj,
            &sm,
            self.mork_cache_epoch,
            |conj_bytes, ctx| {
                let conj_expr = mork_expr::Expr {
                    ptr: conj_bytes.as_ptr().cast_mut(),
                };
                let mut out: Vec<crate::backend::models::GenericBindings<V>> = Vec::new();
                mork::space::Space::query_multi(&space.btm, conj_expr, |res, _matched| {
                    if let Err(mork_bindings) = res {
                        if let Ok(binds) =
                            crate::backend::mork_convert::mork_bindings_to_generic::<
                                V,
                                F,
                                Multiplicity,
                            >(&mork_bindings, ctx, &space, &self.factory)
                        {
                            out.push(binds);
                        }
                    }
                    true // collect ALL join solutions (no early termination here)
                });
                out
            },
        );

        match result {
            Ok(v) => Some(v),
            Err(_) => None, // encoding failure (e.g. arity overflow) → fall back
        }
    }

    /// Single-pattern trie-pruned match of `btm` via MORK `query_multi` (the MM2 fast
    /// path for `match_space`). The pattern is wrapped as a 1-conjunct `(, pattern)` (the
    /// form `query_multi` expects), so the `ProductZipper` descends only the matching
    /// trie branches — O(matches) instead of the linear O(|btm|) scan. Each match
    /// instantiates `template` with the namespace-0 bindings (extracted via
    /// `mork_bindings_to_generic`) and carries the matched fact's multiplicity.
    ///
    /// Returns `Some` ONLY when COMPLETE for the `btm` store; the CALLER must have
    /// established a ground space (no variable-containing facts — directional matching
    /// would otherwise miss a stored variable). Returns `None` on a non-head pattern,
    /// arity ≥ 64, or an encoding failure → caller falls back to the linear scan. Covers
    /// `btm` only; the caller still scans `wide_btm` for arity-≥64 facts.
    fn match_space_btm_query_multi(
        &self,
        pattern: &V,
        template: &V,
    ) -> Option<Vec<MultiplicityMatch<V>>> {
        let head = pattern.get_head_symbol()?;
        let arity = pattern.get_arity();
        if arity == 0 || arity >= 64 {
            return None;
        }
        // Rules `(= lhs rhs)` are the only De-Bruijn (variable-containing) atoms in `btm`
        // (added via `add_rule`, NOT counted by `variable_fact_count`). Directional
        // `query_multi` would mishandle a `=`-headed rule query, so fall back to the
        // linear+bidirectional scan for those. Non-`=` patterns only match literal facts
        // (which are all ground when the caller's `variable_fact_count == 0` gate holds).
        if head == "=" {
            return None;
        }
        if !self
            .shared
            .atom_space
            .head_arity_bloom
            .read()
            .may_contain(head, arity as u8)
        {
            return Some(Vec::new());
        }
        let conj = self.factory.conjunction(vec![pattern.clone()]);
        let space = self.create_space();
        let sm = self.shared_mapping.clone();
        let result = crate::backend::mork_convert::with_mork_query_bytes(
            &conj,
            &sm,
            self.mork_cache_epoch,
            |bytes, ctx| {
                let conj_expr = mork_expr::Expr {
                    ptr: bytes.as_ptr().cast_mut(),
                };
                let mut out: Vec<MultiplicityMatch<V>> = Vec::new();
                mork::space::Space::query_multi(&space.btm, conj_expr, |res, matched_expr| {
                    if let Err(mork_bindings) = res {
                        if let Ok(binds) =
                            crate::backend::mork_convert::mork_bindings_to_generic::<
                                V,
                                F,
                                Multiplicity,
                            >(&mork_bindings, ctx, &space, &self.factory)
                        {
                            let instantiated =
                                apply_bindings_generic(template, &binds, &self.factory);
                            // Multiplicity from the matched atom's exact PathMap key.
                            let first_byte = unsafe { *matched_expr.ptr };
                            let mult = if mork_expr::maybe_byte_item(first_byte).is_ok() {
                                let mork_bytes = unsafe { &*matched_expr.span() };
                                get_multiplicity(&space.btm, mork_bytes).max(1) as usize
                            } else {
                                1
                            };
                            out.push(MultiplicityMatch::new(instantiated, mult));
                        }
                    }
                    true // collect ALL matches
                });
                out
            },
        );
        result.ok()
    }

    /// Stage 2 — trie-pruned existence check of `btm` via MORK `query_multi` with
    /// EARLY TERMINATION: the streaming `FnMut -> bool` callback returns `false` on the
    /// first match, `longjmp`-ing out of the trie walk (O(depth) instead of the linear
    /// O(|btm|) scan — the win is largest when the match is absent or late in byte order).
    /// Same eligibility as `match_space_btm_query_multi` (ground space, non-`=` head,
    /// arity 1..=63); `Some(true)`=match exists, `Some(false)`=none in btm, `None`=fall
    /// back to the linear+bidirectional scan. Covers `btm` only.
    fn match_space_btm_exists_query_multi(&self, pattern: &V) -> Option<bool> {
        let head = pattern.get_head_symbol()?;
        let arity = pattern.get_arity();
        if arity == 0 || arity >= 64 || head == "=" {
            return None;
        }
        if !self
            .shared
            .atom_space
            .head_arity_bloom
            .read()
            .may_contain(head, arity as u8)
        {
            return Some(false);
        }
        let conj = self.factory.conjunction(vec![pattern.clone()]);
        let space = self.create_space();
        let sm = self.shared_mapping.clone();
        let result = crate::backend::mork_convert::with_mork_query_bytes(
            &conj,
            &sm,
            self.mork_cache_epoch,
            |bytes, _ctx| {
                let conj_expr = mork_expr::Expr {
                    ptr: bytes.as_ptr().cast_mut(),
                };
                let mut found = false;
                mork::space::Space::query_multi(&space.btm, conj_expr, |res, _matched| {
                    if res.is_err() {
                        found = true;
                        false // first match — stop the trie walk (early termination)
                    } else {
                        true
                    }
                });
                found
            },
        );
        result.ok()
    }

    pub fn match_space(&self, pattern: &V, template: &V) -> Vec<MultiplicityMatch<V>> {
        // Bloom filter check using trait methods (no conversion). The bloom tracks ONLY
        // overlay (in-memory) facts, so a definite-miss may still match the attached ACT
        // BASE — only short-circuit the empty return when NO base is attached. The
        // `has_act_base` load is reached only on the (rare) bloom-miss branch, so the hot
        // "bloom passes" path is byte-identical to before tiering.
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self
                .shared
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(expected_head, pattern_arity);
            if !bloom_result && !self.shared.atom_space.has_act_base.load(Ordering::Acquire) {
                return Vec::new();
            }
        }

        // T03/050 (spec §04.1 + HE-bisim): `(match &self pattern template)`
        // must unify the pattern with each stored atom *bidirectionally* — a
        // free variable in the *stored* atom can bind against a concrete atom
        // (or another variable) in the pattern. The unidirectional
        // `pattern_match_generic` only binds pattern-side variables, so a
        // fact like `(rule (condition $c) (action $a))` queried with
        // `(rule (condition C) (action $A))` would silently return zero
        // matches. We use `space_match_bidirectional_generic` (with stored
        // atoms freshened to prevent variable capture) whenever the stored
        // atom has variables; ground atoms keep the cheap unidirectional
        // fast path. Mirrors `SpaceHandle::match_pattern_generic`
        // (`src/backend/models/space_handle.rs:869`) for owned spaces.
        use crate::backend::eval::bindings::collect_variables_generic;
        use crate::backend::eval::freshening::freshen_variables_generic;
        use crate::backend::eval::space_match::space_match_bidirectional_generic;
        let pattern_vars = collect_variables_generic(pattern);

        let mut results = Vec::new();

        // Stage 1b: MM2 trie-pruned fast path for the `btm` store. When the space holds
        // NO variable-containing facts (`variable_fact_count == 0`) and no SpaceHandle
        // variable atoms, MORK `query_multi` descends only matching trie branches
        // (O(matches)) instead of the linear O(|btm|) scan below — turning match-heavy
        // workloads over large ground KBs from O(|space|·queries) into O(matches·queries).
        // Not eligible / not ground ⇒ fall through to the linear + bidirectional scan
        // (which is required to bind variables in *stored* atoms).
        let ground_space = self
            .shared
            .atom_space
            .variable_fact_count
            .load(Ordering::Acquire)
            == 0
            && self.shared.atom_space.variable_atoms.read().is_empty();
        let btm_done = if ground_space {
            match self.match_space_btm_query_multi(pattern, template) {
                Some(fast) => {
                    results = fast;
                    true
                }
                None => false,
            }
        } else {
            false
        };

        if !btm_done {
            let space = self.create_space();
            let mut rz = space.btm.read_zipper();

            // Iterate through MORK PathMap (linear fallback)
            while rz.to_next_val() {
                let path_bytes = rz.path();
                let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1) as usize;

                // Direct MORK bytes → V conversion (no MettaValue intermediate)
                if let Ok(atom) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                    path_bytes,
                    &space,
                    &self.factory,
                ) {
                    if atom.has_variables_fast() {
                        // Stored atom has variables — bidirectional match with
                        // freshening (HE-bisim for variable-containing facts).
                        let freshened = freshen_variables_generic(&atom, &self.factory);
                        if let Some(bindings) =
                            space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                        {
                            let instantiated =
                                apply_bindings_generic(template, &bindings, &self.factory);
                            results.push(MultiplicityMatch::new(instantiated, multiplicity));
                        }
                    } else if let Some(bindings) = pattern_match_generic(pattern, &atom) {
                        // Ground atom — fast unidirectional path.
                        let instantiated =
                            apply_bindings_generic(template, &bindings, &self.factory);
                        results.push(MultiplicityMatch::new(instantiated, multiplicity));
                    }
                }
            }

            drop(space);
        }

        // Check wide expression PathMap (arity >= 64, Wide MORK keys)
        {
            let wbtm = self.shared.atom_space.wide_btm.read();
            let mut rz = wbtm.read_zipper();
            while rz.to_next_val() {
                let path_bytes = rz.path();
                let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1) as usize;
                if let Ok(atom) = wide_bytes_to_generic_value::<V, F>(path_bytes, &self.factory) {
                    if atom.has_variables_fast() {
                        let freshened = freshen_variables_generic(&atom, &self.factory);
                        if let Some(bindings) =
                            space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                        {
                            let instantiated =
                                apply_bindings_generic(template, &bindings, &self.factory);
                            results.push(MultiplicityMatch::new(instantiated, multiplicity));
                        }
                    } else if let Some(bindings) = pattern_match_generic(pattern, &atom) {
                        let instantiated =
                            apply_bindings_generic(template, &bindings, &self.factory);
                        results.push(MultiplicityMatch::new(instantiated, multiplicity));
                    }
                }
            }
        }

        // LSM-tiered base (Stage 5a "next layer"): when an out-of-core ACT base is
        // attached, the matches above are the in-memory OVERLAY; append the base matches
        // MINUS tombstone suppression (`base − tombstones`), OVERLAY-FIRST. The
        // `has_act_base` gate is a single relaxed-acquire load — the no-base hot path is a
        // predictable-false branch and is otherwise byte-identical to before tiering
        // (Hard Constraint: zero hot-path regression). `match_space_base` re-opens the
        // mmap per query (the proven `query_act` I/O pattern) and is fully generic, so this
        // works for the sole concrete `V = MettaValue` instantiation that a base can attach
        // to. Bag multiplicity is preserved (each `MultiplicityMatch` carries its count).
        if self.shared.atom_space.has_act_base.load(Ordering::Acquire) {
            results.extend(self.match_space_base(pattern, template));
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
        // Bloom filter check (overlay-only) — skip the definite-miss short-circuit when an
        // ACT base is attached, since the base may match a head absent from the overlay
        // bloom. The `has_act_base` load is reached only on the bloom-miss branch, so the
        // hot "bloom passes" path is unchanged. See `match_space` for the rationale.
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self
                .shared
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(expected_head, pattern_arity)
                && !self.shared.atom_space.has_act_base.load(Ordering::Acquire)
            {
                return false;
            }
        }

        // T03/050 consistency: existence check must agree with `match_space`
        // about which stored atoms "match" — bidirectional unification when
        // the stored atom has variables, unidirectional fast path otherwise.
        use crate::backend::eval::bindings::collect_variables_generic;
        use crate::backend::eval::freshening::freshen_variables_generic;
        use crate::backend::eval::space_match::space_match_bidirectional_generic;
        let pattern_vars = collect_variables_generic(pattern);

        // Stage 2: trie-pruned existence early-exit for ground spaces (non-`=` patterns).
        // `query_multi` stops at the first match (O(depth)) vs the linear scan below.
        let ground_space = self
            .shared
            .atom_space
            .variable_fact_count
            .load(Ordering::Acquire)
            == 0
            && self.shared.atom_space.variable_atoms.read().is_empty();
        let btm_checked_fast = if ground_space {
            match self.match_space_btm_exists_query_multi(pattern) {
                Some(true) => return true,
                Some(false) => true, // fast path ran; no btm match → skip linear btm scan
                None => false,
            }
        } else {
            false
        };

        if !btm_checked_fast {
            let space = self.create_space();
            let mut rz = space.btm.read_zipper();

            while rz.to_next_val() {
                let path_bytes = rz.path();

                // Direct MORK bytes → V conversion (no MettaValue intermediate)
                if let Ok(atom) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                    path_bytes,
                    &space,
                    &self.factory,
                ) {
                    if atom.has_variables_fast() {
                        let freshened = freshen_variables_generic(&atom, &self.factory);
                        if space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                            .is_some()
                        {
                            return true;
                        }
                    } else if pattern_match_generic(pattern, &atom).is_some() {
                        return true;
                    }
                }
            }

            drop(space);
        }

        // Check wide expression PathMap (arity ≥ 64, Wide MORK encoding)
        {
            let wbtm = self.shared.atom_space.wide_btm.read();
            let mut wrz = wbtm.read_zipper();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                if let Ok(atom) = wide_bytes_to_generic_value::<V, F>(path_bytes, &self.factory) {
                    if atom.has_variables_fast() {
                        let freshened = freshen_variables_generic(&atom, &self.factory);
                        if space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                            .is_some()
                        {
                            return true;
                        }
                    } else if pattern_match_generic(pattern, &atom).is_some() {
                        return true;
                    }
                }
            }
        }

        // LSM-tiered base: the overlay (in-memory btm/wide_btm) did not match — consult the
        // attached ACT base MINUS tombstones. Existence-only (early-exit on first surviving
        // base match), so it agrees with the augmented `match_space`. Gated by the relaxed-
        // acquire `has_act_base` load → no-base hot path stays byte-identical.
        if self.shared.atom_space.has_act_base.load(Ordering::Acquire)
            && self.match_space_base_exists(pattern)
        {
            return true;
        }

        false
    }

    /// Get all atoms from the Space (MORK PathMap + Wide MORK PathMap).
    ///
    /// This iterates the same data as `match_space()` but without pattern filtering,
    /// returning every stored atom as-is.
    ///
    /// ## Zero-Conversion Architecture
    ///
    /// Uses `mork_bytes_to_generic_value()` for MORK bytes → V conversion,
    /// and `wide_bytes_to_generic_value()` for Wide MORK bytes → V conversion.
    pub fn get_all_atoms(&self) -> Vec<V> {
        let space = self.create_space();
        let mut rz = space.btm.read_zipper();
        let mut atoms = Vec::new();

        // Iterate through MORK PathMap
        while rz.to_next_val() {
            let path_bytes = rz.path();
            if let Ok(atom) =
                mork_bytes_to_generic_value::<V, F, Multiplicity>(path_bytes, &space, &self.factory)
            {
                atoms.push(atom);
            }
        }

        drop(space);

        // Include wide expression PathMap (arity ≥ 64, Wide MORK encoding)
        {
            let wbtm = self.shared.atom_space.wide_btm.read();
            let mut wrz = wbtm.read_zipper();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                if let Ok(atom) = wide_bytes_to_generic_value::<V, F>(path_bytes, &self.factory) {
                    atoms.push(atom);
                }
            }
        }

        // Include variable atoms (stored separately from PathMap because
        // MORK can't pattern-match on variable atoms)
        {
            let var_atoms = self.shared.atom_space.variable_atoms.read();
            for (atom, _mult) in var_atoms.iter() {
                let freshened = crate::backend::eval::freshening::freshen_variables_generic(
                    atom,
                    &self.factory,
                );
                atoms.push(freshened);
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
pub type MettaEnvironment = GenericEnvironment<MettaValue, crate::backend::models::ActiveFactory>;

impl Default for MettaEnvironment {
    fn default() -> Self {
        GenericEnvironment::new(crate::backend::models::active_factory())
    }
}

/// Recursively check if a MettaValue tree contains a specific atom name.
/// Used by `rule_rhs_contains_atom` to detect `(cut)` in rule bodies.
fn contains_atom_recursive<V: MettaValueTrait>(value: &V, target: &str) -> bool {
    if let Some(name) = value.as_atom() {
        return name == target;
    }
    if let Some(items) = value.as_sexpr() {
        return items
            .iter()
            .any(|item| contains_atom_recursive(item, target));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    static NORMAL_FORM_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn space_add_mutations_invalidate_normal_form_bloom() {
        let _serial = NORMAL_FORM_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use crate::backend::eval::trampoline::{
            clear_normal_form_memo_for_new_query, is_memoized_normal_form, memoize_normal_form,
        };

        let probe = MettaValue::SExpr(vec![
            MettaValue::Atom("__nf_probe_after_space_add__".to_string()),
            MettaValue::Atom("payload".to_string()),
        ]);
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("__space_fact__".to_string()),
            MettaValue::Atom("a".to_string()),
        ]);

        clear_normal_form_memo_for_new_query();
        memoize_normal_form(&probe);
        assert!(
            is_memoized_normal_form(&probe),
            "test setup should install the probe in the normal-form bloom"
        );

        let mut env: MettaEnvironment = MettaEnvironment::default();
        env.add_to_space(&fact);
        assert!(
            !is_memoized_normal_form(&probe),
            "regular atom-space add must invalidate normal-form bloom entries"
        );

        memoize_normal_form(&probe);
        assert!(
            is_memoized_normal_form(&probe),
            "test setup should reinstall the probe before the shared add"
        );
        env.add_to_space_shared(&fact);
        assert!(
            !is_memoized_normal_form(&probe),
            "shared atom-space add must invalidate normal-form bloom entries"
        );
    }

    // ── Stage 1: MM2 ProductZipper conjunctive join ──────────────────────
    // Empirically validates the binding reconciliation: a `(, (parent $x $y)
    // (parent $y $z))` join over ground facts must return the single chained
    // solution {$x=a, $y=b, $z=c}, with pattern variables resolved at MORK
    // namespace 0 via the shared De-Bruijn context.
    #[test]
    fn test_match_conjunction_query_multi_two_goal_join() {
        let mut env: MettaEnvironment = MettaEnvironment::default();
        let parent = |a: &str, b: &str| {
            MettaValue::SExpr(vec![
                MettaValue::Atom("parent".to_string()),
                MettaValue::Atom(a.to_string()),
                MettaValue::Atom(b.to_string()),
            ])
        };
        env.add_to_space(&parent("a", "b"));
        env.add_to_space(&parent("b", "c"));

        let g0 = MettaValue::SExpr(vec![
            MettaValue::Atom("parent".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let g1 = MettaValue::SExpr(vec![
            MettaValue::Atom("parent".to_string()),
            MettaValue::Atom("$y".to_string()),
            MettaValue::Atom("$z".to_string()),
        ]);

        let result = env
            .match_conjunction_query_multi(&[g0, g1])
            .expect("ground-fact, non-=-headed goals must take the ProductZipper fast path");

        assert_eq!(
            result.len(),
            1,
            "expected exactly one join solution, got {:?}",
            result
        );
        let b = &result[0];
        assert_eq!(b.get("$x").and_then(|v| v.as_atom()), Some("a"));
        assert_eq!(b.get("$y").and_then(|v| v.as_atom()), Some("b"));
        assert_eq!(b.get("$z").and_then(|v| v.as_atom()), Some("c"));
    }

    // The completeness gate must fall back to `None` whenever the conjunction
    // join could be incomplete (so the iterative bidirectional path is used).
    #[test]
    fn test_match_conjunction_query_multi_completeness_gate() {
        let mut env: MettaEnvironment = MettaEnvironment::default();
        let g = MettaValue::SExpr(vec![
            MettaValue::Atom("parent".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);

        // Empty goal list → None.
        assert!(env.match_conjunction_query_multi(&[]).is_none());

        // `=`-headed goal (rules are De-Bruijn variable atoms in btm) → None.
        let eq_goal = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        assert!(env.match_conjunction_query_multi(&[eq_goal]).is_none());

        // With only ground facts, the single-goal conjunction takes the fast path.
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("parent".to_string()),
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]));
        assert!(env
            .match_conjunction_query_multi(std::slice::from_ref(&g))
            .is_some());

        // After adding a VARIABLE-containing fact, the directional fast path could
        // miss it, so the gate must fall back.
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("parent".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("z".to_string()),
        ]));
        assert!(
            env.match_conjunction_query_multi(std::slice::from_ref(&g))
                .is_none(),
            "variable-containing fact in btm must force fallback"
        );
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
