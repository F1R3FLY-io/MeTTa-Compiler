//! Rule management operations for Environment.
//!
//! Provides methods for adding, indexing, and querying rules.
//! Rules are stored as (= lhs rhs) in MORK PathMap as De Bruijn-encoded MORK bytes.
//!
//! # Multiplicity Tracking
//!
//! Rules can be defined multiple times, and we track multiplicities efficiently
//! using PathMap<Multiplicity> — each MORK byte key maps to its multiplicity count.
//!
//! # Rule Discovery
//!
//! Rules are discovered via a two-level index:
//!
//! 1. **Bloom filter** — O(1) rejection for non-matching head/arity combinations
//! 2. **RuleIndex** — HashMap-backed `(head, arity) → Vec<RuleEntry>` for O(1) candidate lookup
//! 3. **MORK `extract_data()`** — O(n) byte-level structural pattern matching per candidate
//! 4. **MettaValue binding application** — `apply_bindings_generic()` on cached RHS template
//!
//! Rules are stored in PathMap with De Bruijn encoding (via `with_mork_query_bytes`).
//! The RuleIndex caches De Bruijn bytes and metadata at insertion time for zero-deserialization
//! matching. Only the final matched result is deserialized to MettaValue.

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;

use crate::backend::hash_utils::FxBuildHasher;

/// Global epoch counter for rule/type mutations.
///
/// Incremented on `add_rule()`, `add_type_generic()`, and `remove_type_generic()`.
/// Used by `ExprCompilationState` to cache `TypeSignatureRegistry` across JIT
/// entries — the registry is rebuilt only when the epoch changes.
pub static RULE_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Increment the global rule/type epoch counter.
///
/// Must be called after any mutation that could change type signatures:
/// adding/removing rules, adding/removing type declarations.
#[inline]
pub fn increment_rule_epoch() {
    RULE_EPOCH.fetch_add(1, Ordering::Release);
}

use mork::space::Space;
use mork_expr::{maybe_byte_item, Expr, ExprZipper, Tag};
// Disabled: PathMap no longer directly constructed in this module — add_rules_bulk now
// delegates to add_rule() for consistent De Bruijn encoding.
// use pathmap::PathMap;
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
use smallvec::SmallVec;
use tracing::trace;

thread_local! {
    /// Reusable buffer for MORK-serialized expressions in `match_rules_native`.
    /// Grows as needed but is never freed — amortized zero allocation after warmup.
    static MATCH_EXPR_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(256));

    /// Reusable ExprZipper for `extract_data()` calls in `match_rules_native`.
    /// The `trace` Vec capacity grows monotonically — `reset()` uses `set_len(0)` to
    /// clear without deallocating, so after warmup all match attempts are zero-alloc.
    /// Pre-allocated with capacity 16 for typical PLN expression depths (6-8).
    static MATCH_ZIPPER: RefCell<ExprZipper> = RefCell::new({
        let trace = Vec::with_capacity(16);
        // ExprZipper::new() would push a Breadcrumb based on the root's first byte,
        // but with a null root, we skip that and just set up the capacity.
        // The actual root and trace initialization happen via reset() before each use.
        ExprZipper { root: Expr { ptr: std::ptr::null_mut() }, loc: 0, trace }
    });

    /// Thread-local MORK serialization cache for `match_rules_native`.
    ///
    /// Content-hash keyed: `u64` (xxh3 content hash of MettaValue). This survives GC
    /// safepoints (no pointer ABA issue) and shares entries across structurally-identical
    /// expressions at different slab addresses. Value includes the arity for cheap
    /// collision validation.
    ///
    /// 8192 entries × ~64 bytes avg = ~512 KB per thread. LRU eviction bounds memory.
    /// Increased from 2048 to improve hit rate for PLN's working set (>2048 distinct exprs).
    static MORK_BYTES_CACHE: RefCell<LruCache<u64, (Vec<u8>, usize), FxBuildHasher>> =
        RefCell::new(LruCache::with_hasher(NonZeroUsize::new(8192).expect("non-zero"), FxBuildHasher));
}

/// Clear the thread-local MORK serialization cache.
///
/// With content-hash keying, this is only needed when the SharedMapping changes
/// (different symbol table = different interned IDs for same bytes). GC safepoints
/// no longer require clearing since keys are content hashes, not pointers.
pub fn clear_mork_bytes_cache() {
    MORK_BYTES_CACHE.with(|c| c.borrow_mut().clear());
}

use super::generic::GenericEnvironment;
use super::mork_encoding::{mork_bytes_to_generic_value, mork_expr_byte_len};
// Disabled: mork_expr_to_generic_value no longer used directly — deserialization happens via
// mork_bytes_to_generic_value for individual binding bytes.
// use super::mork_encoding::mork_expr_to_generic_value;
use super::multiplicity::{
    decrement_multiplicity, get_multiplicity, increment_multiplicity,
    Multiplicity,
};
use super::{MettaEnvironment, MettaValue};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait, ValueView};
use crate::backend::mork_convert::{with_mork_bytes, with_mork_query_bytes};

/// Extract (lhs, rhs) from a deserialized rule value `(= lhs rhs)`.
///
/// Returns `Some((lhs, rhs))` if the value is an s-expression with 3 elements
/// where the first element is the atom `"="`.
pub(crate) fn extract_rule_parts<V: MettaValueTrait + Clone>(value: &V) -> Option<(V, V)> {
    let children = value.as_sexpr()?;
    if children.len() == 3 {
        if let Some(op) = children[0].as_atom() {
            if op == "=" {
                return Some((children[1].clone(), children[2].clone()));
            }
        }
    }
    None
}

// ============================================================================
// RuleIndex — In-memory index for O(1) rule lookup + byte-level matching
// ============================================================================

/// Result of a native rule match via `match_rules_native()`.
///
/// Contains the instantiated RHS (bindings applied), the original RHS template
/// (for bytecode compilation caching), and named bindings (for bytecode VM stack frames).
#[derive(Debug, Clone)]
pub struct RuleMatchResult<V: MettaValueTrait + Clone> {
    /// RHS with bindings applied (for trampoline evaluation)
    pub instantiated_rhs: V,
    /// Original RHS template with original variable names (for bytecode compilation caching)
    pub rhs_template: V,
    /// Named bindings ($x -> value, for bytecode VM stack frames)
    pub bindings: GenericBindings<V>,
    /// How many times this rule was defined (multiplicity)
    pub multiplicity: u64,
    /// Phase 8.7: Cached return type of the RHS (from RuleEntry).
    /// Used for branch pruning when `expected_type` is set.
    pub rhs_type: Option<V>,
    /// Whether the RHS template contains variables, computed once at rule insertion time.
    /// When `false`, `apply_bindings_generic` can be skipped entirely (O(1) clone).
    pub rhs_has_variables: bool,
}

/// A single rule entry in the RuleIndex.
///
/// Caches both the original MettaValues (for display, debugging, bytecode VM) and the
/// De Bruijn-encoded bytes (for MORK `extract_data()` byte-level matching).
#[derive(Debug, Clone)]
pub(crate) struct RuleEntry<V: MettaValueTrait + Clone> {
    // --- Cached MettaValues (original variable names) ---
    /// LHS pattern with original variable names (for display, debugging)
    pub lhs: V,
    /// RHS template with original variable names (for bytecode compilation/caching)
    pub rhs: V,

    // --- De Bruijn bytes (for extract_data matching) ---
    /// LHS with NewVar/VarRef De Bruijn encoding (extracted from PathMap key).
    /// Empty for wide rules (arity ≥ 64) — use `lhs_wide_debruijn` instead.
    pub lhs_debruijn: Vec<u8>,

    /// LHS with Wide MORK De Bruijn encoding (tag-byte + LEB128, no arity limit).
    /// Empty for narrow rules (arity < 64) — use `lhs_debruijn` instead.
    /// Populated when MORK encoding fails due to arity ≥ 64.
    pub lhs_wide_debruijn: Vec<u8>,

    // --- Metadata ---
    /// De Bruijn index → original variable name (e.g., "$x", "$y")
    /// Only contains variables from LHS (used for building named bindings).
    /// Interned as `&'static str` via slab allocator to avoid per-match String clones.
    pub var_names: Vec<&'static str>,
    /// Indices of `_` wildcards (skip these in named bindings)
    pub wildcard_indices: SmallVec<[u8; 4]>,
    // NOTE: The old `specificity` field (count of NewVar tags) was removed.
    // MeTTa HE has NO specificity filter — all matching rules fire nondeterministically.
    // The old filter dropped structurally-more-specific rules when a variable-only rule
    // happened to have fewer NewVar tags (e.g. `(f ($c $tv) $y)` with 3 vars beat
    // `(f ((Implication $A $B) $TV) $Y)` with 4 vars despite the latter being more specific).
    /// How many times this rule was added (synced with PathMap multiplicity)
    pub multiplicity: u64,
    /// Cached return type of the RHS, computed once at insertion time.
    /// Used by Phase 8 optimizations for rule pre-filtering by expected type.
    /// `None` if RHS type couldn't be inferred (e.g., variable RHS, untyped operators).
    pub rhs_type: Option<V>,
    /// Cached result of `rhs.contains_variables()`, computed once at insertion time.
    /// When `false`, `apply_bindings` can skip the RHS entirely (O(1) clone).
    pub rhs_has_variables: bool,
    /// Compiled structural matcher for direct MettaValue pattern matching.
    /// `Some` for rules with structurally-matchable LHS (>95% of rules).
    /// `None` for rules too complex for structural matching — falls back to MORK.
    pub structural_matcher: Option<StructuralMatcher>,
}

/// Extract the head symbol of a value's first argument (for second-level rule indexing).
///
/// For an S-expression `(f (Implication $A $B) ...)`, returns `Some("Implication")`.
/// For `(f $x ...)` (variable first arg) or atoms, returns `None`.
///
/// Used at both `add_rule()` time (to index the LHS pattern's first arg) and at
/// query time (to narrow candidates in `get_candidates()`).
#[inline]
pub(crate) fn get_first_arg_head<V: MettaValueTrait>(value: &V) -> Option<&str> {
    let items = value.as_sexpr()?;
    if items.len() < 2 {
        return None; // No arguments (head-only S-expression)
    }
    items[1].get_head_symbol()
}

/// Second-level index group for rules sharing the same `(head, arity)`.
///
/// Partitions rules by the first argument's head symbol for O(1) candidate
/// narrowing. For PLN, many rules share the same head (e.g., `init-sentence`,
/// `|-`); this reduces the inner MORK `extract_data` loop by 3-10x.
///
/// ## Layout
///
/// - `by_first_arg_head`: Rules where the LHS first argument is an S-expression
///   with a concrete head symbol. Keyed by that symbol (interned `&'static str`).
/// - `variable_first_arg`: Rules where the LHS first argument is a variable (`$x`),
///   wildcard (`_`), or non-S-expression. These must be included in all queries
///   since they match any first argument.
#[derive(Debug, Clone)]
struct RuleGroup<V: MettaValueTrait + Clone> {
    /// Rules indexed by first argument's head symbol.
    by_first_arg_head: HashMap<&'static str, Vec<RuleEntry<V>>>,
    /// Rules with variable/wildcard/non-S-expression first argument.
    /// Always included in query results since they match any first argument.
    variable_first_arg: Vec<RuleEntry<V>>,
}

impl<V: MettaValueTrait + Clone> RuleGroup<V> {
    fn new() -> Self {
        RuleGroup {
            by_first_arg_head: HashMap::new(),
            variable_first_arg: Vec::new(),
        }
    }

    /// Get the appropriate entry list for inserting a rule with the given first-arg head.
    fn entries_for_mut(&mut self, first_arg_head: Option<&'static str>) -> &mut Vec<RuleEntry<V>> {
        match first_arg_head {
            Some(fah) => self.by_first_arg_head.entry(fah).or_insert_with(Vec::new),
            None => &mut self.variable_first_arg,
        }
    }

