//! Rule management operations for Environment.
//!
//! Provides methods for adding, indexing, and querying rules.
//! Rules are stored as (= lhs rhs) in MettaTrie via TrieKey decomposition.
//!
//! # Multiplicity Tracking
//!
//! Rules can be defined multiple times, and we track multiplicities efficiently
//! using MettaTrie<V, Multiplicity> — each TrieKey path maps to its multiplicity count.
//!
//! # Rule Discovery
//!
//! Rules are discovered via a two-level index:
//!
//! 1. **Bloom filter** — O(1) rejection for non-matching head/arity combinations
//! 2. **RuleIndex** — HashMap-backed `(head, arity) → Vec<RuleEntry>` for O(1) candidate lookup
//! 3. **StructuralMatcher / EnhancedMatcher** — direct MettaValue pattern matching per candidate
//! 4. **MettaValue binding application** — `apply_bindings_generic()` on cached RHS template
//!
//! Rules are stored in MettaTrie with literal decomposition (via `decompose_literal()`).
//! The RuleIndex caches metadata at insertion time for zero-deserialization matching.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Gate for I-13/I-14 hot-path analysis tracking.
/// When false (default), `with_incremental_index` and `with_adaptive_registry`
/// calls in `match_rules_native` are skipped — a single branch-predicted
/// `load(Relaxed)` check. Set to true via `activate_analysis()` when
/// `METTATRON_AAM_ANALYSIS=1`.
static ANALYSIS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Enable I-13/I-14 analysis tracking on the hot path.
pub fn activate_analysis() {
    ANALYSIS_ACTIVE.store(true, Ordering::Relaxed);
}

/// Check whether analysis tracking is active.
#[inline(always)]
pub fn is_analysis_active() -> bool {
    ANALYSIS_ACTIVE.load(Ordering::Relaxed)
}

/// I-12: Global monotonic counter for assigning unique global rule indices.
/// Used by CompressedRuleFilter to identify dead rules across all groups.
static GLOBAL_RULE_COUNTER: AtomicU32 = AtomicU32::new(0);

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

use smallvec::SmallVec;
use tracing::trace;

use super::generic::GenericEnvironment;
use super::multiplicity;
use super::{MettaEnvironment, MettaValue};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait, ValueView};

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
// RuleIndex — In-memory index for O(1) rule lookup + structural matching
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
/// Caches both the original MettaValues (for display, debugging, bytecode VM) and
/// metadata for structural pattern matching.
#[derive(Debug, Clone)]
pub(crate) struct RuleEntry<V: MettaValueTrait + Clone> {
    // --- Cached MettaValues (original variable names) ---
    /// LHS pattern with original variable names (for display, debugging)
    pub lhs: V,
    /// RHS template with original variable names (for bytecode compilation/caching)
    pub rhs: V,

    // --- Legacy field (kept for ABI compatibility, always empty) ---
    /// Formerly held De Bruijn encoded bytes. Now always empty `vec![]`.
    /// The StructuralMatcher and EnhancedMatcher handle matching without these.
    pub lhs_debruijn: Vec<u8>,