    /// Get candidates matching the given first-arg head.
    /// Returns first-arg-specific rules chained with variable-first-arg rules.
    fn get_candidates(&self, first_arg_head: Option<&str>) -> RuleGroupIter<'_, V> {
        let specific = match first_arg_head {
            Some(fah) => {
                use crate::backend::models::gc_allocator::global_allocator;
                let interned = global_allocator().alloc_str(fah);
                self.by_first_arg_head.get(interned).map(|v| v.as_slice()).unwrap_or(&[])
            }
            None => &[], // No first-arg head known — only variable_first_arg rules apply
        };
        RuleGroupIter {
            specific: specific.iter(),
            variable: self.variable_first_arg.iter(),
            // When querying without first_arg_head, we must include ALL rules
            // (both specific and variable) since any could match
            all_specific: if first_arg_head.is_none() {
                Some(self.by_first_arg_head.values())
            } else {
                None
            },
            current_all_specific: None,
        }
    }

    /// Iterate over ALL rules in this group (for remove_rule, len, etc.)
    fn all_entries(&self) -> impl Iterator<Item = &RuleEntry<V>> {
        self.by_first_arg_head.values().flat_map(|v| v.iter())
            .chain(self.variable_first_arg.iter())
    }

    /// Iterate over ALL rules mutably (for increment_multiplicity)
    fn all_entries_mut(&mut self) -> impl Iterator<Item = &mut RuleEntry<V>> {
        self.by_first_arg_head.values_mut().flat_map(|v| v.iter_mut())
            .chain(self.variable_first_arg.iter_mut())
    }

    /// Total number of rules in this group.
    fn len(&self) -> usize {
        self.by_first_arg_head.values().map(|v| v.len()).sum::<usize>()
            + self.variable_first_arg.len()
    }

    /// Remove a rule by position. Returns true if entry was fully removed.
    fn remove_rule(&mut self, lhs: &V, rhs: &V) -> Option<bool> {
        // Search in first-arg-indexed buckets
        for entries in self.by_first_arg_head.values_mut() {
            if let Some(pos) = entries.iter().position(|e| &e.lhs == lhs && &e.rhs == rhs) {
                if entries[pos].multiplicity > 1 {
                    entries[pos].multiplicity -= 1;
                    return Some(false);
                } else {
                    entries.remove(pos);
                    return Some(true);
                }
            }
        }
        // Search in variable-first-arg list
        if let Some(pos) = self.variable_first_arg.iter().position(|e| &e.lhs == lhs && &e.rhs == rhs) {
            if self.variable_first_arg[pos].multiplicity > 1 {
                self.variable_first_arg[pos].multiplicity -= 1;
                return Some(false);
            } else {
                self.variable_first_arg.remove(pos);
                return Some(true);
            }
        }
        None // Not found in this group
    }
}

/// Iterator over candidates in a RuleGroup.
///
/// When `first_arg_head` was provided: yields specific matches + variable_first_arg.
/// When `first_arg_head` was None: yields ALL rules (all specific buckets + variable_first_arg).
struct RuleGroupIter<'a, V: MettaValueTrait + Clone> {
    specific: std::slice::Iter<'a, RuleEntry<V>>,
    variable: std::slice::Iter<'a, RuleEntry<V>>,
    /// When querying without first_arg_head, iterate over ALL specific buckets
    all_specific: Option<std::collections::hash_map::Values<'a, &'static str, Vec<RuleEntry<V>>>>,
    current_all_specific: Option<std::slice::Iter<'a, RuleEntry<V>>>,
}

impl<'a, V: MettaValueTrait + Clone> Iterator for RuleGroupIter<'a, V> {
    type Item = &'a RuleEntry<V>;

    fn next(&mut self) -> Option<Self::Item> {
        // 1. Yield from specific matches first
        if let Some(item) = self.specific.next() {
            return Some(item);
        }
        // 2. If querying without first_arg_head, drain all specific buckets
        if let Some(ref mut all_vals) = self.all_specific {
            loop {
                if let Some(ref mut current) = self.current_all_specific {
                    if let Some(item) = current.next() {
                        return Some(item);
                    }
                }
                match all_vals.next() {
                    Some(bucket) => self.current_all_specific = Some(bucket.iter()),
                    None => {
                        self.all_specific = None;
                        break;
                    }
                }
            }
        }
        // 3. Yield from variable-first-arg rules
        self.variable.next()
    }
}

/// Lightweight in-memory index for O(1) rule lookup + MORK byte-level matching.
///
/// Populated at `add_rule()` time. Authoritative source for rule queries.
/// PathMap remains the storage-of-record (for `match_space`, serialization).
///
/// ## Indexing Strategy
///
/// Rules are indexed by `(head_symbol, arity)` for O(1) first-level lookup, then
/// further partitioned by first argument's head symbol for O(1) second-level
/// narrowing. For PLN workloads, this reduces the candidate set by 3-10x since
/// many rules share the same head (e.g., `init-sentence`, `|-`).
///
/// Rules with non-S-expression LHS (e.g., `(= $x $x)`) are stored in a separate
/// `wildcard` vec and included in all query results since they can match any expression.
///
/// ## Duplicate Detection
///
/// When `add_rule()` is called with the same `(lhs, rhs)` (by `PartialEq`),
/// the existing entry's multiplicity is incremented rather than creating a duplicate.
#[derive(Debug, Clone)]
pub(crate) struct RuleIndex<V: MettaValueTrait + Clone> {
    /// Rules indexed by (head_symbol, arity) → RuleGroup (with second-level first-arg indexing).
    /// Head symbols are interned as `&'static str` via the slab allocator for zero-alloc lookups.
    by_head_arity: HashMap<(&'static str, usize), RuleGroup<V>>,

    /// Rules with non-S-expression LHS (atoms, variables like `$x`).
    /// Always included in query results since they can match any expression.
    wildcard: Vec<RuleEntry<V>>,
}

impl<V: MettaValueTrait + Clone> RuleIndex<V> {
    /// Create a new empty RuleIndex.
    pub fn new() -> Self {
        RuleIndex {
            by_head_arity: HashMap::new(),
            wildcard: Vec::new(),
        }
    }

    /// Insert a rule entry, or increment multiplicity if a duplicate exists.
    ///
    /// Duplicate detection uses `PartialEq` on `(lhs, rhs)` MettaValues.
    /// If `head` is `Some`, indexes by `(head, arity)` with second-level
    /// first-arg indexing. Otherwise adds to wildcard list.
    pub fn add_rule(
        &mut self,
        head: Option<&str>,
        arity: usize,
        first_arg_head: Option<&'static str>,
        entry: RuleEntry<V>,
    ) {
        use crate::backend::models::gc_allocator::global_allocator;

        match head {
            Some(h) => {
                let interned: &'static str = global_allocator().alloc_str(h);
                let group = self.by_head_arity
                    .entry((interned, arity))
                    .or_insert_with(RuleGroup::new);

                // Check for duplicate across ALL entries in the group
                for existing in group.all_entries_mut() {
                    if existing.lhs == entry.lhs && existing.rhs == entry.rhs {
                        existing.multiplicity += 1;
                        return;
                    }
                }

                group.entries_for_mut(first_arg_head).push(entry);
            }
            None => {
                // Check for duplicate in wildcard list
                for existing in self.wildcard.iter_mut() {
                    if existing.lhs == entry.lhs && existing.rhs == entry.rhs {
                        existing.multiplicity += 1;
                        return;
                    }
                }
                self.wildcard.push(entry);
            }
        }
    }

    /// Remove a rule by decrementing multiplicity. Returns true if the entry was removed entirely.
    pub fn remove_rule(&mut self, lhs: &V, rhs: &V) -> bool {
        // Search in all groups
        for group in self.by_head_arity.values_mut() {
            if let Some(removed) = group.remove_rule(lhs, rhs) {
                return removed;
            }
        }
        // Check wildcard
        if let Some(pos) = self.wildcard.iter().position(|e| &e.lhs == lhs && &e.rhs == rhs) {
            if self.wildcard[pos].multiplicity > 1 {
                self.wildcard[pos].multiplicity -= 1;
                return false;
            } else {
                self.wildcard.remove(pos);
                return true;
            }
        }
        false
    }

    /// Get candidate rules for the given (head, arity) pair, optionally narrowed
    /// by the first argument's head symbol.
    ///
    /// Returns an iterator over matching rules chained with wildcard rules.
    /// When `first_arg_head` is `Some`, the candidate set is narrowed to rules
    /// whose LHS first argument has the same head symbol, plus rules with
    /// variable/wildcard first arguments. This reduces MORK `extract_data()`
    /// invocations by 3-10x for PLN workloads.
    pub fn get_candidates(
        &self,
        head: &str,
        arity: usize,
        first_arg_head: Option<&str>,
    ) -> impl Iterator<Item = &RuleEntry<V>> {
        use crate::backend::models::gc_allocator::global_allocator;

        let interned: &'static str = global_allocator().alloc_str(head);
        let group_iter = self.by_head_arity
            .get(&(interned, arity))
            .map(|group| group.get_candidates(first_arg_head));

        // Chain: group candidates (if group exists) + wildcard rules
        GroupOrEmpty { inner: group_iter }.chain(self.wildcard.iter())
    }

    /// Get all rules (for no-head queries).
    pub fn get_all_rules(&self) -> impl Iterator<Item = &RuleEntry<V>> {
        self.by_head_arity.values()
            .flat_map(|group| group.all_entries())
            .chain(self.wildcard.iter())
    }

    /// Check if there are any wildcard rules (rules with variable heads).
    #[inline]
    pub fn has_wildcard_rules(&self) -> bool {
        !self.wildcard.is_empty()
    }

    /// Get the number of rules in the index.
    pub fn len(&self) -> usize {
        self.by_head_arity
            .values()
            .map(|group| group.len())
            .sum::<usize>()
            + self.wildcard.len()
    }

    /// Clear the index.
    pub fn clear(&mut self) {
        self.by_head_arity.clear();
        self.wildcard.clear();
    }
}

/// Helper iterator: wraps an `Option<RuleGroupIter>`, yielding nothing when `None`.
struct GroupOrEmpty<'a, V: MettaValueTrait + Clone> {
    inner: Option<RuleGroupIter<'a, V>>,
}

impl<'a, V: MettaValueTrait + Clone> Iterator for GroupOrEmpty<'a, V> {
    type Item = &'a RuleEntry<V>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.as_mut()?.next()
    }
}

// ============================================================================
// Structural Matcher — Direct MettaValue pattern matching (MORK bypass)
// ============================================================================

/// Path from root to a node in the expression tree.
///
/// Each index selects a child of the current S-expression node.
/// For example, `[1, 0]` means "root's child at index 1, then that node's child at index 0".
/// Maximum depth of 8 covers all practical MeTTa patterns.
#[derive(Clone, Copy, Debug)]
struct MatchPath {
    indices: [u8; 8],
    len: u8,
}

impl MatchPath {
    /// Empty path (refers to the root node).
    #[inline]
    const fn root() -> Self {
        MatchPath {
            indices: [0; 8],
            len: 0,
        }
    }

    /// Create a new path by appending a child index.
    #[inline]
    fn child(&self, idx: u8) -> Self {
        debug_assert!((self.len as usize) < 8, "MatchPath depth overflow");
        let mut new = *self;
        new.indices[new.len as usize] = idx;
        new.len += 1;
        new
    }

    /// Navigate from root to the node at this path.
    /// Returns `None` if any intermediate node is not an S-expression or index is out of bounds.
    #[inline]
    fn navigate<'a, V: MettaValueTrait>(&self, root: &'a V) -> Option<&'a V> {
        let mut current = root;
        for i in 0..self.len {
            let items = current.as_sexpr()?;
            current = items.get(self.indices[i as usize] as usize)?;
        }
        Some(current)
    }

    /// Navigate from root to the node at this path, resolving variables through
    /// `bindings` at each step.  Returns an owned value because variable resolution
    /// may return a value from the binding map rather than the expression tree.
    ///
    /// At each intermediate step, if the current node is a bound variable, it is
    /// replaced with its binding before descending into children.  The final leaf
    /// is also resolved.  Unbound variables are returned as-is (same as
    /// `apply_bindings_generic` behaviour — single-level, no transitivity).
    #[inline]
    fn navigate_resolving<V>(
        &self,
        root: &V,
        bindings: &GenericBindings<V>,
    ) -> Option<V>
    where
        V: MettaValueTrait + Clone,
    {
        // Resolve a single node: if it is a bound variable, return the binding.
        #[inline(always)]
        fn resolve_one<V: MettaValueTrait + Clone>(
            val: &V,
            bindings: &GenericBindings<V>,
        ) -> V {
            if let Some(name) = val.as_atom() {
                if (name.starts_with('$')
                    || (name.starts_with('&') && name != "&" && name != "&self" && name != "&kb" && name != "&stack")
                    || name.starts_with('\''))
                    && name.len() > 1
                {
                    if let Some(bound) = bindings.get(name) {
                        return bound.clone();
                    }
                }
            }
            val.clone()
        }

        let mut current: V = resolve_one(root, bindings);
        for i in 0..self.len {
            let items = current.as_sexpr()?;
            let child = items.get(self.indices[i as usize] as usize)?;
            current = resolve_one(child, bindings);
        }
        Some(current)
    }
}

/// A structural check on the expression tree (no variable dependencies).
///
/// These checks are evaluated in order for fail-fast behavior. Each check
/// accesses a specific path in the expression tree and validates a structural
/// or value constraint.
#[derive(Clone, Copy, Debug)]
enum StructuralCheck {
    /// Check that the node at `path` is an S-expression with exactly `expected` children.
    Arity {
        path: MatchPath,
        expected: u16,
    },
    /// Check that the node at `path` is an atom equal to `expected`.
    /// Atom strings are interned (`&'static str`), so this is typically a pointer comparison.
    Atom {
        path: MatchPath,
        expected: &'static str,
    },
    /// Check that the node at `path` is a Long integer equal to `expected`.
    Long {
        path: MatchPath,
        expected: i64,
    },
    /// Check that the node at `path` is a Bool equal to `expected`.
    Bool {
        path: MatchPath,
        expected: bool,
    },
    /// Check that the node at `path` is a Float with bits equal to `expected_bits`.
    /// Uses bitwise comparison to avoid NaN issues.
    Float {
        path: MatchPath,
        expected_bits: u64,
    },
    /// Check that the node at `path` is a String equal to `expected`.
    Str {
        path: MatchPath,
        expected: &'static str,
    },
}

/// Variable binding operation, executed after all structural checks pass.
#[derive(Clone, Copy, Debug)]
enum VarOp {
    /// Bind the value at `path` to the variable `name`.
    Bind {
        path: MatchPath,
        name: &'static str,
    },
    /// Check that the value at `path` equals the already-bound variable at `bind_index`.
    /// Used for repeated variables like `(f $x $x)` where the second occurrence must
    /// equal the first.
    EqualCheck {
        path: MatchPath,
        bind_index: u8,
    },
}

/// Compiled structural matcher for direct MettaValue pattern matching.
///
/// Eliminates MORK serialization (`encode_wide_storage_inner`, 2% CPU) and
/// MORK trie traversal (`gnext`, 3.3% CPU) by performing structural comparison
/// directly on the in-memory MettaValue representation.
///
/// Created at `add_rule()` time by analyzing the LHS pattern. Each rule's LHS
/// is decomposed into a sequence of structural checks (arity, atom equality,
/// literal equality) followed by variable binding operations.
///
/// ## Execution Model
///
/// 1. **Structural checks** (fail-fast): Each check accesses a specific path
///    in the expression tree. If any check fails, the match fails immediately
///    without examining the remaining checks or extracting any bindings.
///
/// 2. **Variable bindings**: Only executed if all structural checks pass.
///    Variables are extracted at known tree positions. Repeated variables
///    (e.g., `$x` appearing twice) generate an `EqualCheck` for the second
///    occurrence.
///
/// ## Performance
///
/// For a typical PLN rule with 4 structural checks + 4 variable bindings:
/// - Structural matcher: ~60-100 ns (pointer derefs + comparisons)
/// - MORK path: ~300-500 ns (serialize + trie traverse + extract bindings)
/// - Speedup: ~4-6x per candidate match
///
/// When ALL candidates in a `(head, arity)` group have structural matchers,
/// the MORK byte serialization step is skipped entirely — eliminating the
/// amortized ~200 ns serialization cost.
#[derive(Debug, Clone)]
pub(crate) struct StructuralMatcher {
    /// Structural checks (arity + concrete value equality) — executed first, fail-fast.
    checks: SmallVec<[StructuralCheck; 8]>,
    /// Variable binding operations — executed after all structural checks pass.
    var_ops: SmallVec<[VarOp; 8]>,
}

impl StructuralMatcher {
    /// Analyze a rule's LHS pattern and create a StructuralMatcher.
    ///
    /// Returns `Some(matcher)` for patterns composed of S-expressions, atoms,
    /// variables, wildcards, and literals (Long, Bool, Float, String).
    ///
    /// Returns `None` for patterns containing:
    /// - Type nodes, Conjunction nodes, Error nodes
    /// - Quoted nodes in patterns
    /// - Any other non-standard value types
    ///
    /// These unsupported patterns fall back to MORK byte-level matching.
    pub fn analyze<V: MettaValueTrait + Clone>(lhs: &V) -> Option<Self> {
        let mut checks = SmallVec::new();
        let mut var_ops = SmallVec::new();
        // Track seen variables for repeated-variable detection.
        // Key: variable name, Value: index in var_ops of the first Bind.
        let mut seen_vars: SmallVec<[(&'static str, u8); 8]> = SmallVec::new();

        if !Self::analyze_node(lhs, MatchPath::root(), &mut checks, &mut var_ops, &mut seen_vars) {
            return None;
        }

        Some(StructuralMatcher { checks, var_ops })
    }

    /// Recursively analyze a node in the LHS pattern.
    /// Returns `false` if the node contains unsupported patterns.
    fn analyze_node<V: MettaValueTrait + Clone>(
        value: &V,
        path: MatchPath,
        checks: &mut SmallVec<[StructuralCheck; 8]>,
        var_ops: &mut SmallVec<[VarOp; 8]>,
        seen_vars: &mut SmallVec<[(&'static str, u8); 8]>,
    ) -> bool {
        // S-expression: check arity, recurse into children
        if let Some(items) = value.as_sexpr() {
            if path.len >= 8 {
                return false; // Depth overflow — bail to MORK
            }
            checks.push(StructuralCheck::Arity {
                path,
                expected: items.len() as u16,
            });
            for (i, child) in items.iter().enumerate() {
                if !Self::analyze_node(child, path.child(i as u8), checks, var_ops, seen_vars) {
                    return false;
                }
            }
            return true;
        }

        // Atom: variable, wildcard, or concrete symbol
        if let Some(atom) = value.as_atom() {
            if (atom.starts_with('$')
                || atom.starts_with('\'')
                || (atom.starts_with('&') && atom != "&"))
                && atom.len() > 1
            {
                // Variable — check if repeated
                if let Some(pos) = seen_vars.iter().position(|(name, _)| *name == atom) {
                    // Repeated variable: add equality check against first binding
                    var_ops.push(VarOp::EqualCheck {
                        path,
                        bind_index: seen_vars[pos].1,
                    });
                } else {
                    // First occurrence: bind
                    let bind_index = var_ops.len() as u8;
                    seen_vars.push((atom, bind_index));
                    var_ops.push(VarOp::Bind { path, name: atom });
                }
                return true;
            }
            if atom == "_" {
                // Wildcard — matches anything, no check or binding
                return true;
            }
            // Concrete atom — check equality
            checks.push(StructuralCheck::Atom {
                path,
                expected: atom,
            });
            return true;
        }

        // Long integer literal
        if let Some(n) = value.as_long() {
            checks.push(StructuralCheck::Long { path, expected: n });
            return true;
        }

        // Boolean literal
        if let Some(b) = value.as_bool() {
            checks.push(StructuralCheck::Bool { path, expected: b });
            return true;
        }

        // Float literal (bitwise comparison)
        if let Some(f) = value.as_float() {
            checks.push(StructuralCheck::Float {
                path,
                expected_bits: f.to_bits(),
            });
            return true;
        }

        // String literal
        if let Some(s) = value.as_string() {
            // Intern the string for fast comparison
            let interned = crate::backend::models::gc_allocator::global_allocator().alloc_str(s);
            checks.push(StructuralCheck::Str {
                path,
                expected: interned,
            });
            return true;
        }

        // Unsupported node type (Type, Conjunction, Error, Quoted, etc.)
        // Fall back to MORK matching
        false
    }

    /// Try to match an expression against this compiled pattern.
    ///
    /// Returns `Some(bindings)` if the expression matches all structural checks
    /// and variable constraints. Returns `None` on any mismatch.
    ///
    /// Cost: O(checks + var_ops) with very low constant factors (pointer derefs
    /// and comparisons, no hashing or serialization).
    #[inline]
    pub fn try_match<V>(&self, expr: &V) -> Option<GenericBindings<V>>
    where
        V: MettaValueTrait + Clone + PartialEq,
    {
        // Phase 1: Structural checks (fail-fast)
        for check in &self.checks {
            match check {
                StructuralCheck::Arity { path, expected } => {
                    let val = path.navigate(expr)?;
                    let items = val.as_sexpr()?;
                    if items.len() != *expected as usize {
                        return None;
                    }
                }
                StructuralCheck::Atom { path, expected } => {
                    let val = path.navigate(expr)?;
                    let atom = val.as_atom()?;
                    if atom != *expected {
                        return None;
                    }
                }
                StructuralCheck::Long { path, expected } => {
                    let val = path.navigate(expr)?;
                    if val.as_long()? != *expected {
                        return None;
                    }
                }
                StructuralCheck::Bool { path, expected } => {
                    let val = path.navigate(expr)?;
                    if val.as_bool()? != *expected {
                        return None;
                    }
                }
                StructuralCheck::Float { path, expected_bits } => {
                    let val = path.navigate(expr)?;
                    if val.as_float()?.to_bits() != *expected_bits {
                        return None;
                    }
                }
                StructuralCheck::Str { path, expected } => {
                    let val = path.navigate(expr)?;
                    let s = val.as_string()?;
                    if s != *expected {
                        return None;
                    }
                }
            }
        }

        // Phase 2: Variable bindings (only executed if all checks pass)
        let mut bindings = GenericBindings::new();
        // Track bound values for EqualCheck (repeated variables)
        let mut bound_values: SmallVec<[&V; 8]> = SmallVec::new();

        for op in &self.var_ops {
            match op {
                VarOp::Bind { path, name } => {
                    let val = path.navigate(expr)?;
                    bound_values.push(val);
                    bindings.insert(*name, val.clone());
                }
                VarOp::EqualCheck { path, bind_index } => {
                    let val = path.navigate(expr)?;
                    let bound = bound_values[*bind_index as usize];
                    if val != bound {
                        return None;
                    }
                }
            }
        }

        Some(bindings)
    }

    /// Match a **template** expression whose variables are not yet substituted,
    /// resolving them on-the-fly through `outer_bindings`.
    ///
    /// This is the WAM-inspired "binding-aware" matching path: instead of first
    /// materializing `apply_bindings(template, outer_bindings)` and then calling
    /// `try_match`, we resolve variables lazily during navigation.  This avoids
    /// the O(tree) allocation that `apply_bindings_generic` would perform.
    ///
    /// Semantically equivalent to:
    /// ```text
    ///   let materialized = apply_bindings(template, outer_bindings);
    ///   self.try_match(&materialized)
    /// ```
    /// but without allocating the materialized expression.
    #[inline]
    pub fn try_match_with_bindings<V>(
        &self,
        template: &V,
        outer_bindings: &GenericBindings<V>,
    ) -> Option<GenericBindings<V>>
    where
        V: MettaValueTrait + Clone + PartialEq,
    {
        // Phase 1: Structural checks with variable resolution
        for check in &self.checks {
            match check {
                StructuralCheck::Arity { path, expected } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    let items = val.as_sexpr()?;
                    if items.len() != *expected as usize {
                        return None;
                    }
                }
                StructuralCheck::Atom { path, expected } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    let atom = val.as_atom()?;
                    if atom != *expected {
                        return None;
                    }
                }
                StructuralCheck::Long { path, expected } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    if val.as_long()? != *expected {
                        return None;
                    }
                }
                StructuralCheck::Bool { path, expected } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    if val.as_bool()? != *expected {
                        return None;
                    }
                }
                StructuralCheck::Float { path, expected_bits } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    if val.as_float()?.to_bits() != *expected_bits {
                        return None;
                    }
                }
                StructuralCheck::Str { path, expected } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    let s = val.as_string()?;
                    if s != *expected {
                        return None;
                    }
                }
            }
        }

        // Phase 2: Variable bindings with resolution
        let mut bindings = GenericBindings::new();
        let mut bound_values: SmallVec<[V; 8]> = SmallVec::new();

        for op in &self.var_ops {
            match op {
                VarOp::Bind { path, name } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    bound_values.push(val.clone());
                    bindings.insert(*name, val);
                }
                VarOp::EqualCheck { path, bind_index } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    let bound = &bound_values[*bind_index as usize];
                    if val != *bound {
                        return None;
                    }
                }
            }
        }

        Some(bindings)
    }
}

/// Validate that ALL tag bytes in a MORK expression are valid (no reserved bytes 0x40-0x7F).
///
/// Uses the same traversal logic as `mork_expr_byte_len()`. Returns `Ok(len)` if all
/// bytes are valid MORK tags, or `Err((offset, byte))` for the first reserved byte found.
///
/// This is used as a diagnostic tool to catch byte misalignment issues before they
/// cause panics in `ExprZipper::new()` or `ExprZipper::tag()` (which call `byte_item()`).
#[cfg(any(debug_assertions, test))]
pub(crate) fn validate_mork_bytes(bytes: &[u8]) -> Result<usize, (usize, u8)> {
    let mut offset = 0usize;
    let mut depth = 1u32;

    while depth > 0 && offset < bytes.len() {
        let byte = bytes[offset];
        let tag = match maybe_byte_item(byte) {
            Ok(t) => t,
            Err(reserved) => return Err((offset, reserved)),
        };
        offset += 1;
        depth -= 1;

        match tag {
            Tag::NewVar | Tag::VarRef(_) => {}
            Tag::SymbolSize(size) => {
                let end = offset + size as usize;
                if end > bytes.len() {
                    // Symbol data extends past the buffer — truncated expression
                    return Err((offset - 1, byte));
                }
                offset = end;
            }
            Tag::Arity(arity) => {
                depth += arity as u32;
            }
        }
    }

    if depth > 0 {
        // Expression is incomplete — ran out of bytes before all children were consumed
        return Err((offset, 0xFF));
    }

    Ok(offset)
}