    // --- Metadata ---
    /// De Bruijn index → original variable name (e.g., "$x", "$y")
    /// Only contains variables from LHS (used for building named bindings).
    /// Interned as `&'static str` via slab allocator to avoid per-match String clones.
    pub var_names: Vec<&'static str>,
    /// Indices of `_` wildcards (skip these in named bindings)
    pub wildcard_indices: SmallVec<[u8; 4]>,
    // NOTE: The old `specificity` field (count of NewVar tags) was removed.
    // MeTTa HE has NO specificity filter — all matching rules fire nondeterministically.
    /// How many times this rule was added (synced with MettaTrie multiplicity)
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
    /// `None` for rules too complex for structural matching — falls back to pattern_match_generic.
    pub structural_matcher: Option<StructuralMatcher>,
    /// Enhanced matcher for deep patterns (depth > 8) that StructuralMatcher rejects (I-2).
    /// Provides indexed binding slots and no depth limit.
    pub enhanced_matcher: Option<crate::backend::eval::cesk::EnhancedMatcher>,
    /// Monotonic index within the RuleGroup (for discrimination tree indexing, I-1).
    pub rule_index_in_group: u32,
    /// I-12: Global monotonic rule index (for CompressedRuleFilter dead-rule filtering).
    pub global_rule_index: u32,
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
/// `|-`); this reduces the inner match loop by 3-10x.
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
    /// Discrimination tree for multi-level candidate pruning (Phase 1.3 / I-1).
    /// Built when the group has 4+ rules. Prunes candidates before structural
    /// matching, reducing the number of try_match() invocations.
    disc_tree: Option<crate::backend::eval::cesk::DiscriminationTree>,
    /// Monotonic counter for assigning rule_index_in_group to new entries.
    next_rule_index: u32,
}