/// Count the number of NewVar tags in MORK bytes (used for specificity computation).
///
/// Each NewVar tag (0xC0) introduces a new variable binding position.
/// Fewer NewVar tags = more specific pattern (more concrete structure).
///
/// Uses `maybe_byte_item()` to validate the first byte before creating an `ExprZipper`.
/// Returns 0 if the bytes are empty or start with a reserved byte.
fn count_newvar_tags(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    // Validate first byte is a valid MORK tag before calling ExprZipper::new()
    // (which uses byte_item() and panics on reserved bytes 0x40-0x7F)
    if let Err(reserved) = maybe_byte_item(bytes[0]) {
        tracing::warn!(
            target: "mettatron::count_newvar_tags",
            "LHS De Bruijn bytes start with reserved byte 0x{:02x}, skipping",
            reserved
        );
        return 0;
    }
    let mut count = 0;
    let expr = Expr { ptr: bytes.as_ptr().cast_mut() };
    let mut ez = ExprZipper::new(expr);
    loop {
        if ez.tag() == Tag::NewVar {
            count += 1;
        }
        if !ez.next() {
            break;
        }
    }
    count
}

/// Build a list of original variable names from the ConversionContext,
/// identifying wildcard indices (anonymous variables from `_`).
///
/// Returns `(var_names, wildcard_indices)` where:
/// - `var_names[i]` is the full variable name including `$` prefix for De Bruijn index `i`
/// - `wildcard_indices` contains indices of anonymous wildcard variables
fn build_var_names_and_wildcards(
    ctx_var_names: &[String],
    lhs_var_count: usize,
) -> (Vec<&'static str>, SmallVec<[u8; 4]>) {
    use crate::backend::models::gc_allocator::global_allocator;

    let alloc = global_allocator();
    let mut var_names = Vec::with_capacity(lhs_var_count);
    let mut wildcard_indices = SmallVec::new();

    for (i, name) in ctx_var_names.iter().enumerate() {
        if i >= lhs_var_count {
            break;
        }
        if name.starts_with("__anon") {
            // Wildcard _ was encoded as __anonN
            var_names.push("_");
            wildcard_indices.push(i as u8);
        } else {
            // Regular variable — restore the $ prefix, intern via slab allocator
            let interned = alloc.alloc_str(&format!("${}", name));
            var_names.push(interned);
        }
    }

    (var_names, wildcard_indices)
}

/// Extract bindings by walking De Bruijn bytes and the original expression in parallel.
///
/// Since `extract_data` already confirmed the structural match, we can skip all
/// matching logic and just navigate to NewVar positions to capture sub-expressions
/// from the original value. This preserves runtime types (SpaceHandle, State, etc.)
/// that can't survive a MORK serialize→deserialize round trip.
///
/// O(pattern_size) time — same as extract_data, but operates on MettaValues.
fn extract_bindings_from_expr<V>(
    lhs_debruijn: &[u8],
    expr: &V,
    var_names: &[&'static str],
    wildcard_indices: &SmallVec<[u8; 4]>,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
{
    let mut bindings = GenericBindings::new();
    let mut offset = 0usize;
    let mut newvar_idx = 0u8;
    let mut expr_stack: Vec<&V> = vec![expr];
    // Exclude padding byte (0x00) appended for ExprZipper read-past-end safety
    let end = lhs_debruijn.len().saturating_sub(1);

    while offset < end && !expr_stack.is_empty() {
        let tag = match maybe_byte_item(lhs_debruijn[offset]) {
            Ok(t) => t,
            Err(_) => break,
        };
        offset += 1;

        match tag {
            Tag::NewVar => {
                let value = match expr_stack.pop() {
                    Some(v) => v,
                    None => break,
                };
                if !wildcard_indices.contains(&newvar_idx)
                    && (newvar_idx as usize) < var_names.len()
                {
                    bindings.insert(var_names[newvar_idx as usize], value.clone());
                }
                newvar_idx += 1;
            }
            Tag::VarRef(_) => {
                expr_stack.pop(); // Consume without binding
            }
            Tag::SymbolSize(size) => {
                offset += size as usize; // Skip symbol bytes
                expr_stack.pop(); // Consume the corresponding atom/leaf
            }
            Tag::Arity(n) => {
                if n == 0 {
                    expr_stack.pop(); // Unit / empty S-expression
                } else if let Some(parent) = expr_stack.pop() {
                    // Push children in reverse order so first child is on top
                    if let Some(items) = parent.as_sexpr() {
                        for child in items.iter().rev() {
                            expr_stack.push(child);
                        }
                    } else if let Some(goals) = parent.as_conjunction() {
                        // Conjunction: MORK writes Arity(goals+1) with comma as first child.
                        // Push goals in reverse, then a placeholder for the comma
                        // (the comma's SymbolSize tag will pop and discard it).
                        for goal in goals.iter().rev() {
                            expr_stack.push(goal);
                        }
                        expr_stack.push(parent); // Placeholder consumed by SymbolSize(",")
                    } else if let Some((_msg, details)) = parent.as_error() {
                        // Error: MORK writes Arity(3) with "error", "msg", details.
                        // Push details, then placeholders for msg and "error".
                        expr_stack.push(details);
                        expr_stack.push(parent); // Placeholder consumed by SymbolSize("\"msg\"")
                        expr_stack.push(parent); // Placeholder consumed by SymbolSize("error")
                    } else {
                        // Other compound types (Space, State serialized as S-expr in MORK).
                        // These shouldn't appear as Arity match targets in LHS patterns
                        // (they're opaque types matched by NewVar), but if they do,
                        // we can't drill into them — stop extraction.
                        break;
                    }
                }
            }
        }
    }

    bindings
}

/// Extract bindings by walking Wide MORK De Bruijn bytes and the original expression in parallel.
///
/// Same algorithm as `extract_bindings_from_expr` but for Wide MORK tag format
/// (tag-byte + LEB128 instead of MORK's 2-bit tag + 6-bit payload).
///
/// Since `wide_extract_data` already confirmed the structural match, we can skip all
/// matching logic and just navigate to NewVar positions to capture sub-expressions.
fn extract_bindings_from_wide_expr<V>(
    lhs_wide_debruijn: &[u8],
    expr: &V,
    var_names: &[&'static str],
    wildcard_indices: &SmallVec<[u8; 4]>,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
{
    use crate::backend::wide_mork::encoding::{
        WideTag, decode_leb128,
    };

    let mut bindings = GenericBindings::new();
    let mut offset = 0usize;
    let mut newvar_idx = 0u8;
    let mut expr_stack: Vec<&V> = vec![expr];
    let end = lhs_wide_debruijn.len();

    while offset < end && !expr_stack.is_empty() {
        let tag = match WideTag::from_byte(lhs_wide_debruijn[offset]) {
            Ok(t) => t,
            Err(_) => break,
        };
        offset += 1;

        match tag {
            WideTag::NewVar => {
                let value = match expr_stack.pop() {
                    Some(v) => v,
                    None => break,
                };
                if !wildcard_indices.contains(&newvar_idx)
                    && (newvar_idx as usize) < var_names.len()
                {
                    bindings.insert(var_names[newvar_idx as usize], value.clone());
                }
                newvar_idx += 1;
            }
            WideTag::VarRef => {
                // Consume the LEB128 index and the corresponding expression
                if let Some((_idx, consumed)) = decode_leb128(&lhs_wide_debruijn[offset..]) {
                    offset += consumed;
                } else {
                    break;
                }
                expr_stack.pop(); // Consume without binding
            }
            WideTag::SymbolSize => {
                // Skip the LEB128 size and the symbol bytes
                if let Some((size, consumed)) = decode_leb128(&lhs_wide_debruijn[offset..]) {
                    offset += consumed + size as usize;
                } else {
                    break;
                }
                expr_stack.pop(); // Consume the corresponding atom/leaf
            }
            WideTag::Arity => {
                if let Some((n, consumed)) = decode_leb128(&lhs_wide_debruijn[offset..]) {
                    offset += consumed;
                    if n == 0 {
                        expr_stack.pop(); // Unit / empty S-expression
                    } else if let Some(parent) = expr_stack.pop() {
                        // Push children in reverse order so first child is on top
                        if let Some(items) = parent.as_sexpr() {
                            for child in items.iter().rev() {
                                expr_stack.push(child);
                            }
                        } else if let Some(goals) = parent.as_conjunction() {
                            for goal in goals.iter().rev() {
                                expr_stack.push(goal);
                            }
                            expr_stack.push(parent); // Placeholder for comma
                        } else if let Some((_msg, details)) = parent.as_error() {
                            expr_stack.push(details);
                            expr_stack.push(parent); // Placeholder for msg
                            expr_stack.push(parent); // Placeholder for "error"
                        } else {
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }
    }

    bindings
}

/// Build the head+arity MORK byte prefix for targeted rule lookup.
///
/// **NOTE**: Superseded by `RuleIndex` for the primary hot path. Retained for
/// `get_matching_rules_for_expr()` fallback used in tests and `match_space` compatibility.
///
/// Extends the cached rule prefix with `[Arity(arity+1)] + [head symbol MORK bytes]`.
/// The MORK arity includes the head element, so MeTTa arity (excludes head) needs `+1`.
///
/// Returns `None` if the head is empty, arity exceeds the MORK 6-bit limit (63),
/// or MORK serialization fails.
fn build_head_arity_prefix<V, F>(
    rule_prefix: &[u8],
    head: &str,
    arity: usize,
    factory: &F,
    sm: &mork_interning::SharedMappingHandle,
    cache_epoch: u64,
) -> Option<Vec<u8>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if head.is_empty() {
        return None;
    }
    let mork_arity = (arity + 1) as u8; // MORK arity includes head
    if mork_arity >= 64 {
        return None; // MORK arity limit (6 bits)
    }

    // Serialize head symbol to get its MORK bytes (SymbolSize tag + interned key)
    let head_atom = factory.atom(head);
    with_mork_bytes(&head_atom, sm, cache_epoch, |head_bytes| {
        let mut prefix = Vec::with_capacity(rule_prefix.len() + 1 + head_bytes.len());
        prefix.extend_from_slice(rule_prefix);
        // Arity tag: upper 2 bits = 00, lower 6 bits = arity value
        prefix.push(mork_arity);
        prefix.extend_from_slice(head_bytes);
        prefix
    })
    .ok()
}

/// Iterator over rule heads with their arities and counts.
///
/// Data is collected into a Vec during creation for iteration.
pub struct RuleHeadsIter {
    inner: std::vec::IntoIter<(String, usize, usize)>,
}

impl RuleHeadsIter {
    /// Create a new iterator from collected rule heads.
    pub fn new(items: Vec<(String, usize, usize)>) -> Self {
        Self {
            inner: items.into_iter(),
        }
    }
}

impl Iterator for RuleHeadsIter {
    type Item = (String, usize, usize);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

// ============================================================================
// Generic Rule Operations (for GenericEnvironment<V, F>)
// ============================================================================

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Add a rule to the environment.
    ///
    /// The rule is stored as `(= lhs rhs)` in the MORK PathMap using De Bruijn encoding
    /// (via `with_mork_query_bytes`). The RuleIndex is populated with cached MettaValues,
    /// De Bruijn bytes, and metadata for O(1) lookup + byte-level matching.
    ///
    /// Bloom filter is updated for O(1) rejection in match_space().
    ///
    /// # Arguments
    /// - `lhs`: The left-hand side pattern
    /// - `rhs`: The right-hand side template
    pub fn add_rule(&mut self, lhs: V, rhs: V) {
        trace!(target: "mettatron::environment::add_rule", "Adding rule");
        self.make_owned(); // CoW: ensure we own data before modifying

        // Phase 9.5: Invalidate normal-form memoization — new rules may make
        // previously normal-form expressions reducible.
        crate::backend::eval::trampoline::invalidate_normal_form_memo();

        // Clear eval memo and match result caches — new rules may change
        // evaluation and matching results for previously cached expressions.
        crate::backend::eval::trampoline::clear_eval_memo();
        crate::backend::eval::trampoline::clear_match_result_cache();

        // Increment rule/type epoch — invalidates cached TypeSignatureRegistry in JIT.
        increment_rule_epoch();

        // Get head symbol and arity for bloom filter (clone head string before moving lhs)
        let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
        let arity = lhs.get_arity();

        // Phase 8.1: Compute RHS type at insertion time for branch pruning (Phase 8.7).
        // Only stores non-trivial types — %Undefined% provides no pruning benefit.
        let rhs_type = {
            use crate::backend::eval::types_generic::infer_type_generic;
            let inferred = infer_type_generic(&rhs, &self.factory, self);
            if inferred.as_atom() == Some("%Undefined%") { None } else { Some(inferred) }
        };

        // Trace: RhsTypeComputed
        #[cfg(feature = "eval-trace")]
        {
            crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    crate::backend::trace::trace_value_generic(&lhs),
                    vec![],
                    None,
                    trace_format::TraceEventKind::RhsTypeComputed {
                        head: head_owned.as_deref().unwrap_or("<none>").to_string(),
                        arity: arity as u32,
                        lhs: crate::backend::trace::trace_value_generic(&lhs),
                        rhs: crate::backend::trace::trace_value_generic(&rhs),
                        rhs_type: rhs_type.as_ref().map(crate::backend::trace::trace_value_generic),
                    },
                );
            });
        }

        // Phase 10.1: Register inferred return type in function return type index.
        // Makes rhs_type queryable by infer_types_generic for user-defined functions
        // without explicit (: f (-> ...)) type declarations.
        if let Some(ref rt) = rhs_type {
            if let Some(ref head) = head_owned {
                self.register_inferred_type(head, rt);

                // Trace: InferredTypeRegistered (Phase 10.1)
                #[cfg(feature = "eval-trace")]
                {
                    crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            0,
                            crate::backend::trace::trace_value_generic(rt),
                            vec![],
                            None,
                            trace_format::TraceEventKind::InferredTypeRegistered {
                                function_name: head.clone(),
                                registered_type: crate::backend::trace::trace_value_generic(rt),
                                source: "phase-10.1-rhs".to_string(),
                            },
                        );
                    });
                }
            }
        }

        // Phase 10.4: Synthesize arrow type from rule LHS pattern + RHS body.
        // Analyzes parameter constraints from RHS usage to build (-> T1 T2 ... Tret).
        // Only for rules without explicit (: f (-> ...)) type declarations.
        if let Some(ref head) = head_owned {
            // Skip if the function already has a declared arrow type
            let has_declared_arrow = self.get_types_generic(head).iter().any(|t| {
                t.as_sexpr()
                    .and_then(|items| items.first().and_then(|v| v.as_atom()))
                    == Some("->")
            });
            if !has_declared_arrow {
                use crate::backend::eval::types_generic::infer_arrow_type_from_rule;
                if let Some(arrow) = infer_arrow_type_from_rule(
                    &lhs,
                    &rhs,
                    rhs_type.as_ref(),
                    &self.factory,
                    self,
                ) {
                    self.register_inferred_type(head, &arrow);

                    // Trace: InferredTypeRegistered (Phase 10.4)
                    #[cfg(feature = "eval-trace")]
                    {
                        crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                0,
                                crate::backend::trace::trace_value_generic(&arrow),
                                vec![],
                                None,
                                trace_format::TraceEventKind::InferredTypeRegistered {
                                    function_name: head.clone(),
                                    registered_type: crate::backend::trace::trace_value_generic(&arrow),
                                    source: "phase-10.4-arrow".to_string(),
                                },
                            );
                        });
                    }
                }
            }
        }

        // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
        if let Some(ref head) = head_owned {
            self.shared.fuzzy_matcher.write().insert(head);
        }

        // Create rule s-expression: (= lhs rhs)
        let rule_sexpr = self.factory.sexpr(vec![
            self.factory.atom("="),
            lhs.clone(),
            rhs.clone(),
        ]);

        // Convert to De Bruijn bytes and insert into PathMap + RuleIndex
        let rule_prefix_len = super::generic::RULE_PREFIX_LEN;
        let result = with_mork_query_bytes(
            &rule_sexpr,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |debruijn_bytes, ctx| {
                // 1. Insert De Bruijn bytes into PathMap (increment multiplicity)
                {
                    let mut btm = self.shared.atom_space.btm.write();
                    super::multiplicity::add_atom(&mut btm, debruijn_bytes);
                }
                self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

                // 2. Split De Bruijn bytes into LHS and RHS ranges
                // Layout: [Arity(3)] ["=" symbol bytes] [LHS bytes] [RHS bytes]
                //         |---------- rule_prefix_len --|
                if debruijn_bytes.len() <= rule_prefix_len {
                    return; // Shouldn't happen for valid rules
                }

                // Verify structural invariant: rules start with Arity(3) byte
                #[cfg(debug_assertions)]
                {
                    assert_eq!(
                        debruijn_bytes[0], 0x03,
                        "Rule debruijn_bytes does not start with Arity(3): first byte is 0x{:02x}",
                        debruijn_bytes[0]
                    );
                }

                let lhs_start = rule_prefix_len;
                let lhs_byte_len = mork_expr_byte_len(&debruijn_bytes[lhs_start..]);

                // Validate LHS byte range, first byte, and ALL bytes
                #[cfg(debug_assertions)]
                {
                    if lhs_start + lhs_byte_len > debruijn_bytes.len() {
                        panic!(
                            "LHS byte range {}..{} exceeds debruijn_bytes len {}",
                            lhs_start, lhs_start + lhs_byte_len, debruijn_bytes.len()
                        );
                    }
                    let first_lhs_byte = debruijn_bytes[lhs_start];
                    if let Err(reserved) = maybe_byte_item(first_lhs_byte) {
                        panic!(
                            "LHS starts with reserved byte 0x{:02x} at offset {} in {:02x?}",
                            reserved, lhs_start, &debruijn_bytes[..debruijn_bytes.len().min(32)]
                        );
                    }
                }

                // Extract LHS De Bruijn bytes with one extra zero byte of padding.
                // See the comment on expr_bytes_owned in match_rules_native() for why
                // padding is needed (ExprZipper::gnext reads one byte past the end).
                let mut lhs_debruijn = Vec::with_capacity(lhs_byte_len + 1);
                lhs_debruijn.extend_from_slice(&debruijn_bytes[lhs_start..lhs_start + lhs_byte_len]);
                lhs_debruijn.push(0x00); // Padding byte for ExprZipper read-past-end safety

                // Validate ALL bytes in lhs_debruijn are valid MORK (no reserved 0x40-0x7F)
                #[cfg(debug_assertions)]
                {
                    if let Err((off, byte)) = validate_mork_bytes(&lhs_debruijn) {
                        panic!(
                            "lhs_debruijn has invalid byte 0x{:02x} at offset {} (len={}).\n\
                             lhs_debruijn: {:02x?}\n\
                             full debruijn_bytes (first 64): {:02x?}\n\
                             rule_prefix_len: {}, lhs_start: {}, lhs_byte_len: {}",
                            byte, off, lhs_debruijn.len(),
                            &lhs_debruijn[..lhs_debruijn.len().min(32)],
                            &debruijn_bytes[..debruijn_bytes.len().min(64)],
                            rule_prefix_len, lhs_start, lhs_byte_len
                        );
                    }
                }

                // 3. Compute metadata from De Bruijn encoding
                let lhs_var_count = count_newvar_tags(&lhs_debruijn);
                let (var_names, wildcard_indices) =
                    build_var_names_and_wildcards(&ctx.var_names, lhs_var_count);

                // 4. Populate RuleIndex with second-level first-arg indexing
                let alloc = crate::backend::models::gc_allocator::global_allocator();
                let first_arg_head_interned: Option<&'static str> =
                    get_first_arg_head(&lhs).map(|s| alloc.alloc_str(s));
                let structural_matcher = StructuralMatcher::analyze(&lhs);
                let entry = RuleEntry {
                    lhs: lhs.clone(),
                    rhs_has_variables: rhs.contains_variables(),
                    rhs: rhs.clone(),
                    lhs_debruijn,
                    lhs_wide_debruijn: Vec::new(), // Narrow path — MORK encoding succeeded
                    var_names,
                    wildcard_indices,
                    multiplicity: 1,
                    rhs_type: rhs_type.clone(),
                    structural_matcher,
                };
                // Phase 4a: Pre-seed tiered cache so first RHS evaluation
                // immediately triggers bytecode compilation (no warmup delay)
                crate::backend::bytecode::tiered_cache::global_tiered_cache()
                    .preseed_for_immediate_compile(rhs.hash_value());

                self.shared.rule_index.write().add_rule(
                    head_owned.as_deref(),
                    arity,
                    first_arg_head_interned,
                    entry,
                );
            },
        );

        // Fallback for expressions that can't be MORK-encoded (arity >= 64).
        // Use Wide MORK encoding for proper byte-level pattern matching.
        // Store in wide_btm (PathMap<Multiplicity>) — same type as btm.
        if result.is_err() {
            let mut wide_key = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_storage(&rule_sexpr, &mut wide_key);

            {
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                super::multiplicity::add_atom(&mut wbtm, &wide_key);
            }

            self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

            // Encode the LHS with Wide MORK De Bruijn encoding for byte-level matching.
            // This replaces the old structural fallback with O(n) byte-level matching.
            let mut wide_ctx = crate::backend::wide_mork::encoding::WideConversionContext::new();
            let mut lhs_wide_debruijn = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_debruijn(
                &lhs, &mut wide_ctx, &mut lhs_wide_debruijn,
            );

            let lhs_var_count = crate::backend::wide_mork::encoding::count_wide_newvar_tags(&lhs_wide_debruijn);
            let (var_names, wildcard_indices) =
                build_var_names_and_wildcards(&wide_ctx.var_names, lhs_var_count);

            let alloc = crate::backend::models::gc_allocator::global_allocator();
            let first_arg_head_interned: Option<&'static str> =
                get_first_arg_head(&lhs).map(|s| alloc.alloc_str(s));
            let structural_matcher = StructuralMatcher::analyze(&lhs);
            let entry = RuleEntry {
                lhs: lhs.clone(),
                rhs_has_variables: rhs.contains_variables(),
                rhs: rhs.clone(),
                lhs_debruijn: Vec::new(), // Empty — this is a wide rule
                lhs_wide_debruijn,
                var_names,
                wildcard_indices,
                multiplicity: 1,
                rhs_type, // Phase 8.1: computed before closure, last use — no clone needed
                structural_matcher,
            };
            // Phase 4a: Pre-seed tiered cache for wide MORK path
            crate::backend::bytecode::tiered_cache::global_tiered_cache()
                .preseed_for_immediate_compile(rhs.hash_value());

            self.shared.rule_index.write().add_rule(
                head_owned.as_deref(),
                arity,
                first_arg_head_interned,
                entry,
            );
        }

        // Update bloom filter with (head, arity) for O(1) match_space() rejection
        if let Some(ref head) = head_owned {
            let arity_u8 = arity as u8;
            self.shared
                .atom_space.head_arity_bloom
                .write()
                .insert(head.as_bytes(), arity_u8);
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Match rules natively using MORK byte-level `extract_data()` for pattern matching.
    ///
    /// This replaces the old pipeline of:
    /// 1. `get_matching_rules_for_expr()` → trie traversal + LHS/RHS deserialization
    /// 2. `pattern_match_generic()` → MettaValue-level structural comparison
    /// 3. `apply_bindings_generic()` → MettaValue-level binding substitution
    ///
    /// New pipeline:
    /// 1. **Bloom filter** — O(1) rejection by (head, arity)
    /// 2. **RuleIndex lookup** — O(1) HashMap lookup for `(head, arity) → Vec<RuleEntry>`
    /// 3. **Serialize expr ONCE** — `with_mork_bytes(expr)` → expr_bytes
    /// 4. **`extract_data()`** — O(n) byte-level pattern matching per candidate (no deserialization)
    /// 5. **Extract bindings** — Only for successful matches (all matching rules fire nondeterministically)
    /// 6. **`apply_bindings_generic()`** — Apply bindings to cached RHS MettaValue
    ///
    /// ## Performance
    ///
    /// Eliminates LHS deserialization per candidate (~27% of old wall time),
    /// trie traversal page faults (~23%), and MettaValue-level pattern matching (~15%).
    ///
    /// ## Structural Matcher Fast Path
    ///
    /// When all candidate rules have compiled structural matchers (>95% of the time
    /// for typical PLN workloads), MORK serialization is bypassed entirely.
    /// The structural matcher performs direct MettaValue comparison at ~4-6x the speed
    /// of MORK byte-level matching, eliminating `encode_wide_storage_inner` (2% CPU)
    /// and `ExprZipper::gnext` (3.3% CPU) from the hot path.
    pub fn match_rules_native(
        &self,
        expr: &V,
        apply_bindings: impl Fn(&V, &GenericBindings<V>, &F) -> V,
    ) -> Vec<RuleMatchResult<V>> {
        let head = expr.get_head_symbol().unwrap_or("");
        let arity = expr.get_arity();
        // Phase 3: Extract first argument's head symbol for second-level index narrowing
        let first_arg_head = get_first_arg_head(expr);

        // Bloom filter O(1) rejection: skip MORK serialization entirely when
        // the bloom filter says no head-specific rules exist for this head+arity
        // AND there are no wildcard rules (which match any head).
        if !head.is_empty() {
            let bloom_says_no = !self
                .shared
                .atom_space.head_arity_bloom
                .read()
                .may_contain(head.as_bytes(), arity as u8);
            if bloom_says_no && !self.shared.rule_index.read().has_wildcard_rules() {
                return Vec::new();
            }
        }

        // ── Structural Matcher Fast Path ──
        //
        // Try structural matchers for all candidates first. If ALL candidates have
        // structural matchers, we bypass MORK serialization entirely — eliminating
        // encode_wide_storage_inner (2% CPU) and gnext (3.3% CPU) from the hot path.
        //
        // Structural matchers perform direct MettaValue comparison: arity checks via
        // as_sexpr().len(), atom equality via &'static str comparison, and variable
        // extraction at known tree positions. No hashing, no serialization, no trie.
        {
            let rule_index = self.shared.rule_index.read();

            // Collect candidates into a SmallVec (stack-allocated for ≤16 entries).
            // This avoids holding the iterator across the match loop (which would
            // prevent us from knowing upfront whether all candidates are structural).
            let candidates: SmallVec<[&RuleEntry<V>; 16]> = if !head.is_empty() {
                rule_index.get_candidates(head, arity, first_arg_head).collect()
            } else {
                rule_index.get_all_rules().collect()
            };

            if candidates.is_empty() {
                return Vec::new();
            }

            // Check if ALL candidates have structural matchers
            let all_structural = candidates.iter().all(|e| e.structural_matcher.is_some());

            // Phase E: Populate operator inline cache with metadata about this
            // (head, arity) — the caller can use this to skip hash_value()
            // computation on subsequent calls.
            if !head.is_empty() {
                let current_epoch = RULE_EPOCH.load(Ordering::Acquire);
                crate::backend::eval::trampoline::dispatch_hints::operator_cache_put(
                    head,
                    arity,
                    crate::backend::eval::trampoline::dispatch_hints::OperatorCacheEntry {
                        rule_epoch: current_epoch,
                        all_structural,
                        candidate_count: candidates.len(),
                    },
                );
            }

            if all_structural {
                // Fast path: all candidates have structural matchers — bypass MORK entirely
                let mut results: Vec<RuleMatchResult<V>> = Vec::new();

                for entry in &candidates {
                    if let Some(ref matcher) = entry.structural_matcher {
                        if let Some(bindings) = matcher.try_match(expr) {
                            let instantiated_rhs = if entry.rhs_has_variables {
                                apply_bindings(&entry.rhs, &bindings, &self.factory)
                            } else {
                                entry.rhs.clone()
                            };
                            let multiplicity = entry.multiplicity.max(1);
                            if multiplicity == 1 {
                                results.push(RuleMatchResult {
                                    instantiated_rhs,
                                    rhs_template: entry.rhs.clone(),
                                    bindings,
                                    multiplicity: 1,
                                    rhs_type: entry.rhs_type.clone(),
                                    rhs_has_variables: entry.rhs_has_variables,
                                });
                            } else {
                                for _ in 0..multiplicity {
                                    results.push(RuleMatchResult {
                                        instantiated_rhs: instantiated_rhs.clone(),
                                        rhs_template: entry.rhs.clone(),
                                        bindings: bindings.clone(),
                                        multiplicity,
                                        rhs_type: entry.rhs_type.clone(),
                                        rhs_has_variables: entry.rhs_has_variables,
                                    });
                                }
                            }
                        }
                    }
                }

                return results;
            }
        }
        // ── End Structural Matcher Fast Path ──

        // Slow path: at least one candidate lacks a structural matcher — need MORK.
        //
        // Serialize the expression to MORK bytes ONCE using a thread-local buffer.
        // The buffer grows as needed but is never freed — amortized zero allocation.
        //
        // IMPORTANT: One extra zero byte is appended as padding. MORK's ExprZipper::gnext()
        // reads one byte past the last element of any S-expression to check if the next
        // sibling is an Arity tag. When expressions are embedded in PathMap memory, this
        // read is harmless (it reads a byte from the parent structure). But for standalone
        // Vec<u8> buffers, this reads past the allocation — causing UB and panics on
        // reserved bytes (0x40-0x7F) under valgrind. A trailing 0x00 is Arity(0), which
        // is valid and harmless (just pushes an empty breadcrumb that gets popped immediately).
        //
        // All three phases (byte matching, specificity filter, binding extraction) are
        // performed inside the thread-local borrow to avoid copying the buffer out.
        MATCH_EXPR_BUFFER.with(|buf_cell| {
            let mut buf = buf_cell.borrow_mut();

            // Check the MORK bytes cache first. Content-hash keyed: survives GC safepoints,
            // shares entries across structurally-identical expressions at different addresses.
            let cache_key = expr.hash_value();
            let expr_arity = arity;
            let cache_hit = MORK_BYTES_CACHE.with(|cache_cell| {
                let mut cache = cache_cell.borrow_mut();
                if let Some(entry) = cache.get(&cache_key) {
                    // Cheap collision validation: arity must match
                    if entry.1 == expr_arity {
                        buf.clear();
                        buf.reserve(entry.0.len());
                        buf.extend_from_slice(&entry.0);
                        return true;
                    }
                }
                false
            });

            let serialize_ok = if cache_hit {
                Ok(())
            } else {
                let result = with_mork_bytes(expr, &self.shared_mapping, self.mork_cache_epoch, |bytes| {
                    buf.clear();
                    buf.reserve(bytes.len() + 1);
                    buf.extend_from_slice(bytes);
                    buf.push(0x00); // Padding byte for ExprZipper read-past-end safety
                });
                // Cache the serialized bytes on success
                if result.is_ok() {
                    MORK_BYTES_CACHE.with(|cache_cell| {
                        cache_cell.borrow_mut().put(cache_key, (buf.clone(), expr_arity));
                    });
                }
                result
            };

            if serialize_ok.is_err() {
                // MORK can't encode this expression (e.g., a child S-expression
                // has arity >= 64). Try structural matchers first, then use Wide
                // MORK byte-level matching for remaining candidates.
                drop(buf);

                let rule_index = self.shared.rule_index.read();
                let candidates: Vec<&RuleEntry<V>> = if !head.is_empty() {
                    rule_index.get_candidates(head, arity, first_arg_head).collect()
                } else {
                    rule_index.get_all_rules().collect()
                };

                let mut results: Vec<RuleMatchResult<V>> = Vec::new();

                // Lazily-computed wide storage encoding (only allocated if needed)
                let mut expr_wide_buf: Option<Vec<u8>> = None;

                for entry in candidates {
                    // Try structural matcher first (works regardless of MORK encoding)
                    if let Some(ref matcher) = entry.structural_matcher {
                        if let Some(bindings) = matcher.try_match(expr) {
                            let instantiated_rhs = if entry.rhs_has_variables {
                                apply_bindings(&entry.rhs, &bindings, &self.factory)
                            } else {
                                entry.rhs.clone()
                            };
                            let multiplicity = entry.multiplicity.max(1);
                            if multiplicity == 1 {
                                results.push(RuleMatchResult {
                                    instantiated_rhs,
                                    rhs_template: entry.rhs.clone(),
                                    bindings,
                                    multiplicity: 1,
                                    rhs_type: entry.rhs_type.clone(),
                                    rhs_has_variables: entry.rhs_has_variables,
                                });
                            } else {
                                for _ in 0..multiplicity {
                                    results.push(RuleMatchResult {
                                        instantiated_rhs: instantiated_rhs.clone(),
                                        rhs_template: entry.rhs.clone(),
                                        bindings: bindings.clone(),
                                        multiplicity,
                                        rhs_type: entry.rhs_type.clone(),
                                        rhs_has_variables: entry.rhs_has_variables,
                                    });
                                }
                            }
                        }
                        continue; // Structural matcher is authoritative
                    }

                    // Fallback: wide MORK byte-level matching or pattern_match_generic
                    let matched_bindings = if !entry.lhs_wide_debruijn.is_empty() {
                        let wide_data = expr_wide_buf.get_or_insert_with(|| {
                            let mut wb = Vec::with_capacity(256);
                            crate::backend::wide_mork::encoding::encode_wide_storage(expr, &mut wb);
                            wb
                        });
                        if crate::backend::wide_mork::extract::wide_extract_data(
                            &entry.lhs_wide_debruijn,
                            wide_data,
                        ).is_ok() {
                            Some(extract_bindings_from_wide_expr(
                                &entry.lhs_wide_debruijn,
                                expr,
                                &entry.var_names,
                                &entry.wildcard_indices,
                            ))
                        } else {
                            None
                        }
                    } else {
                        // Structural pattern match fallback for MORK-only candidates
                        crate::backend::eval::trampoline::pattern_match_generic(&entry.lhs, expr)
                    };

                    if let Some(bindings) = matched_bindings {
                        let instantiated_rhs = if entry.rhs_has_variables {
                            apply_bindings(&entry.rhs, &bindings, &self.factory)
                        } else {
                            entry.rhs.clone()
                        };
                        let multiplicity = entry.multiplicity.max(1);
                        if multiplicity == 1 {
                            results.push(RuleMatchResult {
                                instantiated_rhs,
                                rhs_template: entry.rhs.clone(),
                                bindings,
                                multiplicity: 1,
                                rhs_type: entry.rhs_type.clone(),
                                rhs_has_variables: entry.rhs_has_variables,
                            });
                        } else {
                            for _ in 0..multiplicity {
                                results.push(RuleMatchResult {
                                    instantiated_rhs: instantiated_rhs.clone(),
                                    rhs_template: entry.rhs.clone(),
                                    bindings: bindings.clone(),
                                    multiplicity,
                                    rhs_type: entry.rhs_type.clone(),
                                    rhs_has_variables: entry.rhs_has_variables,
                                });
                            }
                        }
                    }
                }
                return results;
            }

            // Validate expr buffer contains valid MORK bytes (no reserved 0x40-0x7F)
            #[cfg(debug_assertions)]
            {
                // Validate excluding the padding byte
                if let Err((off, byte)) = validate_mork_bytes(&buf[..buf.len() - 1]) {
                    panic!(
                        "expr buffer has invalid byte 0x{:02x} at offset {} (len={}).\n\
                         expr_bytes: {:02x?}\n\
                         expr: {:?}",
                        byte, off, buf.len() - 1,
                        &buf[..buf.len().min(64)],
                        expr
                    );
                }
            }

            // Get candidates from RuleIndex (read lock — multiple concurrent readers OK)
            let rule_index = self.shared.rule_index.read();

            // Phase 1: Byte-level pattern matching via extract_data.
            // Store entry references directly in MatchHit, eliminating the intermediate
            // candidates Vec allocation (which can hold 1000s of entries for large programs).
            //
            // In the mixed path (some candidates have structural matchers, some don't),
            // structural matchers are tried first per-candidate. If a structural matcher
            // matches, its pre-computed bindings are stored in the hit to avoid redundant
            // extract_bindings_from_expr calls.
            struct MatchHit<'a, V: MettaValueTrait + Clone> {
                entry: &'a RuleEntry<V>,
                is_wide: bool,  // true if matched via Wide MORK
                /// Pre-computed bindings from structural matcher (None = use MORK extraction)
                precomputed_bindings: Option<GenericBindings<V>>,
            }

            let mut hits: Vec<MatchHit<'_, V>> = Vec::new();

            // Lazily-computed wide storage encoding of expr (only allocated if needed)
            let mut expr_wide_storage: Option<Vec<u8>> = None;

            // Phase 7: Borrow the thread-local ExprZipper for reuse across match candidates.
            // reset() uses set_len(0) to clear the trace Vec without deallocating — after
            // warmup, the capacity grows to max expression depth and all subsequent matches
            // are zero-alloc (no jemalloc calls for Vec<Breadcrumb>).
            MATCH_ZIPPER.with(|zipper_cell| {
            let mut input_zipper = zipper_cell.borrow_mut();

            // Inline macro to avoid duplicating the match body for both iterator paths
            macro_rules! try_match_entry {
                ($entry:expr) => {
                    let entry = $entry;

                    // Try structural matcher first (avoids MORK byte-level matching)
                    if let Some(ref matcher) = entry.structural_matcher {
                        if let Some(bindings) = matcher.try_match(expr) {
                            hits.push(MatchHit { entry, is_wide: false, precomputed_bindings: Some(bindings) });
                        }
                        // Structural matcher is authoritative — skip MORK
                    } else if !entry.lhs_debruijn.is_empty() {
                        // MORK narrow path fallback (arity < 64)
                        if let Err(reserved) = maybe_byte_item(entry.lhs_debruijn[0]) {
                            tracing::warn!(
                                target: "mettatron::match_rules_native",
                                "RuleEntry has invalid first byte 0x{:02x} in lhs_debruijn (len={}), \
                                 head={}, arity={}, lhs_bytes={:02x?}",
                                reserved,
                                entry.lhs_debruijn.len(),
                                head,
                                arity,
                                &entry.lhs_debruijn[..entry.lhs_debruijn.len().min(16)]
                            );
                        } else {
                            let lhs_expr = Expr { ptr: entry.lhs_debruijn.as_ptr().cast_mut() };
                            // Reuse thread-local zipper — reset() preserves Vec capacity
                            input_zipper.root = Expr { ptr: buf.as_ptr().cast_mut() };
                            input_zipper.reset();
                            if lhs_expr.extract_data(&mut input_zipper).is_ok() {
                                hits.push(MatchHit { entry, is_wide: false, precomputed_bindings: None });
                            }
                        }
                    } else if !entry.lhs_wide_debruijn.is_empty() {
                        // Wide MORK path: encode expr to wide storage bytes (lazy) and match
                        let wide_data = expr_wide_storage.get_or_insert_with(|| {
                            let mut wide_buf = Vec::with_capacity(256);
                            crate::backend::wide_mork::encoding::encode_wide_storage(expr, &mut wide_buf);
                            wide_buf
                        });
                        if crate::backend::wide_mork::extract::wide_extract_data(
                            &entry.lhs_wide_debruijn,
                            wide_data,
                        ).is_ok() {
                            hits.push(MatchHit { entry, is_wide: true, precomputed_bindings: None });
                        }
                    }
                    // else: both empty — skip (shouldn't happen)
                };
            }

            if !head.is_empty() {
                for entry in rule_index.get_candidates(head, arity, first_arg_head) {
                    try_match_entry!(entry);
                }
            } else {
                for entry in rule_index.get_all_rules() {
                    try_match_entry!(entry);
                }
            }

            }); // end MATCH_ZIPPER.with — drop zipper borrow before Phase 3

            if hits.is_empty() {
                return Vec::new();
            }

            // Phase 3: Extract bindings from original expression and build results.
            // Uses parallel tree walk instead of MORK deserialization to preserve
            // runtime types (SpaceHandle, State, etc.) that can't survive a round trip.
            // Structural matcher hits already have pre-computed bindings — skip extraction.
            let mut results: Vec<RuleMatchResult<V>> = Vec::with_capacity(hits.len());

            for hit in &hits {
                let entry = hit.entry;

                // Use pre-computed bindings from structural matcher, or extract from MORK
                let bindings = if let Some(ref precomputed) = hit.precomputed_bindings {
                    precomputed.clone()
                } else if hit.is_wide {
                    extract_bindings_from_wide_expr(
                        &entry.lhs_wide_debruijn,
                        expr,
                        &entry.var_names,
                        &entry.wildcard_indices,
                    )
                } else {
                    extract_bindings_from_expr(
                        &entry.lhs_debruijn,
                        expr,
                        &entry.var_names,
                        &entry.wildcard_indices,
                    )
                };

                // Apply bindings to the cached RHS template.
                // Phase 6: Skip apply_bindings for ground RHS (no variables).
                let instantiated_rhs = if entry.rhs_has_variables {
                    apply_bindings(&entry.rhs, &bindings, &self.factory)
                } else {
                    entry.rhs.clone()
                };

                // Expand by multiplicity — fast path for common case (multiplicity=1)
                // avoids cloning bindings/instantiated_rhs when a move suffices
                let multiplicity = entry.multiplicity.max(1);
                if multiplicity == 1 {
                    results.push(RuleMatchResult {
                        instantiated_rhs,
                        rhs_template: entry.rhs.clone(),
                        bindings,
                        multiplicity: 1,
                        rhs_type: entry.rhs_type.clone(),
                        rhs_has_variables: entry.rhs_has_variables,
                    });
                } else {
                    for _ in 0..multiplicity {
                        results.push(RuleMatchResult {
                            instantiated_rhs: instantiated_rhs.clone(),
                            rhs_template: entry.rhs.clone(),
                            bindings: bindings.clone(),
                            multiplicity,
                            rhs_type: entry.rhs_type.clone(),
                            rhs_has_variables: entry.rhs_has_variables,
                        });
                    }
                }
            }

            results
        })
    }

    /// Get matching rules for an expression from PathMap via trie prefix navigation.
    ///
    /// **NOTE**: Superseded by `match_rules_native()` for the primary hot path.
    /// Retained for `match_space` compatibility and fallback scenarios where
    /// De Bruijn-encoded rules in PathMap need to be iterated directly.
    ///
    /// Returns `(lhs, rhs, multiplicity)` tuples for all rules whose LHS
    /// head symbol and arity match the given expression. The caller is
    /// responsible for performing full pattern matching on the returned
    /// candidates.
    pub fn get_matching_rules_for_expr(&self, expr: &V) -> Vec<(V, V, u64)> {
        let head = expr.get_head_symbol().unwrap_or("");
        let arity = expr.get_arity();

        // Bloom filter O(1) rejection: skip when no head-specific rules exist
        // AND there are no wildcard rules (which match any head).
        if !head.is_empty() {
            let bloom_says_no = !self
                .shared
                .atom_space.head_arity_bloom
                .read()
                .may_contain(head.as_bytes(), arity as u8);
            if bloom_says_no && !self.shared.rule_index.read().has_wildcard_rules() {
                return Vec::new();
            }
        }

        let space = self.create_space();
        let rule_prefix_len = super::generic::RULE_PREFIX_LEN;
        let rule_prefix = self.compute_rule_prefix();
        let mut rules: Vec<(V, V, u64)> = Vec::new();

        // 1. Head+arity-specific prefix navigation (most selective)
        if !head.is_empty() {
            if let Some(head_prefix) = build_head_arity_prefix::<V, F>(
                &rule_prefix,
                head,
                arity,
                &self.factory,
                &self.shared_mapping,
                self.mork_cache_epoch,
            ) {
                self.collect_rules_from_prefix(
                    &space,
                    &head_prefix,
                    rule_prefix_len,
                    &mut rules,
                );
            }
        } else {
            // No head info — collect all rules under the rule prefix
            self.collect_rules_from_prefix(
                &space,
                &rule_prefix,
                rule_prefix_len,
                &mut rules,
            );
        }

        // 2. Collect wildcard rules (LHS is atom/variable, not S-expression)
        self.collect_wildcard_rules(&space, &rule_prefix, rule_prefix_len, head, arity, &mut rules);

        rules
    }

    /// Collect rules from a trie subtree rooted at `prefix`.
    ///
    /// **NOTE**: Superseded by `RuleIndex + extract_data()` for the primary hot path.
    /// Retained for `get_matching_rules_for_expr()` fallback used in tests.
    ///
    /// Navigates the trie to `prefix`, then for each entry:
    /// 1. Splits the MORK path bytes into LHS and RHS ranges using `mork_expr_byte_len()`
    /// 2. Deserializes LHS and RHS independently — never constructs `(= LHS RHS)`
    /// 3. Reads multiplicity in-place from zipper `val()`
    fn collect_rules_from_prefix(
        &self,
        space: &Space<Multiplicity>,
        prefix: &[u8],
        rule_prefix_len: usize,
        rules: &mut Vec<(V, V, u64)>,
    ) {
        let mut rz = space.btm.read_zipper();
        let descended = rz.descend_to_existing(prefix);
        if descended < prefix.len() {
            return; // Prefix doesn't exist in trie
        }
        while rz.to_next_val() {
            let path = rz.path();
            if !path.starts_with(prefix) {
                break; // Left the subtree
            }
            // Multiplicity directly from zipper value (no separate lookup)
            let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1).max(1);

            // Split path into LHS and RHS byte ranges.
            // path layout: [Arity(3)] ["=" bytes] [LHS bytes] [RHS bytes]
            //              |--- rule_prefix_len --|
            if path.len() <= rule_prefix_len {
                continue; // Path too short to contain LHS+RHS
            }
            // De Bruijn encoding: NewVar is in LHS, VarRef in RHS references LHS vars.
            // Must deserialize the FULL rule (= lhs rhs) as a single unit to share
            // the variable context, then extract lhs and rhs from the result.
            let full_rule = match mork_bytes_to_generic_value::<V, F, Multiplicity>(
                path,
                space,
                &self.factory,
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (lhs, rhs) = match extract_rule_parts(&full_rule) {
                Some(parts) => parts,
                None => continue, // Not a valid rule — skip
            };

            rules.push((lhs, rhs, multiplicity));
        }
    }

    /// Collect wildcard rules (LHS is atom/variable, not S-expression).
    ///
    /// **NOTE**: Superseded by `RuleIndex.wildcard` vec for the primary hot path.
    /// Retained for `get_matching_rules_for_expr()` fallback used in tests.
    ///
    /// Wildcard rules like `(= $x $x)` have a non-S-expression LHS. Their LHS byte
    /// starts with `SymbolSize` (0xC1-0xFF), `NewVar` (0xC0), or `VarRef` (0x80-0xBF),
    /// not `Arity` (0x00-0x3F). After collecting head-specific matches, this scans
    /// the rule prefix subtree for non-Arity LHS entries.
    ///
    /// Rules are filtered by head+arity to match the old behavior:
    /// - Variable LHS (no head symbol) matches everything
    /// - Atom LHS with a specific head matches only when head+arity agree
    fn collect_wildcard_rules(
        &self,
        space: &Space<Multiplicity>,
        rule_prefix: &[u8],
        rule_prefix_len: usize,
        query_head: &str,
        query_arity: usize,
        rules: &mut Vec<(V, V, u64)>,
    ) {
        let mut rz = space.btm.read_zipper();
        let descended = rz.descend_to_existing(rule_prefix);
        if descended < rule_prefix.len() {
            return;
        }
        while rz.to_next_val() {
            let path = rz.path();
            if !path.starts_with(rule_prefix) {
                break;
            }
            if path.len() <= rule_prefix_len {
                continue;
            }
            let lhs_first_byte = path[rule_prefix_len];
            // Arity tag (0x00-0x3F) = S-expr LHS → skip (handled by head_prefix or rule_prefix)
            if (lhs_first_byte & 0b1100_0000) == 0b0000_0000 {
                continue;
            }

            // Atom/variable LHS — potential wildcard rule.
            // De Bruijn encoding: deserialize full rule as single unit for shared var context.
            let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1).max(1);

            let full_rule = match mork_bytes_to_generic_value::<V, F, Multiplicity>(
                path,
                space,
                &self.factory,
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (lhs, rhs) = match extract_rule_parts(&full_rule) {
                Some(parts) => parts,
                None => continue,
            };

            // Filter by head+arity: variable LHS (no head) matches everything,
            // atom LHS with a specific head must match the query head+arity.
            let rule_head = lhs.get_head_symbol().unwrap_or("");
            if !rule_head.is_empty()
                && (rule_head != query_head || lhs.get_arity() != query_arity)
            {
                continue;
            }

            rules.push((lhs, rhs, multiplicity));
        }
    }
}