impl<V: MettaValueTrait + Clone> RuleGroup<V> {
    fn new() -> Self {
        RuleGroup {
            by_first_arg_head: HashMap::new(),
            variable_first_arg: Vec::new(),
            disc_tree: None,
            next_rule_index: 0,
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

    /// Invalidate the discrimination tree (force rebuild on next query).
    /// Called after rule removal.
    fn invalidate_disc_tree(&mut self) {
        self.disc_tree = None;
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

/// Lightweight in-memory index for O(1) rule lookup + structural matching.
///
/// Populated at `add_rule()` time. Authoritative source for rule queries.
/// MettaTrie remains the storage-of-record (for `match_space`, serialization).
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

                // I-1: Assign monotonic rule index and insert into disc tree
                let mut entry = entry;
                let idx = group.next_rule_index;
                entry.rule_index_in_group = idx;
                entry.global_rule_index = GLOBAL_RULE_COUNTER.fetch_add(1, Ordering::Relaxed);
                group.next_rule_index += 1;

                // I-1: Build/update discrimination tree for multi-level pruning
                let lhs_for_disc = entry.lhs.clone();
                group.entries_for_mut(first_arg_head).push(entry);

                // Build disc tree when group has 4+ rules, or insert into existing
                let total_rules = group.len();
                if total_rules >= 4 {
                    if let Some(ref mut tree) = group.disc_tree {
                        tree.insert(idx, &lhs_for_disc);
                    } else {
                        // First time crossing threshold — build and backfill
                        let mut tree = crate::backend::eval::cesk::DiscriminationTree::new();
                        for existing in group.all_entries() {
                            tree.insert(existing.rule_index_in_group, &existing.lhs);
                        }
                        group.disc_tree = Some(tree);
                    }
                }
            }
            None => {
                // Check for duplicate in wildcard list
                for existing in self.wildcard.iter_mut() {
                    if existing.lhs == entry.lhs && existing.rhs == entry.rhs {
                        existing.multiplicity += 1;
                        return;
                    }
                }
                let mut entry = entry;
                entry.rule_index_in_group = self.wildcard.len() as u32;
                entry.global_rule_index = GLOBAL_RULE_COUNTER.fetch_add(1, Ordering::Relaxed);
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
    /// variable/wildcard first arguments. This reduces match attempts by 3-10x
    /// for PLN workloads.
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
// Structural Matcher — Direct MettaValue pattern matching
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
    /// Check that the node at `path` is Unit (empty tuple `()`).
    IsUnit {
        path: MatchPath,
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
/// Eliminates serialization overhead by performing structural comparison
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
/// - Speedup: ~4-6x per candidate match vs serialization-based approaches
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
    /// These unsupported patterns fall back to `pattern_match_generic`.
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
                return false; // Depth overflow — bail to pattern_match_generic
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

        // Unit — must match Unit exactly (empty tuple `()`)
        // GcFactory::sexpr(vec![]) converts to Unit, so () in patterns is Unit.
        if value.is_unit() || value.is_empty() {
            checks.push(StructuralCheck::IsUnit { path });
            return true;
        }

        // Unsupported node type (Type, Conjunction, Error, Quoted, etc.)
        // Fall back to pattern_match_generic
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
                StructuralCheck::IsUnit { path } => {
                    let val = path.navigate(expr)?;
                    if !val.is_unit() && !val.is_empty() {
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
                StructuralCheck::IsUnit { path } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    if !val.is_unit() && !val.is_empty() {
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

/// Build a list of original variable names from the LHS pattern,
/// identifying wildcard indices (anonymous variables from `_`).
///
/// Returns `(var_names, wildcard_indices)` where:
/// - `var_names[i]` is the full variable name including `$` prefix
/// - `wildcard_indices` contains indices of anonymous wildcard variables
fn build_var_names_from_lhs<V: MettaValueTrait>(
    lhs: &V,
) -> (Vec<&'static str>, SmallVec<[u8; 4]>) {
    use crate::backend::models::gc_allocator::global_allocator;

    let alloc = global_allocator();
    let mut var_names = Vec::new();
    let mut wildcard_indices = SmallVec::new();

    collect_vars_recursive(lhs, &mut var_names, &mut wildcard_indices, alloc);

    (var_names, wildcard_indices)
}

/// Recursively collect variable names from a pattern expression.
fn collect_vars_recursive<V: MettaValueTrait>(
    value: &V,
    var_names: &mut Vec<&'static str>,
    wildcard_indices: &mut SmallVec<[u8; 4]>,
    alloc: &crate::backend::models::gc_allocator::SlabAllocator,
) {
    if let Some(items) = value.as_sexpr() {
        for child in items {
            collect_vars_recursive(child, var_names, wildcard_indices, alloc);
        }
        return;
    }

    if let Some(atom) = value.as_atom() {
        if atom == "_" {
            wildcard_indices.push(var_names.len() as u8);
            var_names.push("_");
        } else if atom.len() > 1
            && (atom.starts_with('$')
                || atom.starts_with('\'')
                || (atom.starts_with('&')
                    && atom != "&"
                    && atom != "&self"
                    && atom != "&kb"
                    && atom != "&stack"))
        {
            // Check for duplicate variable (repeated occurrence)
            if !var_names.contains(&atom) {
                let interned = alloc.alloc_str(atom);
                var_names.push(interned);
            }
        }
    }
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
    /// The rule is stored as `(= lhs rhs)` in the MettaTrie using literal decomposition
    /// (via `decompose_literal`). The RuleIndex is populated with cached MettaValues
    /// and metadata for O(1) lookup + structural matching.
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

        // Decompose and insert into MettaTrie
        let keys = crate::backend::decompose::decompose_literal(&rule_sexpr);
        {
            let mut btm = self.shared.atom_space.btm.write();
            multiplicity::trie_add_atom(&mut btm, &keys, rule_sexpr.clone());
        }
        self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

        // Compute metadata from LHS pattern
        let (var_names, wildcard_indices) = build_var_names_from_lhs(&lhs);

        // Populate RuleIndex with second-level first-arg indexing
        let alloc = crate::backend::models::gc_allocator::global_allocator();
        let first_arg_head_interned: Option<&'static str> =
            get_first_arg_head(&lhs).map(|s| alloc.alloc_str(s));
        let structural_matcher = StructuralMatcher::analyze(&lhs);
        // I-2: If StructuralMatcher fails (e.g., depth > 8), try EnhancedMatcher
        let enhanced_matcher = if structural_matcher.is_none() {
            crate::backend::eval::cesk::EnhancedMatcher::analyze(&lhs)
        } else {
            None
        };
        let entry = RuleEntry {
            lhs: lhs.clone(),
            rhs_has_variables: rhs.contains_variables(),
            rhs: rhs.clone(),
            lhs_debruijn: vec![], // No longer used — structural/enhanced matchers handle matching
            var_names,
            wildcard_indices,
            multiplicity: 1,
            rhs_type: rhs_type.clone(),
            structural_matcher,
            enhanced_matcher,
            rule_index_in_group: 0, // Assigned by RuleIndex::add_rule
            global_rule_index: 0, // Assigned by RuleIndex::add_rule
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

        // Update bloom filter with (head, arity) for O(1) match_space() rejection
        if let Some(ref head) = head_owned {
            let arity_u8 = arity as u8;
            self.shared
                .atom_space.head_arity_bloom
                .write()
                .insert(head, arity_u8);
        }

        // Also insert the rule s-expression's head/arity ("=", arity=2) so that
        // match_space() queries for (= $a $b) patterns aren't rejected by
        // the bloom filter. get_arity() returns items.len()-1 (excludes head),
        // so (= lhs rhs) has arity 2. Without this, direct add_rule() callers
        // (= special form eval, module loading) miss the bloom filter entry.
        self.shared
            .atom_space.head_arity_bloom
            .write()
            .insert("=", 2);

        self.modified.store(true, Ordering::Release);
    }

    /// Match rules natively using structural matchers for pattern matching.
    ///
    /// Pipeline:
    /// 1. **Bloom filter** — O(1) rejection by (head, arity)
    /// 2. **RuleIndex lookup** — O(1) HashMap lookup for `(head, arity) → Vec<RuleEntry>`
    /// 3. **StructuralMatcher / EnhancedMatcher** — direct MettaValue pattern matching per candidate
    /// 4. **Extract bindings** — Only for successful matches (all matching rules fire nondeterministically)
    /// 5. **`apply_bindings_generic()`** — Apply bindings to cached RHS MettaValue
    ///
    /// ## Fallback
    ///
    /// For rules where neither StructuralMatcher nor EnhancedMatcher could be built,
    /// falls back to `pattern_match_generic()` for direct MettaValue-level matching.
    pub fn match_rules_native(
        &self,
        expr: &V,
        apply_bindings: impl Fn(&V, &GenericBindings<V>, &F) -> V,
    ) -> Vec<RuleMatchResult<V>> {
        let head = expr.get_head_symbol().unwrap_or("");
        let arity = expr.get_arity();
        // Phase 3: Extract first argument's head symbol for second-level index narrowing
        let first_arg_head = get_first_arg_head(expr);

        // Bloom filter O(1) rejection: skip entirely when
        // the bloom filter says no head-specific rules exist for this head+arity
        // AND there are no wildcard rules (which match any head).
        if !head.is_empty() {
            let bloom_says_no = !self
                .shared
                .atom_space.head_arity_bloom
                .read()
                .may_contain(head, arity as u8);
            if bloom_says_no && !self.shared.rule_index.read().has_wildcard_rules() {
                return Vec::new();
            }
        }

        let rule_index = self.shared.rule_index.read();

        // Collect candidates into a SmallVec (stack-allocated for ≤16 entries).
        let candidates: SmallVec<[&RuleEntry<V>; 16]> = if !head.is_empty() {
            rule_index.get_candidates(head, arity, first_arg_head).collect()
        } else {
            rule_index.get_all_rules().collect()
        };

        if candidates.is_empty() {
            return Vec::new();
        }

        // I-12: Filter out dead rules via CompressedRuleFilter (from AAM analysis).
        let candidates: SmallVec<[&RuleEntry<V>; 16]> = candidates
            .into_iter()
            .filter(|e| crate::backend::eval::cesk::continuation_compression::is_rule_live(e.global_rule_index))
            .collect();

        if candidates.is_empty() {
            return Vec::new();
        }

        // I-14: Record query pattern for adaptive indexing.
        // I-13: Record consulted (head, arity) group for incremental invalidation.
        // Both are gated behind ANALYSIS_ACTIVE — a single branch-predicted
        // Relaxed load when analysis is not enabled (the common case).
        if is_analysis_active() && !head.is_empty() {
            // I-14: Tracks which argument positions are queried most frequently,
            // enabling runtime rebalancing of the second-level index.
            let head_hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                head.hash(&mut h);
                h.finish()
            };
            let arg_heads: SmallVec<[Option<&str>; 4]> = if let Some(items) = expr.as_sexpr() {
                items.iter().skip(1).map(|item| item.get_head_symbol()).collect()
            } else {
                SmallVec::new()
            };
            crate::backend::eval::cesk::with_adaptive_registry(|r| {
                r.record_query(head_hash, arity, &arg_heads);
            });

            // I-13: On space mutation, only subgoals that consulted the affected group
            // are selectively invalidated (instead of blanket invalidation).
            let expr_hash = expr.hash_value();
            let head_static: &'static str = crate::backend::models::gc_allocator::global_allocator().alloc_str(head);
            crate::backend::eval::cesk::with_incremental_index(|idx| {
                idx.record_dependency(crate::backend::eval::cesk::rete_incremental::SubgoalDependency {
                    subgoal_hash: expr_hash,
                    consulted_groups: smallvec::smallvec![(head_static, arity)],
                    matched_rules: smallvec::SmallVec::new(), // Populated after matching
                    transitive_deps: smallvec::SmallVec::new(),
                });
            });
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

        // I-10: Parallel speculative matching for large candidate sets.
        // Skip when head is empty (all-rules fallback) — these queries rarely match
        // and the 102+ candidates are all false positives for thread spawning.
        if all_structural
            && !head.is_empty()
            && crate::backend::eval::cesk::should_speculate(candidates.len())
            && std::any::TypeId::of::<V>() == std::any::TypeId::of::<crate::backend::models::MettaValue>()
        {
            // I-10: Parallel speculative matching — head matching is pure read-only.
            // Only for MettaValue (GcFactory is Send+Sync).
            // Cap speculative threads to prevent contention with eval work pool.
            // Use at most half the available parallelism, capped at 8.
            const MAX_SPECULATIVE_THREADS: usize = 8;
            let chunks = crate::backend::eval::cesk::chunk_candidates(
                candidates.len(),
                std::thread::available_parallelism()
                    .map(|n| n.get() / 2)
                    .unwrap_or(4)
                    .min(MAX_SPECULATIVE_THREADS)
                    .min(candidates.len())
                    .max(2),
            );

            // SAFETY: V is MettaValue (TypeId checked above). GcFactory is Send+Sync.
            let factory_ptr = &self.factory as *const F as *const crate::backend::models::GcFactory;
            let gc_factory: crate::backend::models::GcFactory = unsafe { *factory_ptr };
            let mut all_results: Vec<RuleMatchResult<V>> = Vec::new();

            std::thread::scope(|s| {
                let handles: Vec<_> = chunks.iter().map(|&(start, end)| {
                    let chunk = &candidates[start..end];
                    let fac = gc_factory;
                    s.spawn(move || {
                        let mut chunk_results = Vec::new();
                        for entry in chunk {
                            let bindings = if let Some(ref m) = entry.structural_matcher {
                                m.try_match(expr)
                            } else if let Some(ref m) = entry.enhanced_matcher {
                                m.try_match(expr)
                            } else {
                                crate::backend::eval::trampoline::pattern_match_generic(&entry.lhs, expr)
                            };
                            if let Some(bindings) = bindings {
                                let instantiated_rhs = if entry.rhs_has_variables {
                                    // SAFETY: V is MettaValue, fac is GcFactory (TypeId checked).
                                    let v_ref: &crate::backend::models::MettaValue =
                                        unsafe { &*(&entry.rhs as *const V as *const crate::backend::models::MettaValue) };
                                    let b_ref: &crate::backend::models::GenericBindings<crate::backend::models::MettaValue> =
                                        unsafe { &*(&bindings as *const _ as *const crate::backend::models::GenericBindings<crate::backend::models::MettaValue>) };
                                    let result = crate::backend::eval::trampoline::apply_bindings_generic(
                                        v_ref, b_ref, &fac,
                                    );
                                    // SAFETY: MettaValue and V are the same type
                                    unsafe { std::mem::transmute_copy::<crate::backend::models::MettaValue, V>(&result) }
                                } else {
                                    entry.rhs.clone()
                                };
                                let multiplicity = entry.multiplicity.max(1);
                                for _ in 0..multiplicity {
                                    chunk_results.push(RuleMatchResult {
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
                        chunk_results
                    })
                }).collect();

                for handle in handles {
                    all_results.extend(handle.join().expect("speculative match thread panicked"));
                }
            });

            return all_results;
        }

        // Sequential path: structural matchers + fallback to pattern_match_generic
        let mut results: Vec<RuleMatchResult<V>> = Vec::new();

        // I-3: Use binding arena for O(1) rollback on failed matches
        let mut arena = crate::backend::eval::cesk::BindingArena::<V>::new();
        arena.push_frame();

        for entry in &candidates {
            // I-3: Save choice point before each attempt
            arena.save_choice_point();

            // Try structural matcher first, then enhanced matcher, then generic fallback
            let bindings = if let Some(ref matcher) = entry.structural_matcher {
                matcher.try_match(expr)
            } else if let Some(ref matcher) = entry.enhanced_matcher {
                matcher.try_match(expr)
            } else {
                // Fallback: pattern_match_generic for entries without compiled matchers
                crate::backend::eval::trampoline::pattern_match_generic(&entry.lhs, expr)
            };

            if let Some(bindings) = bindings {
                // Match succeeded — commit choice point
                arena.commit_choice_point();
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
            } else {
                // I-3: Match failed — restore choice point (O(1) rollback)
                arena.restore_choice_point();
            }
        }

        results
    }

    /// Get matching rules for an expression from MettaTrie iteration.
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
                .may_contain(head, arity as u8);
            if bloom_says_no && !self.shared.rule_index.read().has_wildcard_rules() {
                return Vec::new();
            }
        }

        let btm = self.shared.atom_space.btm.read();
        let mut rules: Vec<(V, V, u64)> = Vec::new();

        for (stored_expr, mult) in btm.iter() {
            if let Some((lhs, rhs)) = extract_rule_parts(stored_expr) {
                // Filter by head+arity if the query has head info
                if !head.is_empty() {
                    let rule_head = lhs.get_head_symbol().unwrap_or("");
                    let rule_arity = lhs.get_arity();
                    // Include if head+arity matches, or if rule is a wildcard (no head)
                    if rule_head.is_empty() || (rule_head == head && rule_arity == arity) {
                        rules.push((lhs, rhs, mult.count().max(1)));
                    }
                } else {
                    // No head info — collect all rules
                    rules.push((lhs, rhs, mult.count().max(1)));
                }
            }
        }

        rules
    }
}

// ============================================================================
// MettaEnvironment-specific Rule Operations
// ============================================================================

impl MettaEnvironment {
    /// Get the number of rules in the environment.
    pub fn rule_count(&self) -> usize {
        let btm = self.shared.atom_space.btm.read();
        let mut count = 0;

        for (expr, _mult) in btm.iter() {
            if Self::is_rule_sexpr_generic(&expr) {
                count += 1;
            }
        }

        count
    }

    /// Iterator over rule heads with their arities and counts.
    pub fn iter_rule_heads(&self) -> RuleHeadsIter {
        let btm = self.shared.atom_space.btm.read();
        let mut head_map: HashMap<(String, usize), usize> = HashMap::new();

        for (expr, _mult) in btm.iter() {
            if let Some((lhs, _rhs)) = extract_rule_parts(expr) {
                let head = lhs.get_head_symbol().unwrap_or("").to_string();
                let arity = lhs.get_arity();
                *head_map.entry((head, arity)).or_insert(0) += 1;
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
        let btm = self.shared.atom_space.btm.read();
        let mut rules = Vec::new();

        for (expr, _mult) in btm.iter() {
            if let Some((lhs, rhs)) = extract_rule_parts(expr) {
                rules.push((lhs, rhs));
            }
        }

        rules
    }

    /// Bulk add rules using MettaTrie + RuleIndex population.
    ///
    /// Each rule is inserted individually into MettaTrie + RuleIndex.
    /// This is consistent with `add_rule()`.
    ///
    /// # Arguments
    /// * `rules` - Vec of (lhs, rhs) pairs
    pub fn add_rules_bulk(&mut self, rules: Vec<(MettaValue, MettaValue)>) -> Result<(), String> {
        trace!(target: "mettatron::environment::add_rules_bulk", rule_count = rules.len());
        if rules.is_empty() {
            return Ok(());
        }

        self.make_owned();

        // Delegate to add_rule() for each — ensures consistent encoding + RuleIndex
        for (lhs, rhs) in rules {
            self.add_rule(lhs, rhs);
        }

        self.modified.store(true, Ordering::Release);
        Ok(())
    }

    /// Get the number of times a rule has been defined (multiplicity).
    ///
    /// Uses literal decomposition to match the MettaTrie entry.
    pub fn get_rule_count(&self, lhs: &MettaValue, rhs: &MettaValue) -> usize {
        let rule_sexpr = self.factory.sexpr(vec![
            self.factory.atom("="),
            lhs.clone(),
            rhs.clone(),
        ]);

        let keys = crate::backend::decompose::decompose_literal(&rule_sexpr);
        let btm = self.shared.atom_space.btm.read();
        let count = multiplicity::trie_get_multiplicity(&btm, &keys);
        if count == 0 { 1 } else { count as usize }
    }

    /// Get the multiplicities (for serialization).
    /// The keys are the expression's display format for serialization stability.
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        let btm = self.shared.atom_space.btm.read();
        let mut result = HashMap::new();

        for (expr, mult) in btm.iter() {
            let count = mult.count() as usize;
            let count = if count == 0 { 1 } else { count };
            let display_key = format!("{:?}", expr);
            result.insert(display_key, count);
        }

        result
    }

    /// Rebuild bloom filter, fuzzy matcher, and RuleIndex from MettaTrie.
    ///
    /// This is needed after deserializing an Environment from MettaTrie data,
    /// since the serialization only preserves the MettaTrie, not the bloom filter
    /// or RuleIndex.
    pub fn rebuild_bloom_filter(&mut self) {
        trace!(target: "mettatron::environment::rebuild_bloom_filter", "Rebuilding bloom filter, fuzzy matcher, and RuleIndex from MettaTrie");
        self.make_owned();

        // Clear the existing RuleIndex before rebuilding
        self.shared.rule_index.write().clear();

        let btm = self.shared.atom_space.btm.read();

        // Collect all rule entries from the trie
        let entries: Vec<(MettaValue, metta_trie::Multiplicity)> = btm.iter()
            .map(|(expr, mult)| (expr.clone(), mult.clone()))
            .collect();

        // Drop the btm read lock before processing (we may need write access later)
        drop(btm);

        for (value, multiplicity_val) in entries {
            if let Some((lhs, rhs)) = extract_rule_parts(&value) {
                // Update bloom filter + fuzzy matcher
                let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
                let arity = lhs.get_arity();
                if let Some(ref head) = head_owned {
                    self.shared.fuzzy_matcher.write().insert(head);
                    self.shared
                        .atom_space.head_arity_bloom
                        .write()
                        .insert(head, arity as u8);
                }

                // Rebuild RuleIndex entry
                let multiplicity = multiplicity_val.count().max(1);

                let (var_names, wildcard_indices) = build_var_names_from_lhs(&lhs);

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
                let enhanced_matcher = if structural_matcher.is_none() {
                    crate::backend::eval::cesk::EnhancedMatcher::analyze(&lhs)
                } else {
                    None
                };
                let entry = RuleEntry {
                    lhs: lhs.clone(),
                    rhs_has_variables: rhs.contains_variables(),
                    rhs: rhs.clone(),
                    lhs_debruijn: vec![], // No longer used
                    var_names,
                    wildcard_indices,
                    multiplicity,
                    rhs_type,
                    structural_matcher,
                    enhanced_matcher,
                    rule_index_in_group: 0,
                    global_rule_index: 0, // Assigned by RuleIndex::add_rule
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
            }
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Increment the multiplicity count for a rule.
    ///
    /// Uses literal decomposition to match the MettaTrie entry.
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

        let keys = crate::backend::decompose::decompose_literal(rule_sexpr);
        let new_count = {
            let mut btm = self.shared.atom_space.btm.write();
            multiplicity::trie_increment_multiplicity(&mut btm, &keys, rule_sexpr.clone())
        };

        self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
        self.modified.store(true, Ordering::Release);

        // Sync RuleIndex: increment the matching entry's multiplicity
        if let Some((lhs, rhs)) = extract_rule_parts(rule_sexpr) {
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

        new_count as usize
    }

    /// Decrement the multiplicity count for a rule.
    ///
    /// Uses literal decomposition to match the MettaTrie entry.
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

        let keys = crate::backend::decompose::decompose_literal(rule_sexpr);
        let old_count = {
            let btm = self.shared.atom_space.btm.read();
            multiplicity::trie_get_multiplicity(&btm, &keys)
        };

        if old_count == 0 {
            self.modified.store(true, Ordering::Release);
            return 0;
        }

        let new_count = {
            let mut btm = self.shared.atom_space.btm.write();
            multiplicity::trie_decrement_multiplicity(&mut btm, &keys)
        };

        self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
        self.modified.store(true, Ordering::Release);

        // Sync RuleIndex: decrement (or remove if multiplicity reaches 0)
        if let Some((lhs, rhs)) = extract_rule_parts(rule_sexpr) {
            self.shared.rule_index.write().remove_rule(&lhs, &rhs);
        }

        new_count as usize
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

    /// Check if a value is a rule s-expression using the generic trait API.
    fn is_rule_sexpr_generic(value: &MettaValue) -> bool {
        if let Some(items) = value.as_sexpr() {
            if items.len() == 3 {
                if let Some(op) = items[0].as_atom() {
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

        let keys = crate::backend::decompose::decompose_literal(value);
        let mut btm = self.shared.atom_space.btm.write();
        multiplicity::trie_increment_multiplicity(&mut btm, &keys, value.clone());
        let new_count = multiplicity::trie_get_multiplicity(&btm, &keys);
        drop(btm);

        self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
        self.modified.store(true, Ordering::Release);
        new_count as usize
    }

    /// Decrement multiplicity for ANY atom.
    pub fn decrement_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned();

        let keys = crate::backend::decompose::decompose_literal(value);
        let old_count = {
            let btm = self.shared.atom_space.btm.read();
            multiplicity::trie_get_multiplicity(&btm, &keys)
        };

        if old_count == 0 {
            return 0;
        }

        let mut btm = self.shared.atom_space.btm.write();
        let new_count = multiplicity::trie_decrement_multiplicity(&mut btm, &keys);
        drop(btm);

        self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
        self.modified.store(true, Ordering::Release);
        new_count as usize
    }

    /// Get multiplicity for ANY atom.
    pub fn get_atom_multiplicity(&self, value: &MettaValue) -> usize {
        let keys = crate::backend::decompose::decompose_literal(value);
        let btm = self.shared.atom_space.btm.read();
        let count = multiplicity::trie_get_multiplicity(&btm, &keys);
        if count == 0 { 1 } else { count as usize }
    }
}