// ============================================================================
// MettaEnvironment-specific Rule Operations
// ============================================================================

impl MettaEnvironment {
    /// Get the number of rules in the environment.
    pub fn rule_count(&self) -> usize {
        let space = self.create_space();
        let mut count = 0;

        for (path_bytes, _) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if Self::is_rule_sexpr(&value) {
                    count += 1;
                }
            }
        }

        count
    }

    /// Iterator over rule heads with their arities and counts.
    pub fn iter_rule_heads(&self) -> RuleHeadsIter {
        let space = self.create_space();
        let mut head_map: HashMap<(String, usize), usize> = HashMap::new();

        for (path_bytes, _) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, _rhs)) = extract_rule_parts(&value) {
                    let head = lhs.get_head_symbol().unwrap_or("").to_string();
                    let arity = lhs.get_arity();
                    *head_map.entry((head, arity)).or_insert(0) += 1;
                }
            }
        }

        let items: Vec<(String, usize, usize)> = head_map
            .into_iter()
            .map(|((head, arity), count)| (head, arity, count))
            .collect();

        RuleHeadsIter::new(items)
    }

    /// Collect all rules as (lhs, rhs) pairs.
    pub fn collect_rules(&self) -> Vec<(MettaValue, MettaValue)> {
        let space = self.create_space();
        let mut rules = Vec::new();

        for (path_bytes, _) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, rhs)) = extract_rule_parts(&value) {
                    rules.push((lhs, rhs));
                }
            }
        }

        rules
    }

    /// Bulk add rules using De Bruijn encoding + RuleIndex population.
    ///
    /// Each rule is serialized with `with_mork_query_bytes` (De Bruijn) and inserted
    /// individually into PathMap + RuleIndex. This is consistent with `add_rule()`.
    ///
    /// # Arguments
    /// * `rules` - Vec of (lhs, rhs) pairs
    pub fn add_rules_bulk(&mut self, rules: Vec<(MettaValue, MettaValue)>) -> Result<(), String> {
        trace!(target: "mettatron::environment::add_rules_bulk", rule_count = rules.len());
        if rules.is_empty() {
            return Ok(());
        }

        self.make_owned();

        // Delegate to add_rule() for each — ensures consistent De Bruijn encoding + RuleIndex
        for (lhs, rhs) in rules {
            self.add_rule(lhs, rhs);
        }

        self.modified.store(true, Ordering::Release);
        Ok(())
    }

    /// Get the number of times a rule has been defined (multiplicity).
    ///
    /// Uses De Bruijn encoding to match the PathMap entry (rules are stored with De Bruijn).
    pub fn get_rule_count(&self, lhs: &MettaValue, rhs: &MettaValue) -> usize {
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            lhs.clone(),
            rhs.clone(),
        ]);

        let sm = self.shared_mapping.clone();
        match with_mork_query_bytes(&rule_sexpr, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
            let btm = self.shared.atom_space.btm.read();
            let count = get_multiplicity(&btm, mork_bytes);
            if count == 0 { 1 } else { count as usize }
        }) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — check wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(&rule_sexpr, &mut wide_key);
                let wbtm = self.shared.atom_space.wide_btm.read();
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                if count == 0 { 1 } else { count as usize }
            }
        }
    }

    /// Get the multiplicities (for serialization).
    /// The keys are hex-encoded MORK bytes for serialization stability.
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        let btm = self.shared.atom_space.btm.read();
        let mut result = HashMap::new();

        for (path, multiplicity) in btm.iter() {
            let count = multiplicity.count() as usize;
            let count = if count == 0 { 1 } else { count };
            let hex_key = hex::encode(&path);
            result.insert(hex_key, count);
        }

        result
    }

    /// Rebuild bloom filter, fuzzy matcher, and RuleIndex from PathMap.
    ///
    /// This is needed after deserializing an Environment from PathMap Par,
    /// since the serialization only preserves the PathMap, not the bloom filter
    /// or RuleIndex.
    ///
    /// ## De Bruijn Encoding
    ///
    /// PathMap stores De Bruijn-encoded bytes for rules. When deserialized, variables
    /// get epoch-suffixed names (`$a%42` instead of original `$x`). These are
    /// re-encoded to De Bruijn bytes (structurally identical) when rebuilding the RuleIndex.
    pub fn rebuild_bloom_filter(&mut self) {
        trace!(target: "mettatron::environment::rebuild_bloom_filter", "Rebuilding bloom filter, fuzzy matcher, and RuleIndex from PathMap");
        self.make_owned();

        // Clear the existing RuleIndex before rebuilding
        self.shared.rule_index.write().clear();

        let space = self.create_space();
        let rule_prefix_len = super::generic::RULE_PREFIX_LEN;

        for (path_bytes, multiplicity_val) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, rhs)) = extract_rule_parts(&value) {
                    // Update bloom filter + fuzzy matcher
                    let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
                    let arity = lhs.get_arity();
                    if let Some(ref head) = head_owned {
                        self.shared.fuzzy_matcher.write().insert(head);
                        self.shared
                            .atom_space.head_arity_bloom
                            .write()
                            .insert(head.as_bytes(), arity as u8);
                    }

                    // Rebuild RuleIndex entry from De Bruijn bytes
                    let multiplicity = multiplicity_val.count().max(1);

                    // Re-serialize the rule to get De Bruijn bytes + ConversionContext
                    let rule_sexpr = MettaValue::SExpr(vec![
                        MettaValue::Atom("=".to_string()),
                        lhs.clone(),
                        rhs.clone(),
                    ]);

                    let sm = self.shared_mapping.clone();
                    let _ = with_mork_query_bytes(&rule_sexpr, &sm, self.mork_cache_epoch, |debruijn_bytes, ctx| {
                        // Split De Bruijn bytes to get LHS range
                        if debruijn_bytes.len() <= rule_prefix_len {
                            return;
                        }
                        let lhs_start = rule_prefix_len;
                        let lhs_byte_len = mork_expr_byte_len(&debruijn_bytes[lhs_start..]);
                        // Pad with 0x00 for ExprZipper read-past-end safety
                        let mut lhs_debruijn = Vec::with_capacity(lhs_byte_len + 1);
                        lhs_debruijn.extend_from_slice(&debruijn_bytes[lhs_start..lhs_start + lhs_byte_len]);
                        lhs_debruijn.push(0x00);

                        let lhs_var_count = count_newvar_tags(&lhs_debruijn);
                        let (var_names, wildcard_indices) =
                            build_var_names_and_wildcards(&ctx.var_names, lhs_var_count);

                        // Phase 8.1: Compute RHS type for branch pruning
                        let rhs_type = {
                            use crate::backend::eval::types_generic::infer_type_generic;
                            let inferred = infer_type_generic(&rhs, &self.factory, self);
                            if inferred.as_atom() == Some("%Undefined%") { None } else { Some(inferred) }
                        };

                        // Phase 10.1: Register inferred return type (bulk path)
                        if let Some(ref rt) = rhs_type {
                            if let Some(ref head) = head_owned {
                                self.register_inferred_type(head, rt);
                            }
                        }

                        // Phase 10.4: Synthesize arrow type (bulk path)
                        if let Some(ref head) = head_owned {
                            let has_declared_arrow = self.get_types_generic(head).iter().any(|t| {
                                t.as_sexpr()
                                    .and_then(|items| items.first().and_then(|v| v.as_atom()))
                                    == Some("->")
                            });
                            if !has_declared_arrow {
                                use crate::backend::eval::types_generic::infer_arrow_type_from_rule;
                                if let Some(arrow) = infer_arrow_type_from_rule(
                                    &lhs,
                                    &rhs,
                                    rhs_type.as_ref(),
                                    &self.factory,
                                    self,
                                ) {
                                    self.register_inferred_type(head, &arrow);
                                }
                            }
                        }

                        let structural_matcher = StructuralMatcher::analyze(&lhs);
                        let entry = RuleEntry {
                            lhs: lhs.clone(),
                            rhs_has_variables: rhs.contains_variables(),
                            rhs: rhs.clone(),
                            lhs_debruijn,
                            lhs_wide_debruijn: Vec::new(), // Bulk path uses MORK encoding
                            var_names,
                            wildcard_indices,
                            multiplicity,
                            rhs_type,
                            structural_matcher,
                        };

                        // Phase 4a: Pre-seed tiered cache for bulk path
                        crate::backend::bytecode::tiered_cache::global_tiered_cache()
                            .preseed_for_immediate_compile(rhs.hash_value());

                        // Set correct multiplicity (don't let add_rule deduplicate)
                        let alloc = crate::backend::models::gc_allocator::global_allocator();
                        let first_arg_head_interned: Option<&'static str> =
                            get_first_arg_head(&lhs).map(|s| alloc.alloc_str(s));
                        self.shared.rule_index.write().add_rule(
                            head_owned.as_deref(),
                            arity,
                            first_arg_head_interned,
                            entry,
                        );
                    });
                }
            }
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Increment the multiplicity count for a rule.
    ///
    /// Uses De Bruijn encoding to match the PathMap entry (rules are stored with De Bruijn).
    /// Also syncs the RuleIndex multiplicity.
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after increment.
    pub fn increment_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::increment_rule_multiplicity", ?rule_sexpr);
        self.make_owned();

        let sm = self.shared_mapping.clone();
        match with_mork_query_bytes(rule_sexpr, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
            let mut btm = self.shared.atom_space.btm.write();
            let new_count = increment_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => {
                // Sync RuleIndex: increment the matching entry's multiplicity
                if let Some((lhs, rhs)) = extract_rule_parts(rule_sexpr) {
                    // RuleIndex.add_rule increments multiplicity for duplicates
                    // We need a simpler increment — just find and bump
                    let mut idx = self.shared.rule_index.write();
                    let mut found = false;
                    'outer: for group in idx.by_head_arity.values_mut() {
                        for entry in group.all_entries_mut() {
                            if entry.lhs == lhs && entry.rhs == rhs {
                                entry.multiplicity += 1;
                                found = true;
                                break 'outer;
                            }
                        }
                    }
                    if !found {
                        if let Some(entry) = idx.wildcard.iter_mut().find(|e| e.lhs == lhs && e.rhs == rhs) {
                            entry.multiplicity += 1;
                        }
                    }
                }
                count
            }
            Err(_) => {
                self.modified.store(true, Ordering::Release);
                1
            }
        }
    }

    /// Decrement the multiplicity count for a rule.
    ///
    /// Uses De Bruijn encoding to match the PathMap entry (rules are stored with De Bruijn).
    /// Also syncs the RuleIndex multiplicity.
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after decrement, or 0 if the rule wasn't tracked.
    pub fn decrement_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::decrement_rule_multiplicity", ?rule_sexpr);
        self.make_owned();

        let sm = self.shared_mapping.clone();
        match with_mork_query_bytes(rule_sexpr, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
            let old_count = {
                let btm = self.shared.atom_space.btm.read();
                get_multiplicity(&btm, mork_bytes)
            };

            if old_count == 0 {
                self.modified.store(true, Ordering::Release);
                return 0;
            }

            let mut btm = self.shared.atom_space.btm.write();
            let new_count = decrement_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => {
                // Sync RuleIndex: decrement (or remove if multiplicity reaches 0)
                if let Some((lhs, rhs)) = extract_rule_parts(rule_sexpr) {
                    self.shared.rule_index.write().remove_rule(&lhs, &rhs);
                }
                count
            }
            Err(_) => {
                self.modified.store(true, Ordering::Release);
                0
            }
        }
    }

    /// Check if a MettaValue is a rule s-expression (= lhs rhs)
    pub fn is_rule_sexpr(value: &MettaValue) -> bool {
        if let ValueView::SExpr(items) = value.view() {
            if items.len() == 3 {
                if let ValueView::Atom(op) = items[0].view() {
                    return op == "=";
                }
            }
        }
        false
    }

    // ========================================================================
    // All-Atom Multiplicity Tracking (MeTTa HE Semantics)
    // ========================================================================

    /// Increment multiplicity for ANY atom (not just rules).
    pub fn increment_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned();

        match with_mork_bytes(value, &self.shared_mapping, self.mork_cache_epoch, |mork_bytes| {
            let mut btm = self.shared.atom_space.btm.write();
            let new_count = increment_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — use wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                super::multiplicity::add_atom(&mut wbtm, &wide_key);
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                drop(wbtm);

                self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                count as usize
            }
        }
    }

    /// Decrement multiplicity for ANY atom.
    pub fn decrement_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned();

        match with_mork_bytes(value, &self.shared_mapping, self.mork_cache_epoch, |mork_bytes| {
            let old_count = {
                let btm = self.shared.atom_space.btm.read();
                get_multiplicity(&btm, mork_bytes)
            };

            if old_count == 0 {
                return 0;
            }

            let mut btm = self.shared.atom_space.btm.write();
            let new_count = decrement_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — use wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                if count > 0 {
                    super::multiplicity::remove_atom(&mut wbtm, &wide_key);
                    drop(wbtm);
                    self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
                    self.modified.store(true, Ordering::Release);
                }
                count.saturating_sub(1) as usize
            }
        }
    }

    /// Get multiplicity for ANY atom.
    pub fn get_atom_multiplicity(&self, value: &MettaValue) -> usize {
        // Rules are stored with De Bruijn encoding, so we must look them up the same way.
        if extract_rule_parts(value).is_some() {
            let sm = self.shared_mapping.clone();
            match with_mork_query_bytes(value, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
                let btm = self.shared.atom_space.btm.read();
                let count = get_multiplicity(&btm, mork_bytes);
                if count == 0 { 1 } else { count as usize }
            }) {
                Ok(count) => count,
                Err(_) => {
                    // Wide expression (arity >= 64) — check wide_btm
                    let mut wide_key = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                    let wbtm = self.shared.atom_space.wide_btm.read();
                    let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                    if count == 0 { 1 } else { count as usize }
                }
            }
        } else {
            match with_mork_bytes(value, &self.shared_mapping, self.mork_cache_epoch, |mork_bytes| {
                let btm = self.shared.atom_space.btm.read();
                let count = get_multiplicity(&btm, mork_bytes);
                if count == 0 { 1 } else { count as usize }
            }) {
                Ok(count) => count,
                Err(_) => {
                    // Wide expression (arity >= 64) — check wide_btm
                    let mut wide_key = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                    let wbtm = self.shared.atom_space.wide_btm.read();
                    let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                    if count == 0 { 1 } else { count as usize }
                }
            }
        }
    }

    /// Get atom multiplicity from raw MORK bytes.
    pub fn get_multiplicity_from_mork_bytes(&self, mork_bytes: &[u8]) -> usize {
        let btm = self.shared.atom_space.btm.read();
        let count = get_multiplicity(&btm, mork_bytes);
        if count == 0 { 1 } else { count as usize }
    }

    /// Get the count of distinct wide atoms (arity >= 64) stored in wide_btm.
    /// Returns 0 if no wide atoms exist.
    pub fn get_wide_atom_count(&self) -> usize {
        use pathmap::zipper::ZipperIteration;
        let wbtm = self.shared.atom_space.wide_btm.read();
        let mut count = 0usize;
        let mut rz = wbtm.read_zipper();
        while rz.to_next_val() {
            count += 1;
        }
        count
    }
}
