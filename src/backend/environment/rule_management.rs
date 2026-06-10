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
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
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

use lru::LruCache;

/// I-12: Global monotonic counter for assigning unique global rule indices.
/// Used by CompressedRuleFilter to identify dead rules across all groups.
static GLOBAL_RULE_COUNTER: AtomicU32 = AtomicU32::new(0);

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

use super::core::GenericEnvironment;
use super::mork_encoding::{mork_bytes_to_generic_value, mork_expr_byte_len};

// Disabled: mork_expr_to_generic_value no longer used directly — deserialization happens via
// mork_bytes_to_generic_value for individual binding bytes.
// use super::mork_encoding::mork_expr_to_generic_value;
use super::multiplicity::{
    decrement_multiplicity, get_multiplicity, increment_multiplicity, Multiplicity,
};
use super::{MettaEnvironment, MettaValue};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait, ValueView};
use crate::backend::mork_convert::{with_mork_bytes, with_mork_query_bytes};

/// Phase 2.x PT cons-pattern rewrite (2026-05-22): walk an LHS pattern
/// and rewrite `(cons HEAD TAIL)` sub-SExprs into `(HEAD . TAIL)` dotted-pair
/// form, BUT only when HEAD is a literal atom (not a variable). This is
/// PLN's idiom for destructuring `(<literal-head> <args>...)` SExprs:
///   `(cons , $args)` → `(, . $args)` matches `(, A B)` binding $args = (A B).
///
/// When HEAD is a variable (e.g. `(cons $x $xs)`), this is the user's
/// own data shape with `cons` as a literal head atom; the structural
/// element-wise match is the correct semantics, so the pattern is left
/// unchanged.
///
/// Idempotent: applying twice produces the same result. Recurses through
/// all SExpr children.
pub(crate) fn rewrite_cons_to_dotted_pair<V, F>(value: V, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: crate::backend::models::MettaValueFactory<V>,
{
    if let Some(items) = value.as_sexpr() {
        // First recurse into children so nested cons-patterns get rewritten.
        let rewritten_children: Vec<V> = items
            .iter()
            .map(|c| rewrite_cons_to_dotted_pair(c.clone(), factory))
            .collect();
        // Then check this SExpr itself for the cons-pattern shape.
        if rewritten_children.len() == 3 && rewritten_children[0].as_atom() == Some("cons") {
            // Only rewrite if HEAD is a literal atom (not a variable like $x).
            // PT's idiom is `(cons <literal> $args)` for destructure; user
            // code's `(cons $x $xs)` is structural (cons literal head + args).
            let head_atom = rewritten_children[1].as_atom();
            let head_is_literal = head_atom
                .map(|n| !n.starts_with('$') && !n.starts_with('\'') && n != "_" && n != "&")
                .unwrap_or(false);
            if head_is_literal {
                // Rewrite `(cons LITERAL TAIL)` → `(LITERAL . TAIL)`.
                return factory.sexpr(vec![
                    rewritten_children[1].clone(),
                    factory.atom("."),
                    rewritten_children[2].clone(),
                ]);
            }
        }
        return factory.sexpr(rewritten_children);
    }
    value
}

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
    /// Caller-visible bindings ($query_var → value) that may escape as branch
    /// provenance.
    ///
    /// Rule-local scratch keys (`$__fr_<epoch>_*` in `rule_scope`) are valid
    /// only while instantiating the RHS and must not be stored here. **The
    /// bytecode VM tier MUST NOT use this field** — its `compiled_rhs` opcodes
    /// reference original rule variable names; use `original_bindings` instead.
    pub bindings: GenericBindings<V>,
    /// Pre-freshen, pre-scope-tag named bindings keyed on the rule's
    /// ORIGINAL LHS variable names (e.g. `$x`, `$y`).
    ///
    /// Populated for bytecode-VM `op_dispatch_rules` frame setup, whose
    /// `compiled_rhs` was built at rule-insertion time against original
    /// names. Always `ROOT_SCOPE`-keyed (the freshen+retag transform that
    /// produces `bindings` is skipped for this field). For ground rules
    /// (`!rhs_has_variables`), this is `Empty`.
    pub original_bindings: GenericBindings<V>,
    /// Per-dispatch scope ID assigned at this match site. Set to a fresh
    /// `allocate_scope_id()` value for every successful match; defaults to
    /// `ROOT_SCOPE` only for ground RHS / no-variable rules where
    /// retagging is a no-op.
    pub rule_scope: crate::backend::models::generic_bindings::ScopeId,
    /// How many times this rule was defined (multiplicity)
    pub multiplicity: u64,
    /// Phase 8.7: Cached return type of the RHS (from RuleEntry).
    /// Used for branch pruning when `expected_type` is set.
    pub rhs_type: Option<V>,
    /// Whether the RHS template contains variables, computed once at rule insertion time.
    /// When `false`, `apply_bindings_generic` can be skipped entirely (O(1) clone).
    pub rhs_has_variables: bool,
    /// Pre-compiled bytecode for the RHS body, if available.
    /// Type-erased; downcasted in op_dispatch_rules.
    pub compiled_rhs: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    /// MTT SUPERSET (2026-05-17): LHS specificity score copied from
    /// `RuleEntry`. Used by the post-match filter when
    /// `env.get_rule_fire_mode() == RuleFireMode::Specificity`. Zero overhead
    /// otherwise — the filter is a single `if` on the pragma.
    pub entry_specificity: u32,
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

    /// Full `(= lhs rhs)` De Bruijn bytes — used by `remove_rule` for
    /// alpha-equivalent rule identification. Without this, remove-atom on
    /// a rule with variables fails silently: rules are stored with
    /// Fix 3B's alpha-renamed variables, so structural `MettaValue == V`
    /// comparison between the caller's original-name rule and the stored
    /// freshened rule never matches. Comparing De Bruijn bytes (which are
    /// alpha-equivalent by construction) fixes that.
    /// Empty for wide rules (arity ≥ 64).
    pub full_debruijn: Vec<u8>,

    // --- Metadata ---
    /// De Bruijn index → original variable name (e.g., "$x", "$y")
    /// Only contains variables from LHS (used for building named bindings).
    /// Interned as `&'static str` via slab allocator to avoid per-match String clones.
    pub var_names: Vec<&'static str>,
    /// Indices of `_` wildcards (skip these in named bindings)
    pub wildcard_indices: SmallVec<[u8; 4]>,
    /// MTT SUPERSET specificity score (2026-05-17 restoration).
    ///
    /// Higher = more structurally constrained. Computed once at insertion
    /// via `lhs_specificity`. Consulted by `match_rules_native` ONLY when
    /// `env.get_rule_fire_mode() == RuleFireMode::Specificity` (opt-in
    /// pragma `(pragma! rule-fire-mode specificity)`).
    ///
    /// HE-bisim default (`Nondet`) ignores this field — all matching rules
    /// fire nondeterministically. Zero overhead on the default path.
    ///
    /// Correct metric: weighted constructor-depth + repeat-var penalty
    /// (replaces the removed broken NewVar-count metric — see
    /// `lhs_specificity` docs for the PLN regression case).
    pub specificity: u32,
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
    /// Enhanced matcher for deep patterns (depth > 8) that StructuralMatcher rejects (I-2).
    /// Provides indexed binding slots and no depth limit.
    pub enhanced_matcher: Option<crate::backend::eval::cesk::EnhancedMatcher>,
    /// Monotonic index within the RuleGroup (for discrimination tree indexing, I-1).
    pub rule_index_in_group: u32,
    /// I-12: Global monotonic rule index (for CompressedRuleFilter dead-rule filtering).
    pub global_rule_index: u32,
    /// Pre-compiled bytecode for the RHS body (compile-on-add).
    /// Type-erased to avoid propagating Send+Sync+'static bounds through RuleEntry<V>.
    pub compiled_rhs: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    /// Whether the RHS transitively contains monadic effect operations
    /// (IO, StateMonad, or other effect types). Computed at rule insertion
    /// time by checking the inferred RHS type. Functions with monadic
    /// effects must not be memoized — repeated calls must re-execute.
    pub has_monadic_effect: bool,
    /// **H8 (2026-05-05)**: Whether this rule's RHS calls `(decons-atom ...)` on
    /// a value that's the LHS first-arg variable. Such rules are structurally
    /// guaranteed to produce empty results when the caller's first arg is `()`.
    /// Set at rule insertion time. Filtered at `get_candidates_filtered` to
    /// skip these rules when the caller's first arg is empty sexpr —
    /// projected 78.7% wall savings on mmverify per Audit #7a.
    pub requires_non_empty_first_arg: bool,
    /// PT-canonical rule-body preservation gate. True iff the RHS top-level
    /// head is in `is_lazy_body_form` (e.g. `add-atom`, `quote`, `if`,
    /// `case`, `let`, `chain`, `match`, `=`, ...). When set, the dispatcher
    /// skips Step-2 arg pre-evaluation for calls to this rule's LHS-head so
    /// the rule body sees its template args verbatim (PeTTa `findall`
    /// substitution semantics).
    ///
    /// PLN benchmarks rely on this: `(=> $A $C $stv) → (add-atom &self
    /// (= $C (Truth_MP $A $stv)))` requires `$A=(father $a $b)` to remain
    /// unreduced so add-atom registers a variable-preserving rule.
    pub body_wants_lazy_args: bool,
    /// PT-canonical meta-typed signature gate. True iff the rule's LHS head
    /// has at least one declared arrow type where ALL arg types AND the
    /// return type are meta-types (`Atom`, `Expression`, `Symbol`,
    /// `Variable`, `Grounded`, `Pattern`, `%Undefined%`).
    ///
    /// When set, the rule-firing dispatcher returns the substituted RHS
    /// VERBATIM (no re-evaluation), matching PeTTa's semantic that a
    /// `(-> Expression Atom)`-typed predicate is data-in / data-out.
    ///
    /// PLN's `(: ? (-> Expression Atom))` declares the canonical
    /// "preserve the term" predicate that drives Direct.metta's inference.
    pub lhs_head_all_meta_typed: bool,
    /// Phase 1 cut-barrier (control substrate): true iff the rule RHS
    /// lexically contains an applied `(cut ...)` head anywhere (recursively),
    /// not shadowed inside a `quote`. Computed once at `add_rule` time via
    /// `expr_contains_cut`. A rule whose body can fire `(cut)` must, when
    /// dispatched, open a fresh cut barrier so the cut prunes THIS clause's
    /// nondeterminism (the rule fan-out AND any match/superpose/let* fan-out
    /// produced while evaluating the body). See
    /// `docs/wam/control-substrate-design.md`.
    pub body_contains_cut: bool,
}

#[cfg(feature = "index-gc")]
fn shade_rule_entry_for_satb<V>(entry: &RuleEntry<V>)
where
    V: MettaValueTrait + Clone + 'static,
{
    let mut roots = Vec::new();
    for value in [&entry.lhs, &entry.rhs] {
        let any = value as &dyn std::any::Any;
        if let Some(root) = any.downcast_ref::<MettaValue>() {
            roots.push(root.clone());
        }
    }
    if let Some(rhs_type) = &entry.rhs_type {
        let any = rhs_type as &dyn std::any::Any;
        if let Some(root) = any.downcast_ref::<MettaValue>() {
            roots.push(root.clone());
        }
    }
    if let Some(compiled) = &entry.compiled_rhs {
        if let Some(chunk) =
            compiled.downcast_ref::<crate::backend::bytecode::chunk::BytecodeChunk>()
        {
            crate::backend::bytecode::cache::collect_chunk_constants(chunk, &mut roots);
        }
    }
    crate::backend::eval::cesk::index_heap::index_gc::satb_shade_evicted_roots(roots);
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
    /// Discrimination tree for multi-level candidate pruning (Phase 1.3 / I-1).
    /// Built when the group has 4+ rules. Prunes candidates before structural
    /// matching, reducing the number of try_match() invocations.
    disc_tree: Option<crate::backend::eval::cesk::DiscriminationTree>,
    /// Monotonic counter for assigning rule_index_in_group to new entries.
    next_rule_index: u32,
}

/// Outcome of a `remove_rule_by_debruijn` call on a `RuleGroup`.
///
/// Distinguishes the multiplicity-decrement case (entry still present) from
/// the full-removal case (entry deleted). Carries the removed RHS so
/// `RuleIndex` can depopulate the per-head RHS-atom bloom symmetrically
/// with `add_rule`. See `RuleGroup::remove_rule_by_debruijn` for details.
///
/// Phase 11.A follow-up (2026-05-18) — without this distinction the bloom
/// drifted to soft false-positives as rules were removed via debruijn
/// matching (still correct, just over-routing to the trampoline path).
#[derive(Debug)]
pub(crate) enum RemovalOutcome<V> {
    /// Multiplicity > 1; was decremented. Bloom must NOT be touched.
    Decremented,
    /// Multiplicity dropped to 0; entry deleted. Caller depopulates the
    /// bloom by passing `rhs` to `PerHeadAtomIndex::note_rule_removed`.
    Removed { rhs: V },
}

impl<V: MettaValueTrait + Clone + 'static> RuleGroup<V> {
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
                self.by_first_arg_head
                    .get(interned)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[])
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
        self.by_first_arg_head
            .values()
            .flat_map(|v| v.iter())
            .chain(self.variable_first_arg.iter())
    }

    /// Iterate over ALL rules mutably (for increment_multiplicity)
    fn all_entries_mut(&mut self) -> impl Iterator<Item = &mut RuleEntry<V>> {
        self.by_first_arg_head
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .chain(self.variable_first_arg.iter_mut())
    }

    /// Total number of rules in this group.
    fn len(&self) -> usize {
        self.by_first_arg_head
            .values()
            .map(|v| v.len())
            .sum::<usize>()
            + self.variable_first_arg.len()
    }

    /// Remove a rule by position. Returns true if entry was fully removed.
    fn remove_rule(&mut self, lhs: &V, rhs: &V, satb_active: bool) -> Option<bool> {
        #[cfg(not(feature = "index-gc"))]
        let _ = satb_active;

        // Search in first-arg-indexed buckets
        for entries in self.by_first_arg_head.values_mut() {
            if let Some(pos) = entries.iter().position(|e| &e.lhs == lhs && &e.rhs == rhs) {
                if entries[pos].multiplicity > 1 {
                    entries[pos].multiplicity -= 1;
                    return Some(false);
                } else {
                    #[cfg(feature = "index-gc")]
                    {
                        let removed = entries.remove(pos);
                        if satb_active {
                            shade_rule_entry_for_satb(&removed);
                        }
                    }
                    #[cfg(not(feature = "index-gc"))]
                    {
                        entries.remove(pos);
                    }
                    // Disc tree indexes by rule_index_in_group; removing an
                    // entry invalidates those indices, so the tree must be
                    // rebuilt on the next query.
                    self.invalidate_disc_tree();
                    return Some(true);
                }
            }
        }
        // Search in variable-first-arg list
        if let Some(pos) = self
            .variable_first_arg
            .iter()
            .position(|e| &e.lhs == lhs && &e.rhs == rhs)
        {
            if self.variable_first_arg[pos].multiplicity > 1 {
                self.variable_first_arg[pos].multiplicity -= 1;
                return Some(false);
            } else {
                #[cfg(feature = "index-gc")]
                {
                    let removed = self.variable_first_arg.remove(pos);
                    if satb_active {
                        shade_rule_entry_for_satb(&removed);
                    }
                }
                #[cfg(not(feature = "index-gc"))]
                {
                    self.variable_first_arg.remove(pos);
                }
                self.invalidate_disc_tree();
                return Some(true);
            }
        }
        None // Not found in this group
    }

    /// Alpha-equivalent rule removal via full De Bruijn byte comparison.
    ///
    /// Rules are stored with Fix 3B alpha-renamed variables, so
    /// `remove_rule(&lhs, &rhs)` using MettaValue structural equality fails
    /// when the caller passes original-name variables. This variant instead
    /// compares the full `(= lhs rhs)` De Bruijn bytes, which are alpha-
    /// equivalent by construction: two rules that differ only in variable
    /// names produce identical bytes.
    ///
    /// Returns `Some(RemovalOutcome::Removed { rhs })` when the entry was
    /// fully removed (multiplicity dropped to 0) — the RHS is returned so
    /// the caller can depopulate the per-head RHS-atom bloom symmetrically
    /// with `add_rule`. Returns `Some(RemovalOutcome::Decremented)` when
    /// only multiplicity was decremented (the rule's bloom entry must NOT
    /// be touched in that case). Returns `None` when no match was found.
    fn remove_rule_by_debruijn(
        &mut self,
        full_bytes: &[u8],
        satb_active: bool,
    ) -> Option<RemovalOutcome<V>> {
        #[cfg(not(feature = "index-gc"))]
        let _ = satb_active;

        // Search first-arg-indexed buckets
        for entries in self.by_first_arg_head.values_mut() {
            if let Some(pos) = entries.iter().position(|e| e.full_debruijn == full_bytes) {
                if entries[pos].multiplicity > 1 {
                    entries[pos].multiplicity -= 1;
                    return Some(RemovalOutcome::Decremented);
                } else {
                    let removed = entries.remove(pos);
                    let rhs = removed.rhs.clone();
                    #[cfg(feature = "index-gc")]
                    if satb_active {
                        shade_rule_entry_for_satb(&removed);
                    }
                    self.invalidate_disc_tree();
                    return Some(RemovalOutcome::Removed { rhs });
                }
            }
        }
        // Search variable-first-arg list
        if let Some(pos) = self
            .variable_first_arg
            .iter()
            .position(|e| e.full_debruijn == full_bytes)
        {
            if self.variable_first_arg[pos].multiplicity > 1 {
                self.variable_first_arg[pos].multiplicity -= 1;
                return Some(RemovalOutcome::Decremented);
            } else {
                let removed = self.variable_first_arg.remove(pos);
                let rhs = removed.rhs.clone();
                #[cfg(feature = "index-gc")]
                if satb_active {
                    shade_rule_entry_for_satb(&removed);
                }
                self.invalidate_disc_tree();
                return Some(RemovalOutcome::Removed { rhs });
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
/// Phase 11.A (2026-05-17) — per-head RHS-atom membership index.
///
/// Answers `rule_rhs_contains_atom(head, atom)` in O(1) via a
/// refcount-backed HashMap, replacing the legacy O(rules × rhs_size)
/// linear scan in `core.rs:rule_rhs_contains_atom`. The scan was being
/// invoked per eval-step × 11 needles × expression tree, contributing
/// roughly 230 K dereferences + 11 read-lock acquisitions per step on
/// a 140-rule environment — that overhead is the dominant regression
/// behind the PLN Robot.metta 60× slowdown.
///
/// **Refcount semantics.** The value tracks the NUMBER OF RULES (not
/// occurrences) whose `(head, atom)` pair is registered. Two distinct
/// rules with the same head, both referencing the same atom anywhere
/// in their RHS, push the refcount to 2; the first removal drops to 1
/// (the entry is still present, so membership remains positive); the
/// second drops to 0 and the key is removed.
///
/// **Multiplicity vs refcount.** Adding a rule that is already present
/// (only `RuleEntry.multiplicity` is incremented) MUST NOT touch the
/// bloom — the rule's RHS atoms are already registered. The hook is
/// at the new-entry path only. Removal symmetrically only depopulates
/// when an entry is actually deleted (multiplicity 1 → 0).
///
/// **Wildcards.** Rules with non-S-expression LHS (variable or atom
/// head, stored in `RuleIndex::wildcard`) carry no head symbol. The
/// legacy `rule_rhs_contains_atom` filtered them out via
/// `entry.lhs.get_head_symbol() == Some(head)`, so the bloom mirrors
/// that: wildcard rules do NOT contribute to the index. Callers asking
/// "any rule for HEAD with atom X" therefore see the same answer set.
///
/// **No false negatives.** The key is the byte-exact atom name. False
/// positives are impossible (the key is `String`-equality based via the
/// underlying `HashMap`). If false positives were possible, the gate
/// would over-route to the trampoline path — still correct but slower.
/// True membership is required for correctness of the dispatch gates
/// in `eval/mod.rs:895` (`expression_involves_impure_rules`).
#[derive(Debug, Clone, Default)]
pub(crate) struct PerHeadAtomIndex {
    /// `(interned_head, interned_atom) → refcount-of-rules`.
    counts: HashMap<(&'static str, &'static str), u32>,
}

impl PerHeadAtomIndex {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// O(1) membership: is there any registered rule for `head` whose
    /// RHS contains an atom named `atom`?
    #[inline]
    pub fn contains(&self, head: &str, atom: &str) -> bool {
        use crate::backend::models::gc_allocator::global_allocator;
        let h: &'static str = global_allocator().alloc_str(head);
        let a: &'static str = global_allocator().alloc_str(atom);
        self.counts.contains_key(&(h, a))
    }

    /// Bump refcount for each unique `atom` appearing anywhere in
    /// `rhs`. Called once per new rule entry in `RuleIndex::add_rule`.
    pub fn note_rule_added<V: MettaValueTrait>(&mut self, head: &'static str, rhs: &V) {
        let mut atoms: HashSet<&'static str> = HashSet::new();
        collect_static_atoms(rhs, &mut atoms);
        for atom in atoms {
            *self.counts.entry((head, atom)).or_insert(0) += 1;
        }
    }

    /// Decrement refcount for each unique `atom` appearing in `rhs`.
    /// Removes the key when refcount reaches zero. Called from
    /// `RuleIndex::remove_rule` ONLY when an entry is actually deleted
    /// (multiplicity 1 → 0); not when multiplicity simply decrements.
    pub fn note_rule_removed<V: MettaValueTrait>(&mut self, head: &'static str, rhs: &V) {
        let mut atoms: HashSet<&'static str> = HashSet::new();
        collect_static_atoms(rhs, &mut atoms);
        for atom in atoms {
            let key = (head, atom);
            if let Some(c) = self.counts.get_mut(&key) {
                *c -= 1;
                if *c == 0 {
                    self.counts.remove(&key);
                }
            }
        }
    }

    /// Test-only inspection: total tracked (head, atom) pairs.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.counts.len()
    }
}

/// Walk `value` recursively and collect every distinct atom name into
/// `out`. Atom names are stored as `&'static str` via the slab
/// allocator — `MettaValueTrait::as_atom` returns
/// `Option<&'static str>` (see `models/metta_value_trait.rs:132`), so
/// the collected names share the same interned storage as the rule
/// itself. No allocation beyond the `HashSet`.
///
/// Phase 11.A — bounded by RHS size, called once per `add_rule` /
/// `remove_rule`. The cost is amortized over the program's lifetime;
/// the eval-step hot path pays only the `HashMap::contains_key` query.
fn collect_static_atoms<V: MettaValueTrait>(value: &V, out: &mut HashSet<&'static str>) {
    if let Some(name) = value.as_atom() {
        out.insert(name);
        return;
    }
    if let Some(items) = value.as_sexpr() {
        for item in items {
            collect_static_atoms(item, out);
        }
    }
}

/// Phase 11.A (2026-05-17) — fast structural test: does `value` contain
/// any atom whose name appears in `keys`?
///
/// Returns `true` as soon as one match is found (early exit). Used by
/// the sequential structural-matcher path at `match_rules_native_inner`
/// to gate the per-binding transitive-resolution loop.
///
/// Bindings whose values have NO cross-key reference are independent of
/// each other; pre-resolution would be a no-op on them. The gate skips
/// the loop entirely in that case — the common case for PLN's
/// recursive-list helpers where each binding is a self-contained
/// ground value (e.g., `$tuple → ((Sentence ...) (Sentence ...) ...)`
/// with no other-binding-name atoms).
///
/// Bindings whose values DO contain a cross-key (e.g., bidirectional
/// unify's `$B → (Inheritance $1 ...)` together with `$1 → Anna`)
/// still take the full transitive-resolution path, preserving the
/// `petta_helpers::modus_ponens_repeated_var_bidirectional_unify`
/// semantics from commit `b359684`.
///
/// Cost: O(|value|) per call. Cheap relative to the avoided
/// `apply_bindings_generic` loop (O(|bindings| × |value|)).
fn value_contains_any_key<V: MettaValueTrait>(value: &V, keys: &[&str]) -> bool {
    if keys.is_empty() {
        return false;
    }
    if let Some(name) = value.as_atom() {
        return keys.iter().any(|k| *k == name);
    }
    if let Some(items) = value.as_sexpr() {
        return items.iter().any(|item| value_contains_any_key(item, keys));
    }
    false
}

#[derive(Debug, Clone)]
pub(crate) struct RuleIndex<V: MettaValueTrait + Clone + 'static> {
    /// Rules indexed by (head_symbol, arity) → RuleGroup (with second-level first-arg indexing).
    /// Head symbols are interned as `&'static str` via the slab allocator for zero-alloc lookups.
    by_head_arity: HashMap<(&'static str, usize), RuleGroup<V>>,

    /// Rules with non-S-expression LHS (atoms, variables like `$x`).
    /// Always included in query results since they can match any expression.
    wildcard: Vec<RuleEntry<V>>,

    /// Phase 11.A — per-head RHS-atom membership index.
    pub(crate) rule_rhs_atoms: PerHeadAtomIndex,

    /// Stage 3b / expr_contains_cut (2026-05-27): monotonic flag — `true` once ANY
    /// added rule's body contains `(cut)` (via `RuleEntry::body_contains_cut`). Lets the
    /// dispatcher skip the per-dispatch full-instantiated-RHS `expr_contains_cut` walk
    /// (~4% of FlyingRaven self-time) when no rule uses cut: the instantiated RHS can
    /// then only carry cut via a binding value (cut-as-data), so only the (small)
    /// binding values are scanned. Set in `add_rule` (the single insertion choke point,
    /// also hit by union/merge re-adds); propagated by `#[derive(Clone)]` (fork /
    /// make_owned / union-clone). Monotonic (never cleared on removal): a conservative
    /// `true` only costs the full scan, whereas a spurious `false` would MISS a cut, so
    /// monotonic is the safe direction.
    pub(crate) any_rule_has_cut: bool,
}

impl<V: MettaValueTrait + Clone> RuleIndex<V> {
    /// Create a new empty RuleIndex.
    pub fn new() -> Self {
        RuleIndex {
            by_head_arity: HashMap::new(),
            wildcard: Vec::new(),
            rule_rhs_atoms: PerHeadAtomIndex::new(),
            any_rule_has_cut: false,
        }
    }

    /// Whether any rule in this index has a `(cut)` in its body. See the field doc.
    #[inline]
    pub(crate) fn any_rule_has_cut(&self) -> bool {
        self.any_rule_has_cut
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
        // Stage 3b: maintain the global cut flag at the single insertion choke point
        // (also covers union/merge re-adds via `merged.add_rule(...)`). Monotonic —
        // see the `any_rule_has_cut` field doc.
        self.any_rule_has_cut |= entry.body_contains_cut;

        use crate::backend::models::gc_allocator::global_allocator;

        match head {
            Some(h) => {
                let interned: &'static str = global_allocator().alloc_str(h);
                let group = self
                    .by_head_arity
                    .entry((interned, arity))
                    .or_insert_with(RuleGroup::new);

                // Check for duplicate across ALL entries in the group
                for existing in group.all_entries_mut() {
                    if existing.lhs == entry.lhs && existing.rhs == entry.rhs {
                        existing.multiplicity += 1;
                        #[cfg(feature = "trace")]
                        crate::backend::trace::with_trace_collector_ref(|tc| {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                0,
                                crate::backend::trace::convert::trace_value_generic(&entry.lhs),
                                Vec::new(),
                                None,
                                trace_format::TraceEventKind::RuleIndexInsert {
                                    rule_lhs: crate::backend::trace::convert::trace_value_generic(
                                        &entry.lhs,
                                    ),
                                    head: Some(h.to_string()),
                                    arity: arity as u32,
                                    first_arg_head: first_arg_head.map(|s| s.to_string()),
                                    rule_index_in_group: existing.rule_index_in_group,
                                    global_rule_index: existing.global_rule_index,
                                    is_duplicate: true,
                                    source: "direct-definition".to_string(),
                                },
                            );
                        });
                        return;
                    }
                }

                // I-1: Assign monotonic rule index and insert into disc tree
                let mut entry = entry;
                let idx = group.next_rule_index;
                entry.rule_index_in_group = idx;
                entry.global_rule_index = GLOBAL_RULE_COUNTER.fetch_add(1, Ordering::Relaxed);
                group.next_rule_index += 1;

                // Phase 11.A — populate the per-head RHS-atom bloom for
                // this NEW entry (skipped on duplicate-multiplicity
                // increments above). Cost is bounded by RHS size, paid
                // once per add.
                self.rule_rhs_atoms.note_rule_added(interned, &entry.rhs);

                #[cfg(feature = "trace")]
                {
                    let global_idx = entry.global_rule_index;
                    crate::backend::trace::with_trace_collector_ref(|tc| {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            0,
                            crate::backend::trace::convert::trace_value_generic(&entry.lhs),
                            Vec::new(),
                            None,
                            trace_format::TraceEventKind::RuleIndexInsert {
                                rule_lhs: crate::backend::trace::convert::trace_value_generic(
                                    &entry.lhs,
                                ),
                                head: Some(h.to_string()),
                                arity: arity as u32,
                                first_arg_head: first_arg_head.map(|s| s.to_string()),
                                rule_index_in_group: idx,
                                global_rule_index: global_idx,
                                is_duplicate: false,
                                source: "direct-definition".to_string(),
                            },
                        );
                    });
                }

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
                        #[cfg(feature = "trace")]
                        crate::backend::trace::with_trace_collector_ref(|tc| {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                0,
                                crate::backend::trace::convert::trace_value_generic(&entry.lhs),
                                Vec::new(),
                                None,
                                trace_format::TraceEventKind::RuleIndexInsert {
                                    rule_lhs: crate::backend::trace::convert::trace_value_generic(
                                        &entry.lhs,
                                    ),
                                    head: None,
                                    arity: 0,
                                    first_arg_head: None,
                                    rule_index_in_group: existing.rule_index_in_group,
                                    global_rule_index: existing.global_rule_index,
                                    is_duplicate: true,
                                    source: "direct-definition".to_string(),
                                },
                            );
                        });
                        return;
                    }
                }
                let mut entry = entry;
                entry.rule_index_in_group = self.wildcard.len() as u32;
                entry.global_rule_index = GLOBAL_RULE_COUNTER.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "trace")]
                {
                    let idx = entry.rule_index_in_group;
                    let global_idx = entry.global_rule_index;
                    crate::backend::trace::with_trace_collector_ref(|tc| {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            0,
                            crate::backend::trace::convert::trace_value_generic(&entry.lhs),
                            Vec::new(),
                            None,
                            trace_format::TraceEventKind::RuleIndexInsert {
                                rule_lhs: crate::backend::trace::convert::trace_value_generic(
                                    &entry.lhs,
                                ),
                                head: None,
                                arity: 0,
                                first_arg_head: None,
                                rule_index_in_group: idx,
                                global_rule_index: global_idx,
                                is_duplicate: false,
                                source: "direct-definition".to_string(),
                            },
                        );
                    });
                }
                self.wildcard.push(entry);
            }
        }
    }

    /// Remove a rule by decrementing multiplicity. Returns true if the entry was removed entirely.
    pub fn remove_rule(&mut self, lhs: &V, rhs: &V) -> bool {
        #[cfg(feature = "index-gc")]
        {
            return crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
                |satb_active| self.remove_rule_inner(lhs, rhs, satb_active),
            );
        }
        #[cfg(not(feature = "index-gc"))]
        {
            self.remove_rule_inner(lhs, rhs, false)
        }
    }

    fn remove_rule_inner(&mut self, lhs: &V, rhs: &V, satb_active: bool) -> bool {
        // Phase 11.A — capture the head BEFORE removal so we can
        // depopulate the per-head RHS-atom bloom symmetrically with
        // `add_rule`. Wildcards have no head and don't contribute to
        // the bloom, so no capture is needed in the wildcard branch.
        let removed_head: Option<&'static str> = lhs.get_head_symbol().map(|h| {
            use crate::backend::models::gc_allocator::global_allocator;
            global_allocator().alloc_str(h)
        });

        // Search in all groups
        for group in self.by_head_arity.values_mut() {
            if let Some(removed) = group.remove_rule(lhs, rhs, satb_active) {
                if removed {
                    if let Some(h) = removed_head {
                        self.rule_rhs_atoms.note_rule_removed(h, rhs);
                    }
                }
                return removed;
            }
        }
        // Check wildcard
        if let Some(pos) = self
            .wildcard
            .iter()
            .position(|e| &e.lhs == lhs && &e.rhs == rhs)
        {
            if self.wildcard[pos].multiplicity > 1 {
                self.wildcard[pos].multiplicity -= 1;
                return false;
            } else {
                #[cfg(feature = "index-gc")]
                {
                    let removed = self.wildcard.remove(pos);
                    if satb_active {
                        shade_rule_entry_for_satb(&removed);
                    }
                }
                #[cfg(not(feature = "index-gc"))]
                {
                    self.wildcard.remove(pos);
                }
                return true;
            }
        }
        false
    }

    /// Alpha-equivalent rule removal via full De Bruijn byte comparison.
    ///
    /// Use this instead of `remove_rule` when the caller has only the
    /// original-name rule form (e.g. from `remove-atom &self (= lhs rhs)`).
    /// Rules stored via `add_rule` are alpha-renamed by Fix 3B, so MettaValue
    /// structural equality fails; this variant matches via the full rule
    /// De Bruijn bytes which are alpha-equivalent by construction.
    ///
    /// Three-state return value (mirrors `RuleGroup::remove_rule_by_debruijn`):
    ///
    /// - `Some(true)`  — the matching entry had multiplicity 1 and was
    ///                   removed entirely.
    /// - `Some(false)` — multiplicity was > 1 and was decremented; the
    ///                   entry remains in the index.
    /// - `None`        — no matching entry was found in any group or in
    ///                   the wildcard bucket.
    ///
    /// **The distinction between `Some(false)` and `None` is essential.**
    /// Earlier versions of this method returned a bare `bool` where `false`
    /// conflated the "decremented" and "not found" cases. The caller's
    /// structural-equality fallback would then fire on a just-decremented
    /// entry (treating `false` as "not found") and fully remove it,
    /// silently corrupting multiplicity > 1 ground rules. Always check
    /// `.is_some()` to decide whether the index was authoritatively
    /// updated; only run a fallback path on `None`.
    pub fn remove_rule_by_debruijn(&mut self, full_bytes: &[u8]) -> Option<bool> {
        #[cfg(feature = "index-gc")]
        {
            return crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
                |satb_active| self.remove_rule_by_debruijn_inner(full_bytes, satb_active),
            );
        }
        #[cfg(not(feature = "index-gc"))]
        {
            self.remove_rule_by_debruijn_inner(full_bytes, false)
        }
    }

    fn remove_rule_by_debruijn_inner(
        &mut self,
        full_bytes: &[u8],
        satb_active: bool,
    ) -> Option<bool> {
        // Phase 11.A follow-up (2026-05-18): symmetrically depopulate the
        // per-head RHS-atom bloom when an entry is fully removed (mirrors
        // the working path in `remove_rule` at the LHS-comparison route).
        // The head comes from the iteration key — `RuleGroup` doesn't know
        // its own head, so the bloom update lives at this layer.
        //
        // Iterate `iter_mut` to keep the (head, arity) key in scope; the
        // first matching group short-circuits, so cost is the same as the
        // previous `values_mut()` loop on the common path.
        for ((head, _arity), group) in self.by_head_arity.iter_mut() {
            if let Some(outcome) = group.remove_rule_by_debruijn(full_bytes, satb_active) {
                return match outcome {
                    RemovalOutcome::Removed { rhs } => {
                        self.rule_rhs_atoms.note_rule_removed(*head, &rhs);
                        Some(true)
                    }
                    RemovalOutcome::Decremented => Some(false),
                };
            }
        }
        // Search wildcard bucket — wildcards do NOT contribute to the
        // bloom (see `PerHeadAtomIndex` "Wildcards" doc), so no
        // `note_rule_removed` call is needed here.
        if let Some(pos) = self
            .wildcard
            .iter()
            .position(|e| e.full_debruijn == full_bytes)
        {
            if self.wildcard[pos].multiplicity > 1 {
                self.wildcard[pos].multiplicity -= 1;
                return Some(false);
            } else {
                #[cfg(feature = "index-gc")]
                {
                    let removed = self.wildcard.remove(pos);
                    if satb_active {
                        shade_rule_entry_for_satb(&removed);
                    }
                }
                #[cfg(not(feature = "index-gc"))]
                {
                    self.wildcard.remove(pos);
                }
                return Some(true);
            }
        }
        None
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
        let group_iter = self
            .by_head_arity
            .get(&(interned, arity))
            .map(|group| group.get_candidates(first_arg_head));

        // Chain: group candidates (if group exists) + wildcard rules
        GroupOrEmpty { inner: group_iter }.chain(self.wildcard.iter())
    }

    /// PT-canonical rule-body preservation: cold-cache fallback for the
    /// dispatcher's Step-2 pre-eval gate. Returns true iff ANY rule
    /// matching (head, arity) has `body_wants_lazy_args = true`. O(candidates).
    pub fn any_rule_wants_lazy_args(&self, head: &str, arity: usize) -> bool {
        use crate::backend::models::gc_allocator::global_allocator;
        let interned: &'static str = global_allocator().alloc_str(head);
        if let Some(group) = self.by_head_arity.get(&(interned, arity)) {
            if group.get_candidates(None).any(|e| e.body_wants_lazy_args) {
                return true;
            }
        }
        self.wildcard.iter().any(|e| e.body_wants_lazy_args)
    }

    /// PT-canonical meta-typed signature gate: cold-cache fallback for the
    /// rule-firing "return verbatim" path. Returns true iff ANY rule matching
    /// (head, arity) has `lhs_head_all_meta_typed = true`. O(candidates).
    pub fn any_rule_lhs_head_all_meta_typed(&self, head: &str, arity: usize) -> bool {
        use crate::backend::models::gc_allocator::global_allocator;
        let interned: &'static str = global_allocator().alloc_str(head);
        if let Some(group) = self.by_head_arity.get(&(interned, arity)) {
            if group
                .get_candidates(None)
                .any(|e| e.lhs_head_all_meta_typed)
            {
                return true;
            }
        }
        self.wildcard.iter().any(|e| e.lhs_head_all_meta_typed)
    }

    /// Collect candidates with discrimination tree pruning applied.
    ///
    /// Returns candidates filtered by the disc tree (if one exists for the group),
    /// plus all wildcard rules (which are always included since they match any head).
    /// The disc tree filter only applies to group-level entries whose
    /// `rule_index_in_group` is in the disc tree's index space.
    pub fn get_candidates_filtered(
        &self,
        head: &str,
        arity: usize,
        first_arg_head: Option<&str>,
        expr: &V,
    ) -> SmallVec<[&RuleEntry<V>; 16]> {
        use crate::backend::models::gc_allocator::global_allocator;
        let interned: &'static str = global_allocator().alloc_str(head);

        // H8 (2026-05-05): detect if the caller's first arg is empty `()`.
        // If so, skip rules whose RHS unconditionally calls `(decons-atom $first-arg)`
        // — those rules are structurally guaranteed to produce empty branches.
        // Audit #7a projected 78.7% wall savings on mmverify by eliminating
        // 1065/2013 wasted match-atom branches.
        let first_arg_is_empty = expr
            .as_sexpr()
            .and_then(|items| items.get(1))
            .and_then(|first_arg| first_arg.as_sexpr())
            .map(|inner| inner.is_empty())
            .unwrap_or(false);

        let mut result = SmallVec::new();

        if let Some(group) = self.by_head_arity.get(&(interned, arity)) {
            let disc_filter = group.disc_tree.as_ref().map(|tree| tree.query(expr));

            // Collect group candidates, filtered by disc tree if available
            for entry in group.get_candidates(first_arg_head) {
                if let Some(ref allowed) = disc_filter {
                    if !allowed.contains(&entry.rule_index_in_group) {
                        continue;
                    }
                }
                // H8: skip empty-branch-guaranteed rules when first arg is `()`.
                if first_arg_is_empty && entry.requires_non_empty_first_arg {
                    continue;
                }
                result.push(entry);
            }
        }

        // Wildcard rules are always included — they're not in any group's disc tree
        // (also skip empty-branch-guaranteed wildcard rules)
        for entry in self.wildcard.iter() {
            if first_arg_is_empty && entry.requires_non_empty_first_arg {
                continue;
            }
            result.push(entry);
        }
        result
    }

    /// Get all rules (for no-head queries).
    pub fn get_all_rules(&self) -> impl Iterator<Item = &RuleEntry<V>> {
        self.by_head_arity
            .values()
            .flat_map(|group| group.all_entries())
            .chain(self.wildcard.iter())
    }

    /// Check if there are any wildcard rules (rules with variable heads).
    #[inline]
    pub fn has_wildcard_rules(&self) -> bool {
        !self.wildcard.is_empty()
    }

    /// Number of wildcard rules (rules with variable heads).
    #[inline]
    pub fn wildcard_len(&self) -> usize {
        self.wildcard.len()
    }

    /// Number of rules in a specific `(head, arity)` group, or 0 if no group exists.
    pub fn group_size(&self, head: &str, arity: usize) -> usize {
        use crate::backend::models::gc_allocator::global_allocator;
        let interned: &'static str = global_allocator().alloc_str(head);
        self.by_head_arity
            .get(&(interned, arity))
            .map(|g| g.len())
            .unwrap_or(0)
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
        #[cfg(feature = "index-gc")]
        {
            crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
                |satb_active| self.clear_inner(satb_active),
            );
        }
        #[cfg(not(feature = "index-gc"))]
        {
            self.clear_inner(false);
        }
    }

    fn clear_inner(&mut self, satb_active: bool) {
        #[cfg(not(feature = "index-gc"))]
        let _ = satb_active;

        #[cfg(feature = "index-gc")]
        if satb_active {
            for entry in self
                .by_head_arity
                .values()
                .flat_map(|group| group.all_entries())
            {
                shade_rule_entry_for_satb(entry);
            }
            for entry in self.wildcard.iter() {
                shade_rule_entry_for_satb(entry);
            }
        }
        self.by_head_arity.clear();
        self.wildcard.clear();
        // Phase 11.A — reset the bloom alongside the index.
        self.rule_rhs_atoms = PerHeadAtomIndex::new();
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

    /// Convert this path to a `Vec<u16>` for serialization in trace events.
    #[inline]
    fn to_vec(&self) -> Vec<u16> {
        (0..self.len as usize)
            .map(|i| self.indices[i] as u16)
            .collect()
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
    fn navigate_resolving<V>(&self, root: &V, bindings: &GenericBindings<V>) -> Option<V>
    where
        V: MettaValueTrait + Clone,
    {
        // Resolve a single node: if it is a bound variable, return the binding.
        #[inline(always)]
        fn resolve_one<V: MettaValueTrait + Clone>(val: &V, bindings: &GenericBindings<V>) -> V {
            if let Some(name) = val.as_atom() {
                if (name.starts_with('$')
                    || (name.starts_with('&')
                        && name != "&"
                        && name != "&self"
                        && name != "&kb"
                        && name != "&stack")
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
    Arity { path: MatchPath, expected: u16 },
    /// Check that the node at `path` is an atom equal to `expected`.
    /// Atom strings are interned (`&'static str`), so this is typically a pointer comparison.
    Atom {
        path: MatchPath,
        expected: &'static str,
    },
    /// Check that the node at `path` is a Long integer equal to `expected`.
    Long { path: MatchPath, expected: i64 },
    /// Check that the node at `path` is a Bool equal to `expected`.
    Bool { path: MatchPath, expected: bool },
    /// Check that the node at `path` is a Float with bits equal to `expected_bits`.
    /// Uses bitwise comparison to avoid NaN issues.
    Float { path: MatchPath, expected_bits: u64 },
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
    Bind { path: MatchPath, name: &'static str },
    /// Check that the value at `path` equals the already-bound variable at `bind_index`.
    /// Used for repeated variables like `(f $x $x)` where the second occurrence must
    /// equal the first.
    EqualCheck { path: MatchPath, bind_index: u8 },
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

        if !Self::analyze_node(
            lhs,
            MatchPath::root(),
            &mut checks,
            &mut var_ops,
            &mut seen_vars,
        ) {
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
            // Dotted-pair pattern detection (2026-05-11): patterns of the form
            // `(a1 ... a_{n-2} . $rest)` are not supported by the structural
            // matcher (which assumes exact arity). Bail to the pattern_match
            // fallback path which handles cons-list head/tail binding.
            if items.len() >= 2 && items[items.len() - 2].as_atom() == Some(".") {
                return false;
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
    ///
    /// For diagnostic tracing of *why* a match failed, see
    /// [`Self::try_match_with_detail`] which is used by the `eval-trace`
    /// feature when the `METTA_TRACE_RULE_MATCH_HEADS` filter is active.
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
                StructuralCheck::Float {
                    path,
                    expected_bits,
                } => {
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
                        // Structural equality failed. Fall back to bidirectional
                        // (Martelli-Montanari) unification: this allows free
                        // variables in the input expression to bind against the
                        // already-bound rule variable. PLN's inference rules
                        // depend on this behavior — e.g., the Modus Ponens rule
                        //
                        //     (= (|- ($A $T1) ((Implication $A $B) $T2)) ...)
                        //
                        // matched against
                        //
                        //     (|- ((Inheritance Anna (IntSet smokes)) (stv 1 0.9))
                        //         ((Implication (Inheritance $1 (IntSet smokes))
                        //                       (Inheritance $1 (IntSet cancerous)))
                        //          (stv 0.6 0.9)))
                        //
                        // requires unifying the second occurrence of $A
                        // (already bound to `(Inheritance Anna (IntSet smokes))`)
                        // with `(Inheritance $1 (IntSet smokes))` from the
                        // implication. Structural equality fails because $1 ≠ Anna,
                        // but unification succeeds with $1 → Anna.
                        let unify_bindings =
                            match crate::backend::eval::bindings::bidirectional_unify_generic(
                                bound, val,
                            ) {
                                Some(b) => b,
                                None => return None,
                            };
                        // Merge the new bindings into the existing ones. Conflicts
                        // would mean the same variable is bound to incompatible
                        // values across the two unification calls — bail out.
                        for (var_name, var_val) in unify_bindings.iter() {
                            if let Some(existing) = bindings.get(var_name) {
                                if existing != var_val {
                                    return None;
                                }
                            } else {
                                bindings.insert(var_name, var_val.clone());
                            }
                        }
                    }
                }
            }
        }

        Some(bindings)
    }

    /// Diagnostic variant of [`Self::try_match`] that returns a rich
    /// failure description on mismatch. Used by the `eval-trace` feature
    /// to emit `RuleMatchAttempt` events explaining *why* a candidate
    /// rule did not fire at a given call site.
    ///
    /// The implementation mirrors `try_match` exactly, but every place
    /// where `try_match` returns `None` is replaced with an `Err(detail)`
    /// carrying the check index, path, expected value, and actual value.
    ///
    /// **Performance**: this method is **only** called when the trace
    /// filter has admitted the call site (see
    /// [`crate::backend::trace::rule_match::RuleMatchFilter`]). The fast
    /// path remains `try_match` itself, byte-identical to the original.
    #[cfg(feature = "trace")]
    pub fn try_match_with_detail<V>(
        &self,
        expr: &V,
    ) -> Result<GenericBindings<V>, crate::backend::trace::rule_match::DetailedFailure<V>>
    where
        V: MettaValueTrait + Clone + PartialEq,
    {
        use crate::backend::trace::rule_match::DetailedFailure;

        // Phase 1: Structural checks (mirror of try_match's logic with
        // explicit failure detail).
        for (check_index, check) in self.checks.iter().enumerate() {
            let check_index = check_index as u32;
            match check {
                StructuralCheck::Arity { path, expected } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: None,
                            })
                        }
                    };
                    let items = match val.as_sexpr() {
                        Some(items) => items,
                        None => {
                            return Err(DetailedFailure::StructuralCheckFailed {
                                check_index,
                                check_kind: "arity",
                                path: path.to_vec(),
                                expected_arity: Some(*expected as usize),
                                expected_atom: None,
                                actual: Some(val.clone()),
                            })
                        }
                    };
                    if items.len() != *expected as usize {
                        return Err(DetailedFailure::StructuralCheckFailed {
                            check_index,
                            check_kind: "arity",
                            path: path.to_vec(),
                            expected_arity: Some(*expected as usize),
                            expected_atom: None,
                            actual: Some(val.clone()),
                        });
                    }
                }
                StructuralCheck::Atom { path, expected } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: None,
                            })
                        }
                    };
                    let atom = match val.as_atom() {
                        Some(a) => a,
                        None => {
                            return Err(DetailedFailure::StructuralCheckFailed {
                                check_index,
                                check_kind: "atom",
                                path: path.to_vec(),
                                expected_arity: None,
                                expected_atom: Some((*expected).to_string()),
                                actual: Some(val.clone()),
                            })
                        }
                    };
                    if atom != *expected {
                        return Err(DetailedFailure::StructuralCheckFailed {
                            check_index,
                            check_kind: "atom",
                            path: path.to_vec(),
                            expected_arity: None,
                            expected_atom: Some((*expected).to_string()),
                            actual: Some(val.clone()),
                        });
                    }
                }
                StructuralCheck::Long { path, expected: _ } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: None,
                            })
                        }
                    };
                    if val.as_long().map(|n| {
                        n == match check {
                            StructuralCheck::Long { expected, .. } => *expected,
                            _ => unreachable!(),
                        }
                    }) != Some(true)
                    {
                        return Err(DetailedFailure::StructuralCheckFailed {
                            check_index,
                            check_kind: "long",
                            path: path.to_vec(),
                            expected_arity: None,
                            expected_atom: None,
                            actual: Some(val.clone()),
                        });
                    }
                }
                StructuralCheck::Bool { path, expected } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: None,
                            })
                        }
                    };
                    if val.as_bool() != Some(*expected) {
                        return Err(DetailedFailure::StructuralCheckFailed {
                            check_index,
                            check_kind: "bool",
                            path: path.to_vec(),
                            expected_arity: None,
                            expected_atom: None,
                            actual: Some(val.clone()),
                        });
                    }
                }
                StructuralCheck::Float {
                    path,
                    expected_bits,
                } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: None,
                            })
                        }
                    };
                    if val.as_float().map(|f| f.to_bits()) != Some(*expected_bits) {
                        return Err(DetailedFailure::StructuralCheckFailed {
                            check_index,
                            check_kind: "float",
                            path: path.to_vec(),
                            expected_arity: None,
                            expected_atom: None,
                            actual: Some(val.clone()),
                        });
                    }
                }
                StructuralCheck::Str { path, expected } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: None,
                            })
                        }
                    };
                    if val.as_string() != Some(*expected) {
                        return Err(DetailedFailure::StructuralCheckFailed {
                            check_index,
                            check_kind: "str",
                            path: path.to_vec(),
                            expected_arity: None,
                            expected_atom: None,
                            actual: Some(val.clone()),
                        });
                    }
                }
            }
        }

        // Phase 2: Variable bindings.
        let mut bindings = GenericBindings::new();
        let mut bound_values: SmallVec<[&V; 8]> = SmallVec::new();

        for op in &self.var_ops {
            match op {
                VarOp::Bind { path, name } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: Some((*name).to_string()),
                            })
                        }
                    };
                    bound_values.push(val);
                    bindings.insert(*name, val.clone());
                }
                VarOp::EqualCheck { path, bind_index } => {
                    let val = match path.navigate(expr) {
                        Some(v) => v,
                        None => {
                            return Err(DetailedFailure::PathNavigateFailed {
                                path: path.to_vec(),
                                var: None,
                            })
                        }
                    };
                    let bound = bound_values[*bind_index as usize];
                    if val != bound {
                        // Bidirectional unification fallback (mirrors try_match).
                        let unify_bindings =
                            match crate::backend::eval::bindings::bidirectional_unify_generic(
                                bound, val,
                            ) {
                                Some(b) => b,
                                None => {
                                    return Err(DetailedFailure::BidirectionalUnifyFailed {
                                        var: "<repeated>".to_string(),
                                        bound: bound.clone(),
                                        candidate: val.clone(),
                                        reason: "unification-failed",
                                    })
                                }
                            };
                        for (var_name, var_val) in unify_bindings.iter() {
                            if let Some(existing) = bindings.get(var_name) {
                                if existing != var_val {
                                    return Err(DetailedFailure::EqualCheckFailed {
                                        var: var_name.to_string(),
                                        first_value: existing.clone(),
                                        second_value: var_val.clone(),
                                    });
                                }
                            } else {
                                bindings.insert(var_name, var_val.clone());
                            }
                        }
                    }
                }
            }
        }

        Ok(bindings)
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
                StructuralCheck::Float {
                    path,
                    expected_bits,
                } => {
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
                        // Structural equality failed. Fall back to bidirectional
                        // (Martelli-Montanari) unification for repeated rule
                        // variables — see the matching block in `try_match` for
                        // the full rationale (PLN Modus Ponens with free
                        // variables in implications).
                        let unify_bindings =
                            match crate::backend::eval::bindings::bidirectional_unify_generic(
                                bound, &val,
                            ) {
                                Some(b) => b,
                                None => return None,
                            };
                        for (var_name, var_val) in unify_bindings.iter() {
                            if let Some(existing) = bindings.get(var_name) {
                                if existing != var_val {
                                    return None;
                                }
                            } else {
                                bindings.insert(var_name, var_val.clone());
                            }
                        }
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
// ============================================================================
// LHS specificity score (MTT SUPERSET — opt-in via `rule-fire-mode specificity`)
// ============================================================================

/// Constructor symbols contribute this much per depth level. Picked large
/// enough that one constructor at any depth outweighs all repeat-var penalties
/// in any realistic pattern.
const SPECIFICITY_W_CONSTRUCTOR: u32 = 1000;

/// Repeat occurrences of the same variable contribute this much (encodes the
/// "occurs check" intuition: `(f $x $x)` is strictly more specific than
/// `(f $x $y)` even though they have the same constructor count).
const SPECIFICITY_W_REPEAT_VAR: u32 = 1;

/// Compute the structural specificity score of a rule's LHS pattern.
///
/// **Higher = more specific.** Score is the sum, over every position in the
/// LHS tree, of:
/// - `SPECIFICITY_W_CONSTRUCTOR * (1 + depth)` for constructor atoms,
///   literals, and S-expression head positions.
/// - `SPECIFICITY_W_REPEAT_VAR` for repeated variable occurrences.
/// - `0` for first-occurrence variables and `_` wildcards.
///
/// **Why this metric replaces the removed NewVar-count:**
///
/// The original removed metric counted NewVar tags in the De Bruijn encoding
/// and selected the rule with the *fewest* tags as "most specific". This
/// inverts the order when variables appear nested inside constructor
/// wrappers — e.g., PLN's `(f ((Implication $A $B) $TV) $Y)` has 4
/// vars but is more structurally constrained than `(f ($c $tv) $y)` (3 vars).
///
/// The corrected metric weights constructor positions by their depth, which
/// is monotone with the subsumption ordering of MeTTa first-order patterns:
/// rule A more-specific-than rule B (in the subsumption sense) implies
/// `lhs_specificity(A) >= lhs_specificity(B)`. Ties keep all candidates
/// (graceful degradation to HE nondet for genuinely incomparable patterns).
///
/// Stack-safe via explicit work-stack (no recursion).
pub(crate) fn lhs_specificity<V: MettaValueTrait + Clone>(lhs: &V) -> u32 {
    let mut total: u32 = 0;
    // Stack-allocated seen-var set; falls back to heap if patterns get huge.
    let mut seen_vars: smallvec::SmallVec<[&'static str; 8]> = smallvec::SmallVec::new();
    // Work-stack of (value-as-erased-pointer, depth). Using indices into
    // a Vec keeps lifetimes simple while still being iterative.
    let mut stack: smallvec::SmallVec<[(V, u32); 16]> = smallvec::SmallVec::new();
    stack.push((lhs.clone(), 0));

    while let Some((val, depth)) = stack.pop() {
        if let Some(items) = val.as_sexpr() {
            // S-expression: push every child for processing. The head atom
            // contributes via its own per-atom handling below — no need to
            // double-count here.
            for child in items.iter() {
                stack.push((child.clone(), depth.saturating_add(1)));
            }
            continue;
        }
        if let Some(name) = val.as_atom() {
            // Heuristic var detector — matches `$x`, `'y`, and namespace `&y`
            // sigils (but excludes the special `&self`/`&kb`/`&stack` tokens
            // which are treated as ground name atoms by the matcher).
            let is_var = name.len() > 1
                && (name.starts_with('$')
                    || name.starts_with('\'')
                    || (name.starts_with('&')
                        && name != "&self"
                        && name != "&kb"
                        && name != "&stack"));
            if is_var {
                // Use a static-str hack: SmallVec stores the &'static str only
                // for comparison; we don't keep references past this iteration.
                // SAFETY: `name` is a &str with the same lifetime as `val`'s
                // backing slab string; it lives at least until the end of this
                // function. Transmuting to 'static is a known borrow-checker
                // workaround used elsewhere in the codebase.
                let name_static: &'static str = unsafe { std::mem::transmute(name) };
                if seen_vars.contains(&name_static) {
                    total = total.saturating_add(SPECIFICITY_W_REPEAT_VAR);
                } else {
                    seen_vars.push(name_static);
                }
            } else if name != "_" {
                // Constructor or head atom — concrete constraint at this depth.
                total = total.saturating_add(
                    SPECIFICITY_W_CONSTRUCTOR.saturating_mul(depth.saturating_add(1)),
                );
            }
            // `_` wildcard contributes 0.
            continue;
        }
        // Literals (Long/Float/Bool/String/Unit) are concrete constraints —
        // they require an exact value match at this position.
        if val.as_long().is_some()
            || val.as_float().is_some()
            || val.as_bool().is_some()
            || val.as_string().is_some()
        {
            total = total
                .saturating_add(SPECIFICITY_W_CONSTRUCTOR.saturating_mul(depth.saturating_add(1)));
            continue;
        }
        // Other value types (Type, Quoted, Space, etc.) — treat as opaque
        // constructors at their position.
        total = total.saturating_add(SPECIFICITY_W_CONSTRUCTOR);
    }

    total
}

// ============================================================================
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
    let expr = Expr {
        ptr: bytes.as_ptr().cast_mut(),
    };
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
/// Walk a value tree and collect every `$`-prefixed atom name into
/// `out` (strings, not pointer identities). Used by the Phase 3.2-B
/// trace emission to show which variable occurrences exist in a RHS
/// template before/after freshening.
#[cfg(feature = "trace")]
fn collect_variable_names_into<V: MettaValueTrait>(value: &V, out: &mut Vec<String>) {
    let mut stack: Vec<&V> = Vec::new();
    stack.push(value);
    while let Some(v) = stack.pop() {
        if let Some(name) = v.as_atom() {
            if name.starts_with('$') && name != "_" {
                out.push(name.to_string());
            }
        } else if let Some(items) = v.as_sexpr() {
            for item in items.iter() {
                stack.push(item);
            }
        } else if let Some(goals) = v.as_conjunction() {
            for goal in goals.iter() {
                stack.push(goal);
            }
        }
    }
}

/// Move a `GenericBindings<MettaValue>` into the generic `V` spelling after
/// a `TypeId` guard has proved that `V == MettaValue`.
///
/// This must be a move, not `transmute_copy`: `GenericBindings` owns
/// `BindingName::Ephemeral(Arc<str>)` keys, and bitwise-copying the container
/// duplicates those `Arc` handles without incrementing their reference counts.
unsafe fn move_metta_bindings_to_v_unchecked<V>(
    bindings: GenericBindings<crate::backend::models::MettaValue>,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone + 'static,
{
    debug_assert_eq!(
        std::any::TypeId::of::<V>(),
        std::any::TypeId::of::<crate::backend::models::MettaValue>()
    );
    let bindings = std::mem::ManuallyDrop::new(bindings);
    unsafe {
        std::ptr::read(
            (&*bindings as *const GenericBindings<crate::backend::models::MettaValue>)
                .cast::<GenericBindings<V>>(),
        )
    }
}

#[inline]
fn export_rule_match_bindings<V, F>(
    scratch: &GenericBindings<V>,
    query: &V,
    current_rule_prefix: &str,
    dispatch_scope: crate::backend::models::generic_bindings::ScopeId,
    factory: &F,
) -> Option<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    crate::backend::eval::bindings::export_query_bindings_generic(
        scratch,
        query,
        current_rule_prefix,
        &[
            dispatch_scope,
            crate::backend::models::generic_bindings::ROOT_SCOPE,
        ],
        factory,
    )
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn move_metta_bindings_to_v_preserves_ephemeral_keys_without_copying_owner() {
        let mut bindings = GenericBindings::<crate::backend::models::MettaValue>::new();
        bindings.insert(
            "$__fr_123_owned",
            crate::backend::models::MettaValue::Atom("value".to_string()),
        );

        let moved: GenericBindings<crate::backend::models::MettaValue> =
            unsafe { move_metta_bindings_to_v_unchecked(bindings) };

        assert_eq!(
            moved.get("$__fr_123_owned").and_then(|v| v.as_atom()),
            Some("value")
        );
    }
}

/// H8 (2026-05-05): Detect whether a rule body would produce only empty
/// branches when invoked with an empty-sexpr first argument.
///
/// Returns `true` if the LHS's first arg is a variable AND the RHS contains
/// `(decons-atom ...)` anywhere. Such rules iterate over a list-shape input
/// via decons-atom; when the input is `()`, decons-atom returns 0 results
/// and the chain produces `(empty)` — wasted work.
///
/// Used by `get_candidates_filtered` to elide structurally-empty branches
/// at dispatch time. Audit #7a: 1065/2013 (53%) of mmverify's match-atom
/// fork:3 branches are structurally empty without this filter.
/// PT-canonical: rule body preserves its arguments verbatim when the body's
/// top-level head is a "lazy" special form (see `is_lazy_body_form`).
/// Returns true iff `rhs` is an S-expression whose head atom is lazy.
///
/// Used to flag `RuleEntry::body_wants_lazy_args` so the dispatcher's Step-2
/// pre-eval is skipped for calls to LHS-heads of such rules. Without the
/// skip, `(=> $A $C $stv) → (add-atom &self (= $C (Truth_MP $A $stv)))`
/// would eagerly reduce `$A` (an arg of `=>`) against any existing rules
/// for $A's head, breaking the PT-canonical variable-preserving registration.
pub(crate) fn rhs_head_is_lazy_form<V: MettaValueTrait>(rhs: &V) -> bool {
    if let Some(items) = rhs.as_sexpr() {
        if let Some(head) = items.first().and_then(|h| h.as_atom()) {
            return crate::backend::eval::helpers::is_lazy_body_form(head);
        }
    }
    false
}

/// Phase 1 cut-barrier (control substrate): returns true iff `v` lexically
/// contains the atom `cut` as an APPLIED HEAD `(cut ...)` anywhere in its
/// tree, NOT shadowed inside a `quote` wrapper.
///
/// Used to precompute `RuleEntry::body_contains_cut` at `add_rule` time and,
/// via the same predicate, to decide at dispatch time whether a rule's RHS
/// opens a new cut barrier (`dispatch_rule_matches`). A `(cut)` inside the
/// rule body must prune the enclosing clause's nondeterminism; this scan
/// identifies which rule bodies carry that obligation.
///
/// We descend into S-expression children but NOT into `Quoted` wrappers,
/// because `(quote (cut))` is data, not a control cut (mirrors the way the
/// evaluator treats quoted forms as inert). Note: the legacy
/// `contains_atom_recursive` in `core.rs` (feeding the bloom-backed
/// `rule_rhs_contains_atom`) matches a bare `cut` atom in any position; this
/// predicate is the precise applied-head, quote-aware variant the barrier
/// lifecycle needs.
///
/// **Stack-safety mandate (2026-05-15)**: implemented with an explicit
/// work-list (no Rust call recursion), so a deeply-nested rule body cannot
/// overflow the native stack. Runs once per rule at load time, never in the
/// eval hot loop.
pub(crate) fn expr_contains_cut<V: MettaValueTrait>(v: &V) -> bool {
    // Small inline stack; rule bodies are shallow in practice, and the
    // SmallVec spills to the heap rather than the C stack for the rare deep
    // body — keeping the scan stack-safe regardless of nesting depth.
    let mut work: SmallVec<[&V; 16]> = SmallVec::new();
    work.push(v);
    while let Some(node) = work.pop() {
        // Quoted forms are inert data — a `(cut)` inside `(quote ...)` is not
        // a control cut, so we do not descend into the quoted payload.
        if node.is_quoted() {
            continue;
        }
        if let Some(items) = node.as_sexpr() {
            // Applied head `(cut ...)` — the control cut we are looking for.
            if let Some(head) = items.first().and_then(|h| h.as_atom()) {
                if head == "cut" {
                    return true;
                }
            }
            for item in items {
                work.push(item);
            }
        }
    }
    false
}

/// PT-canonical meta-typed signature check. Returns true iff the head has
/// at least one declared arrow type where ALL arg types AND the return
/// type are meta-types per `is_meta_type`. Consulted at rule insertion
/// time to cache `RuleEntry::lhs_head_all_meta_typed`.
pub(crate) fn lhs_head_signature_all_meta_typed<V, F>(
    lhs: &V,
    env: &GenericEnvironment<V, F>,
) -> bool
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: crate::backend::models::MettaValueFactory<V> + Clone,
{
    use crate::backend::eval::step::grounded::{
        extract_arg_types, extract_return_type, is_arrow_type, is_meta_type,
    };

    let head = match lhs
        .as_sexpr()
        .and_then(|items| items.first())
        .and_then(|h| h.as_atom())
    {
        Some(h) => h,
        None => return false,
    };

    let types = env.get_types_generic(head);
    if types.is_empty() {
        return false;
    }

    // PT-canonical: ANY arrow declaration with all-meta args+return is enough
    // (one signature establishes the contract).
    types.iter().any(|t| {
        if !is_arrow_type(t) {
            return false;
        }
        let arg_types = match extract_arg_types(t) {
            Some(args) => args,
            None => return false,
        };
        let return_type = match extract_return_type(t) {
            Some(rt) => rt,
            None => return false,
        };
        arg_types.iter().all(is_meta_type) && is_meta_type(&return_type)
    })
}

pub(crate) fn rule_requires_non_empty_first_arg<V: MettaValueTrait>(lhs: &V, rhs: &V) -> bool {
    let lhs_items = match lhs.as_sexpr() {
        Some(items) if items.len() >= 2 => items,
        _ => return false,
    };
    // First arg must be a variable for this rule to fire on empty `()`.
    let first_arg_is_var = lhs_items[1]
        .as_atom()
        .map(|s| s.starts_with('$'))
        .unwrap_or(false);
    if !first_arg_is_var {
        return false;
    }
    fn rhs_uses_decons_atom<V: MettaValueTrait>(v: &V, depth: u32) -> bool {
        if depth == 0 {
            return false;
        }
        if let Some(items) = v.as_sexpr() {
            if let Some(head) = items.first().and_then(|h| h.as_atom()) {
                if head == "decons-atom" {
                    return true;
                }
            }
            for item in items {
                if rhs_uses_decons_atom(item, depth - 1) {
                    return true;
                }
            }
        }
        false
    }
    rhs_uses_decons_atom(rhs, 16)
}

/// PLN-fix 2026-04: Detect whether a type expression contains any freshened
/// variable (atom name starting with `$__fr_`). Used by `run_type_fixpoint`
/// (and any other type-registry update site) to gate `register_inferred_type`
/// calls — freshened vars in a registered type would poison the global type
/// registry and break type-directed dispatch downstream. Rule RHSes are
/// freshened at load time, so naive inference over them may surface
/// `$__fr_*` atoms in the inferred type.
pub(crate) fn type_contains_freshened_var<V: MettaValueTrait>(t: &V) -> bool {
    if let Some(name) = t.as_atom() {
        if name.starts_with("$__fr_") {
            return true;
        }
    }
    if let Some(items) = t.as_sexpr() {
        for item in items {
            if type_contains_freshened_var(item) {
                return true;
            }
        }
    }
    false
}

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
    use crate::backend::wide_mork::encoding::{decode_leb128, WideTag};

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

        // Phase 2.x PT cons-pattern rewrite (2026-05-22):
        // PeTTa rules can use `(cons HEAD TAIL)` LHS patterns to destructure
        // any SExpr (e.g. PLN's `(= (=> (cons , $args) $C $stvImp) ...)`
        // matches `(=> (, A B) C STV)` with $args bound to the tail (A B)).
        // MTT's unifier doesn't natively recognize `cons` as a destructure
        // — but it DOES recognize the equivalent dotted-pair `(HEAD . TAIL)`.
        // Rewrite the LHS in-place at rule-add time: any `(cons X Y)`
        // sub-pattern becomes `(X . Y)`. The transformation is structural
        // and idempotent.
        let lhs = rewrite_cons_to_dotted_pair(lhs, &self.factory);

        // Phase 9.5: Invalidate normal-form memoization — new rules may make
        // previously normal-form expressions reducible.
        crate::backend::eval::trampoline::invalidate_normal_form_memo();

        // Clear eval memo and match result caches — new rules may change
        // evaluation and matching results for previously cached expressions.
        crate::backend::eval::trampoline::clear_eval_memo();
        crate::backend::eval::trampoline::clear_match_result_cache();

        // Increment rule/type epoch — invalidates cached TypeSignatureRegistry in JIT.
        increment_rule_epoch();

        // Rules are stored with ORIGINAL variable names. Per-match freshening
        // (Phase 3.2-A, commit 748a15b) at all match sites in this file allocates
        // a fresh epoch per query and renames RHS variables, mirroring MeTTa HE's
        // `make_variables_unique` (called at retrieval, not insertion). Storing
        // already-freshened rules combined with per-match re-freshening produced
        // nested compound names like `$__fr_84___fr_83_x` — see Robot.metta OOM.

        // Get head symbol and arity for bloom filter (clone head string before moving lhs)
        let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
        let arity = lhs.get_arity();

        // Phase 8.1: Compute RHS type at insertion time for branch pruning (Phase 8.7).
        // Only stores non-trivial types — %Undefined% provides no pruning benefit.
        let rhs_type = {
            use crate::backend::eval::types::infer_type_generic;
            let inferred = infer_type_generic(&rhs, &self.factory, self);
            if inferred.as_atom() == Some("%Undefined%") {
                None
            } else {
                Some(inferred)
            }
        };

        // Trace: RhsTypeComputed
        #[cfg(feature = "trace")]
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
                        rhs_type: rhs_type
                            .as_ref()
                            .map(crate::backend::trace::trace_value_generic),
                    },
                );
            });
        }

        // Phase 10.1: Register inferred return type in function return type index.
        // Makes rhs_type queryable by infer_types_generic for user-defined functions
        // without explicit (: f (-> ...)) type declarations.
        //
        // PLN-fix 2026-04: gate against freshened-var leakage. Rule RHSes are
        // freshened at load time; naive inference may produce a type containing
        // `$__fr_*` atoms, which would poison the global type registry and
        // break type-directed dispatch downstream.
        if let Some(ref rt) = rhs_type {
            if let Some(ref head) = head_owned {
                if !type_contains_freshened_var(rt) {
                    self.register_inferred_type(head, rt);

                    // Trace: InferredTypeRegistered (Phase 10.1)
                    #[cfg(feature = "trace")]
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
                use crate::backend::eval::types::infer_arrow_type_from_rule;
                if let Some(arrow) =
                    infer_arrow_type_from_rule(&lhs, &rhs, rhs_type.as_ref(), &self.factory, self)
                {
                    // PLN-fix 2026-04: gate against freshened-var leakage
                    // (same reasoning as the phase-10.1-rhs site above).
                    if !type_contains_freshened_var(&arrow) {
                        self.register_inferred_type(head, &arrow);

                        // Trace: InferredTypeRegistered (Phase 10.4)
                        #[cfg(feature = "trace")]
                        {
                            crate::backend::trace::thread_local_sink::with_trace_collector_ref(
                                |tc| {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        0,
                                        crate::backend::trace::trace_value_generic(&arrow),
                                        vec![],
                                        None,
                                        trace_format::TraceEventKind::InferredTypeRegistered {
                                            function_name: head.clone(),
                                            registered_type:
                                                crate::backend::trace::trace_value_generic(&arrow),
                                            source: "phase-10.4-arrow".to_string(),
                                        },
                                    );
                                },
                            );
                        }
                    }
                }
            }
        }

        // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
        if let Some(ref head) = head_owned {
            self.shared.fuzzy_matcher.write().insert(head);
        }

        // Create rule s-expression: (= lhs rhs)
        let rule_sexpr = self
            .factory
            .sexpr(vec![self.factory.atom("="), lhs.clone(), rhs.clone()]);

        // Convert to De Bruijn bytes and insert into PathMap + RuleIndex
        let rule_prefix_len = super::core::RULE_PREFIX_LEN;
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
                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);

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
                            lhs_start,
                            lhs_start + lhs_byte_len,
                            debruijn_bytes.len()
                        );
                    }
                    let first_lhs_byte = debruijn_bytes[lhs_start];
                    if let Err(reserved) = maybe_byte_item(first_lhs_byte) {
                        panic!(
                            "LHS starts with reserved byte 0x{:02x} at offset {} in {:02x?}",
                            reserved,
                            lhs_start,
                            &debruijn_bytes[..debruijn_bytes.len().min(32)]
                        );
                    }
                }

                // Extract LHS De Bruijn bytes with one extra zero byte of padding.
                // See the comment on expr_bytes_owned in match_rules_native() for why
                // padding is needed (ExprZipper::gnext reads one byte past the end).
                let mut lhs_debruijn = Vec::with_capacity(lhs_byte_len + 1);
                lhs_debruijn
                    .extend_from_slice(&debruijn_bytes[lhs_start..lhs_start + lhs_byte_len]);
                lhs_debruijn.push(0x00); // Padding byte for ExprZipper read-past-end safety

                // Also save the full rule De Bruijn bytes for alpha-equivalent
                // rule removal in remove_rule / remove_from_space. This is the
                // authoritative key for the rule: two rules compare equal iff
                // their full debruijn bytes match.
                let full_debruijn = debruijn_bytes.to_vec();

                // Validate ALL bytes in lhs_debruijn are valid MORK (no reserved 0x40-0x7F)
                #[cfg(debug_assertions)]
                {
                    if let Err((off, byte)) = validate_mork_bytes(&lhs_debruijn) {
                        panic!(
                            "lhs_debruijn has invalid byte 0x{:02x} at offset {} (len={}).\n\
                             lhs_debruijn: {:02x?}\n\
                             full debruijn_bytes (first 64): {:02x?}\n\
                             rule_prefix_len: {}, lhs_start: {}, lhs_byte_len: {}",
                            byte,
                            off,
                            lhs_debruijn.len(),
                            &lhs_debruijn[..lhs_debruijn.len().min(32)],
                            &debruijn_bytes[..debruijn_bytes.len().min(64)],
                            rule_prefix_len,
                            lhs_start,
                            lhs_byte_len
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
                // I-2: If StructuralMatcher fails (e.g., depth > 8), try EnhancedMatcher
                let enhanced_matcher = if structural_matcher.is_none() {
                    crate::backend::eval::cesk::EnhancedMatcher::analyze(&lhs)
                } else {
                    None
                };
                let compiled_rhs: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> =
                    if std::any::TypeId::of::<V>()
                        == std::any::TypeId::of::<crate::backend::models::MettaValue>()
                    {
                        let metta_rhs: &crate::backend::models::MettaValue = unsafe {
                            &*(&rhs as *const V as *const crate::backend::models::MettaValue)
                        };
                        if crate::backend::bytecode::can_compile_with_env(metta_rhs) {
                            crate::backend::bytecode::compile_bytecode_arc("rule_rhs", metta_rhs)
                                .ok()
                                .map(|chunk| {
                                    chunk as std::sync::Arc<dyn std::any::Any + Send + Sync>
                                })
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                // Check if the inferred RHS type is monadic (IO, StateMonad, etc.)
                let has_monadic_effect = rhs_type.as_ref().map_or(false, |t| {
                    t.is_monadic_type() || t.is_arrow_returning_monadic()
                });
                // H8: detect rules whose RHS does (decons-atom $first-arg-var).
                let requires_non_empty_first_arg = rule_requires_non_empty_first_arg(&lhs, &rhs);
                let body_wants_lazy_args = rhs_head_is_lazy_form(&rhs);
                let lhs_head_all_meta_typed = lhs_head_signature_all_meta_typed(&lhs, self);
                let body_contains_cut = expr_contains_cut(&rhs);
                let entry = RuleEntry {
                    lhs: lhs.clone(),
                    rhs_has_variables: rhs.contains_variables(),
                    rhs: rhs.clone(),
                    lhs_debruijn,
                    lhs_wide_debruijn: Vec::new(), // Narrow path — MORK encoding succeeded
                    full_debruijn,
                    var_names,
                    wildcard_indices,
                    specificity: lhs_specificity(&lhs),
                    multiplicity: 1,
                    rhs_type: rhs_type.clone(),
                    structural_matcher,
                    enhanced_matcher,
                    rule_index_in_group: 0, // Assigned by RuleIndex::add_rule
                    global_rule_index: 0,   // Assigned by RuleIndex::add_rule
                    compiled_rhs,
                    has_monadic_effect,
                    requires_non_empty_first_arg,
                    body_wants_lazy_args,
                    lhs_head_all_meta_typed,
                    body_contains_cut,
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

            self.shared
                .atom_space
                .total_atoms
                .fetch_add(1, Ordering::Relaxed);

            // Encode the LHS with Wide MORK De Bruijn encoding for byte-level matching.
            // This replaces the old structural fallback with O(n) byte-level matching.
            let mut wide_ctx = crate::backend::wide_mork::encoding::WideConversionContext::new();
            let mut lhs_wide_debruijn = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_debruijn(
                &lhs,
                &mut wide_ctx,
                &mut lhs_wide_debruijn,
            );

            // Encode the full rule `(= lhs rhs)` with Wide MORK De Bruijn for
            // alpha-equivalent removal via remove_rule_by_debruijn. Uses a
            // separate fresh context so its variable indices are self-contained.
            let mut full_wide_ctx =
                crate::backend::wide_mork::encoding::WideConversionContext::new();
            let mut full_debruijn = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_debruijn(
                &rule_sexpr,
                &mut full_wide_ctx,
                &mut full_debruijn,
            );

            let lhs_var_count =
                crate::backend::wide_mork::encoding::count_wide_newvar_tags(&lhs_wide_debruijn);
            let (var_names, wildcard_indices) =
                build_var_names_and_wildcards(&wide_ctx.var_names, lhs_var_count);

            let alloc = crate::backend::models::gc_allocator::global_allocator();
            let first_arg_head_interned: Option<&'static str> =
                get_first_arg_head(&lhs).map(|s| alloc.alloc_str(s));
            let structural_matcher = StructuralMatcher::analyze(&lhs);
            let enhanced_matcher = if structural_matcher.is_none() {
                crate::backend::eval::cesk::EnhancedMatcher::analyze(&lhs)
            } else {
                None
            };
            let compiled_rhs: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> =
                if std::any::TypeId::of::<V>()
                    == std::any::TypeId::of::<crate::backend::models::MettaValue>()
                {
                    let metta_rhs: &crate::backend::models::MettaValue = unsafe {
                        &*(&rhs as *const V as *const crate::backend::models::MettaValue)
                    };
                    if crate::backend::bytecode::can_compile_with_env(metta_rhs) {
                        crate::backend::bytecode::compile_bytecode_arc("rule_rhs", metta_rhs)
                            .ok()
                            .map(|chunk| chunk as std::sync::Arc<dyn std::any::Any + Send + Sync>)
                    } else {
                        None
                    }
                } else {
                    None
                };
            // Check if the inferred RHS type is monadic (IO, StateMonad, etc.)
            let has_monadic_effect = rhs_type.as_ref().map_or(false, |t| {
                t.is_monadic_type() || t.is_arrow_returning_monadic()
            });
            let requires_non_empty_first_arg = rule_requires_non_empty_first_arg(&lhs, &rhs);
            let body_wants_lazy_args = rhs_head_is_lazy_form(&rhs);
            let lhs_head_all_meta_typed = lhs_head_signature_all_meta_typed(&lhs, self);
            let body_contains_cut = expr_contains_cut(&rhs);
            let entry = RuleEntry {
                lhs: lhs.clone(),
                rhs_has_variables: rhs.contains_variables(),
                rhs: rhs.clone(),
                lhs_debruijn: Vec::new(), // Empty — this is a wide rule
                lhs_wide_debruijn,
                full_debruijn, // Wide De Bruijn bytes for alpha-equivalent removal
                var_names,
                wildcard_indices,
                specificity: lhs_specificity(&lhs),
                multiplicity: 1,
                rhs_type, // Phase 8.1: computed before closure, last use — no clone needed
                structural_matcher,
                enhanced_matcher,
                rule_index_in_group: 0, // Assigned by RuleIndex::add_rule
                global_rule_index: 0,   // Assigned by RuleIndex::add_rule
                compiled_rhs,
                has_monadic_effect,
                requires_non_empty_first_arg,
                body_wants_lazy_args,
                lhs_head_all_meta_typed,
                body_contains_cut,
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

        // Update bloom filters with (head, arity) for O(1) rejection
        if let Some(ref head) = head_owned {
            let arity_u8 = arity as u8;
            self.shared
                .atom_space
                .head_arity_bloom
                .write()
                .insert(head, arity_u8);
            // Rule-only bloom: used by is_normal_form_bounded to distinguish
            // data constructors (add-atom) from rule heads (=).
            self.shared
                .atom_space
                .rule_head_bloom
                .write()
                .insert(head, arity_u8);

            // If the head is in the overridable set, bump the override
            // bitset so the dispatch arm routes future calls through rule
            // matching instead of the grounded fast path.
            if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                self.shared.dispatch_overrides.note_user_rule_added(id);
            }
        }

        // Rules are stored as (= lhs rhs) atoms in the PathMap. For
        // match_space() queries with (= ...) patterns to pass the bloom
        // filter, we must also insert ("=", 2). Without this, add_rule()
        // only inserts the LHS head (e.g., "father") and match &self
        // with (= ...) patterns is incorrectly rejected by the bloom.
        // S13: Arity convention is `pattern.get_arity()` = `items.len() - 1`
        // (excludes head). So `(= $a $b)` has arity 2, not 3.
        self.shared
            .atom_space
            .head_arity_bloom
            .write()
            .insert("=", 2);

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
    /// MTT SUPERSET (2026-05-17): post-match specificity filter for opt-in
    /// `RuleFireMode::Specificity`. Default `Nondet` (HE-bisim) skips the
    /// filter entirely — a single relaxed atomic read + comparison.
    ///
    /// Reads `pragma_settings.rule_fire_mode` once. If `Specificity` AND
    /// there are 2+ matching rules, retains only those with maximum
    /// `entry_specificity` score. Ties keep all (graceful degradation to
    /// HE-nondet for genuinely incomparable patterns — Ernst et al. (1998)
    /// Theorem 5.3 endorses this fallback).
    #[inline]
    pub(crate) fn apply_rule_fire_mode_filter(&self, results: &mut Vec<RuleMatchResult<V>>) {
        if results.len() < 2 {
            return;
        }
        let mode = self.shared.pragma_settings.read().rule_fire_mode;
        if mode != crate::backend::environment::core::RuleFireMode::Specificity {
            return;
        }
        let max_spec = results
            .iter()
            .map(|r| r.entry_specificity)
            .max()
            .expect("non-empty");
        results.retain(|r| r.entry_specificity == max_spec);
    }

    pub fn match_rules_native(
        &self,
        expr: &V,
        apply_bindings: impl Fn(&V, &GenericBindings<V>, &F) -> V,
        outer_carrying: &GenericBindings<V>,
    ) -> Vec<RuleMatchResult<V>> {
        // Plan Phase F (2026-05-20): the corelib MettaMod chain has been
        // deleted. All built-in helpers (if-decons-expr, if-error,
        // return-on-error, assertIncludes, noreduce-eq) are now dispatched
        // natively at the `'special_forms` arm in `eval/step/sexpr.rs`
        // before rule lookup; built-in type declarations
        // (ErrorDescription, BadType, BadArgType,
        // IncorrectNumberOfArguments) are registered via
        // `MettaEnvironment::register_corelib_types()` invoked at
        // `new_env()` time. No MeTTa source file is involved.
        let mut results = self.match_rules_native_inner(expr, &apply_bindings, outer_carrying);
        self.apply_rule_fire_mode_filter(&mut results);
        results
    }

    /// Inner implementation of `match_rules_native` (post-filter is in the
    /// thin wrapper above).
    fn match_rules_native_inner(
        &self,
        expr: &V,
        apply_bindings: impl Fn(&V, &GenericBindings<V>, &F) -> V,
        outer_carrying: &GenericBindings<V>,
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
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(head, arity as u8);
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
            // Uses get_candidates_filtered when head is known, which applies disc tree
            // pruning to group entries while always including wildcard rules.
            let candidates: SmallVec<[&RuleEntry<V>; 16]> = if !head.is_empty() {
                rule_index.get_candidates_filtered(head, arity, first_arg_head, expr)
            } else {
                rule_index.get_all_rules().collect()
            };

            if candidates.is_empty() {
                return Vec::new();
            }

            // I-12: Filter out dead rules via CompressedRuleFilter (from AAM analysis).
            // The filter is installed on the thread-local by `install_rule_filter()`.
            let candidates: SmallVec<[&RuleEntry<V>; 16]> = candidates
                .into_iter()
                .filter(|e| {
                    crate::backend::eval::cesk::continuation_compression::is_rule_live(
                        e.global_rule_index,
                    )
                })
                .collect();

            if candidates.is_empty() {
                return Vec::new();
            }

            // Trace: RuleLookup — emit candidate pipeline statistics
            #[cfg(feature = "trace")]
            if !head.is_empty() && crate::backend::trace::rule_match::should_trace_lookup(head) {
                let group_size = rule_index.group_size(head, arity) as u32;
                let wildcard_count = rule_index.wildcard_len() as u32;
                let candidates_count = candidates.len() as u32;
                crate::backend::trace::with_trace_collector_ref(|tc| {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        0,
                        crate::backend::trace::convert::trace_value_generic(expr),
                        vec![],
                        None,
                        trace_format::TraceEventKind::RuleLookup {
                            head: head.to_string(),
                            arity: arity as u32,
                            first_arg_head: first_arg_head.map(|s| s.to_string()),
                            group_size,
                            wildcard_count,
                            candidates_after_disc_tree: candidates_count,
                            candidates_after_dead_filter: candidates_count,
                            final_match_count: 0, // filled later
                            bloom_filter_reject: false,
                            self_evaluating: false,
                        },
                    );
                });
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
                    items
                        .iter()
                        .skip(1)
                        .map(|item| item.get_head_symbol())
                        .collect()
                } else {
                    SmallVec::new()
                };
                crate::backend::eval::cesk::with_adaptive_registry(|r| {
                    r.record_query(head_hash, arity, &arg_heads);
                });

                // I-13: On space mutation, only subgoals that consulted the affected group
                // are selectively invalidated (instead of blanket invalidation).
                let expr_hash = expr.hash_value();
                // Intern head as &'static str for the dependency record.
                // Atom strings from MeTTa values are already slab-allocated ('static),
                // but get_head_symbol() returns &str; re-intern is O(1) for existing strings.
                let head_static: &'static str =
                    crate::backend::models::gc_allocator::global_allocator().alloc_str(head);
                crate::backend::eval::cesk::with_incremental_index(|idx| {
                    idx.record_dependency(
                        crate::backend::eval::cesk::rete_incremental::SubgoalDependency {
                            subgoal_hash: expr_hash,
                            consulted_groups: smallvec::smallvec![(head_static, arity)],
                            matched_rules: smallvec::SmallVec::new(), // Populated after matching
                            transitive_deps: smallvec::SmallVec::new(),
                        },
                    );
                });
            }

            // Check if ALL candidates have structural matchers — over THIS expr's
            // first_arg_head/disc-tree-FILTERED match set, for the matching fast
            // path below.
            let all_structural = candidates.iter().all(|e| e.structural_matcher.is_some());

            // Phase E: populate the operator inline cache with per-(head,arity)
            // metadata. Finding 2 (formal/rocq/gc/AtomDedupMemoSoundness.v —
            // ConsumerEquivalence/ConflictingConsumers): this entry is read by
            // consumers whose MISS path computes the UNFILTERED per-(head,arity)
            // value (the pre-eval gate's miss scans get_candidates(None) via
            // RuleIndex::any_rule_wants_lazy_args / any_rule_lhs_head_all_meta_typed).
            // The proof requires the cached value to EQUAL each consumer's
            // miss-path value, so these flags are computed over the UNFILTERED
            // candidate set — NOT the FILTERED `candidates` above (this expr's
            // match set). Caching the FILTERED (expr-specific) metadata under a
            // (head,arity) key was a residual atom interning exposed (a
            // content-stable head pointer made the first expr's filtered metadata
            // stick for every later expr with that head, so a cache HIT differed
            // from a MISS). Unfiltered counts only make the dispatch fast-path
            // gates more CONSERVATIVE, never less correct.
            if !head.is_empty() {
                let meta: SmallVec<[&RuleEntry<V>; 16]> =
                    rule_index.get_candidates(head, arity, None).collect();
                let current_epoch = RULE_EPOCH.load(Ordering::Acquire);
                crate::backend::eval::trampoline::dispatch_hints::operator_cache_put(
                    head,
                    arity,
                    crate::backend::eval::trampoline::dispatch_hints::OperatorCacheEntry {
                        rule_epoch: current_epoch,
                        all_structural: meta.iter().all(|e| e.structural_matcher.is_some()),
                        candidate_count: meta.len(),
                        any_rule_wants_lazy_args: meta.iter().any(|e| e.body_wants_lazy_args),
                        lhs_head_all_meta_typed: meta.iter().any(|e| e.lhs_head_all_meta_typed),
                        any_rule_body_contains_cut: meta.iter().any(|e| e.body_contains_cut),
                    },
                );
            }

            if all_structural {
                // I-10: Parallel speculative matching for large candidate sets
                if crate::backend::eval::cesk::should_speculate(candidates.len())
                    && std::any::TypeId::of::<V>()
                        == std::any::TypeId::of::<crate::backend::models::MettaValue>()
                {
                    // I-10: Parallel speculative matching — head matching is pure read-only.
                    // Only for MettaValue (GcFactory is Send+Sync).
                    let chunks = crate::backend::eval::cesk::chunk_candidates(
                        candidates.len(),
                        std::thread::available_parallelism()
                            .map(|n| n.get())
                            .unwrap_or(4)
                            .min(candidates.len()),
                    );

                    // SAFETY: V is MettaValue (TypeId checked above). GcFactory is Send+Sync.
                    // We need to transmute the factory to a concrete Send+Sync type for
                    // std::thread::scope since the generic F doesn't promise Send.
                    let factory_ptr =
                        &self.factory as *const F as *const crate::backend::models::GcFactory;
                    let gc_factory: crate::backend::models::GcFactory = unsafe { *factory_ptr };
                    // Collect (rule_idx, result) pairs so we can canonicalize order
                    // after thread join. Chunks partition candidate range, so within
                    // a chunk rule_idx is monotonic; across chunks, thread-scheduling
                    // could reorder if we extended naively. Explicit post-sort makes
                    // the output stable across runs.
                    let mut all_indexed: Vec<(usize, RuleMatchResult<V>)> = Vec::new();

                    std::thread::scope(|s| {
                        let handles: Vec<_> = chunks.iter().map(|&(start, end)| {
                            let chunk_offset = start;
                            let chunk = &candidates[start..end];
                            let fac = gc_factory;
                            s.spawn(move || {
                                let mut chunk_results: Vec<(usize, RuleMatchResult<V>)> = Vec::new();
                                for (i, entry) in chunk.iter().enumerate() {
                                    let rule_idx = chunk_offset + i;
                                    let _rule_idx_u32 = rule_idx as u32;
                                    let bindings = if let Some(ref m) = entry.structural_matcher {
                                        m.try_match(expr)
                                    } else if let Some(ref m) = entry.enhanced_matcher {
                                        m.try_match(expr)
                                    } else {
                                        None
                                    };

                                    // eval-trace: emit a RuleMatchAttempt event
                                    // for this candidate (parallel speculative
                                    // path). Cheap when the filter is disabled.
                                    #[cfg(feature = "trace")]
                                    if crate::backend::trace::rule_match::should_trace_match(head) {
                                        let outcome = if let Some(ref b) = bindings {
                                            trace_format::RuleMatchOutcome::Success {
                                                bindings: b.iter()
                                                    .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                                                    .collect(),
                                            }
                                        } else {
                                            trace_format::RuleMatchOutcome::PathNavigateFailed {
                                                path: Vec::new(),
                                                var: None,
                                            }
                                        };
                                        crate::backend::trace::rule_match::emit_outcome::<V>(
                                            "structural-parallel",
                                            head,
                                            arity as u32,
                                            expr,
                                            &entry.lhs,
                                            None,
                                            _rule_idx_u32,
                                            outcome,
                                            None,
                                            0,
                                        );
                                    }
                                    if let Some(bindings) = bindings {
                                        // P2 + Option D: HE-style stored-side rename then scope tag.
                                        use crate::backend::eval::freshening::{
                                            allocate_epoch, freshen_bindings_keys_with_epoch,
                                            freshen_variables_with_epoch, intern_fresh_name,
                                        };
                                        use crate::backend::models::generic_bindings::{
                                            allocate_scope_id, ROOT_SCOPE,
                                        };
                                        let body_local_epoch = allocate_epoch();
                                        let dispatch_scope = allocate_scope_id();
                                        let prefix = format!("$__fr_{}_", body_local_epoch);
                                        // Option A: snapshot pre-freshen bindings for the
                                        // bytecode-VM frame, whose `compiled_rhs` opcodes
                                        // reference rule-LHS ORIGINAL names.
                                        let original_bindings = bindings.clone();
                                        let bindings = freshen_bindings_keys_with_epoch(
                                            bindings,
                                            body_local_epoch,
                                            &entry.var_names,
                                        );
                                        let renamed_var_names: Vec<&'static str> = entry
                                            .var_names
                                            .iter()
                                            .map(|n| intern_fresh_name(body_local_epoch, &n[1..]))
                                            .collect();
                                        let scoped_bindings = crate::backend::eval::bindings::retag_rule_keys_at_scope(
                                            bindings,
                                            &renamed_var_names,
                                            ROOT_SCOPE,
                                            dispatch_scope,
                                        );
                                        let scoped_bindings_mv: &crate::backend::models::GenericBindings<crate::backend::models::MettaValue> =
                                            unsafe { &*(&scoped_bindings as *const _ as *const crate::backend::models::GenericBindings<crate::backend::models::MettaValue>) };
                                        let expr_mv: &crate::backend::models::MettaValue =
                                            unsafe { &*(expr as *const V as *const crate::backend::models::MettaValue) };
                                        let Some(exported_bindings_mv) =
                                            crate::backend::eval::bindings::export_query_bindings_generic(
                                                scoped_bindings_mv,
                                                expr_mv,
                                                &prefix,
                                                &[dispatch_scope, ROOT_SCOPE],
                                                &fac,
                                            )
                                        else {
                                            continue;
                                        };
                                        let exported_bindings: GenericBindings<V> =
                                            unsafe { move_metta_bindings_to_v_unchecked(exported_bindings_mv) };
                                        let instantiated_rhs = if entry.rhs_has_variables {
                                            // SAFETY: V is MettaValue, fac is GcFactory (TypeId checked at outer scope).
                                            let rhs_mv: &crate::backend::models::MettaValue =
                                                unsafe { &*(&entry.rhs as *const V as *const crate::backend::models::MettaValue) };
                                            let rhs_freshened_mv = freshen_variables_with_epoch(
                                                rhs_mv,
                                                body_local_epoch,
                                                &fac,
                                            );
                                            let b_ref: &crate::backend::models::GenericBindings<crate::backend::models::MettaValue> =
                                                unsafe { &*(&scoped_bindings as *const _ as *const crate::backend::models::GenericBindings<crate::backend::models::MettaValue>) };
                                            // Phase 5 (Bug 1): caller-side outer_carrying threaded in.
                                            // SAFETY: V is MettaValue (TypeId checked at outer scope).
                                            let outer_ref: &crate::backend::models::GenericBindings<crate::backend::models::MettaValue> =
                                                unsafe { &*(outer_carrying as *const _ as *const crate::backend::models::GenericBindings<crate::backend::models::MettaValue>) };
                                            // PT-canonical: lazy substitution when rule is all-meta-typed (2026-05-21).
                                            let result = if entry.lhs_head_all_meta_typed {
                                                crate::backend::eval::bindings::apply_bindings_with_rename_scoped_lazy(
                                                    &rhs_freshened_mv,
                                                    b_ref,
                                                    &[dispatch_scope, ROOT_SCOPE],
                                                    dispatch_scope,
                                                    outer_ref,
                                                    &fac,
                                                )
                                            } else {
                                                crate::backend::eval::bindings::apply_bindings_with_rename_scoped(
                                                    &rhs_freshened_mv,
                                                    b_ref,
                                                    &[dispatch_scope, ROOT_SCOPE],
                                                    dispatch_scope,
                                                    outer_ref,
                                                    &fac,
                                                )
                                            };
                                            // SAFETY: MettaValue and V are the same type
                                            unsafe { std::mem::transmute_copy::<crate::backend::models::MettaValue, V>(&result) }
                                        } else {
                                            entry.rhs.clone()
                                        };
                                        let multiplicity = entry.multiplicity.max(1);
                                        for _ in 0..multiplicity {
                                            chunk_results.push((rule_idx, RuleMatchResult {
                                                instantiated_rhs: instantiated_rhs.clone(),
                                                rhs_template: entry.rhs.clone(),
                                                bindings: exported_bindings.clone(),
                                                original_bindings: original_bindings.clone(),
                                                rule_scope: dispatch_scope,
                                                multiplicity,
                                                rhs_type: entry.rhs_type.clone(),
                                                rhs_has_variables: entry.rhs_has_variables,
                                                compiled_rhs: entry.compiled_rhs.clone(),
                                                entry_specificity: entry.specificity,
                                            }));
                                        }
                                    }
                                }
                                chunk_results
                            })
                        }).collect();

                        for handle in handles {
                            all_indexed
                                .extend(handle.join().expect("speculative match thread panicked"));
                        }
                    });

                    // Canonical order: sort by rule_idx so output is stable
                    // regardless of thread scheduling. Stable sort preserves the
                    // multiplicity-duplicate ordering (all copies of the same
                    // rule_idx remain contiguous in insertion order).
                    all_indexed.sort_by_key(|(idx, _)| *idx);
                    let all_results: Vec<RuleMatchResult<V>> =
                        all_indexed.into_iter().map(|(_, r)| r).collect();

                    return all_results;
                }

                // Sequential fast path: all candidates have structural matchers — bypass MORK
                let mut results: Vec<RuleMatchResult<V>> = Vec::new();

                // I-3: Use binding arena for O(1) rollback on failed matches
                let mut arena = crate::backend::eval::cesk::BindingArena::<V>::new();
                arena.push_frame();

                for (rule_idx, entry) in candidates.iter().enumerate() {
                    // I-3: Save choice point before each attempt
                    arena.save_choice_point();
                    let _rule_idx_u32 = rule_idx as u32;

                    // I-2: Try enhanced matcher if structural matcher is None
                    let bindings = if let Some(ref matcher) = entry.structural_matcher {
                        // eval-trace: when METTA_TRACE_RULE_MATCH_HEADS admits
                        // this call_head, use try_match_with_detail and emit a
                        // RuleMatchAttempt event explaining success or failure.
                        // Hot path (filter disabled) is unchanged: a single
                        // atomic load + None check before falling through to
                        // the original try_match invocation.
                        #[cfg(feature = "trace")]
                        if crate::backend::trace::rule_match::should_trace_match(head) {
                            match matcher.try_match_with_detail(expr) {
                                Ok(b) => {
                                    crate::backend::trace::rule_match::emit_outcome::<V>(
                                        "structural",
                                        head,
                                        arity as u32,
                                        expr,
                                        &entry.lhs,
                                        None,
                                        _rule_idx_u32,
                                        trace_format::RuleMatchOutcome::Success {
                                            bindings: b
                                                .iter()
                                                .map(|(k, v)| {
                                                    (
                                                        k.to_string(),
                                                        crate::backend::trace::trace_value_generic(
                                                            v,
                                                        ),
                                                    )
                                                })
                                                .collect(),
                                        },
                                        None,
                                        0,
                                    );
                                    Some(b)
                                }
                                Err(detail) => {
                                    let outcome = crate::backend::trace::rule_match::detailed_failure_to_outcome(detail);
                                    crate::backend::trace::rule_match::emit_outcome::<V>(
                                        "structural",
                                        head,
                                        arity as u32,
                                        expr,
                                        &entry.lhs,
                                        None,
                                        _rule_idx_u32,
                                        outcome,
                                        None,
                                        0,
                                    );
                                    None
                                }
                            }
                        } else {
                            matcher.try_match(expr)
                        }
                        #[cfg(not(feature = "trace"))]
                        {
                            matcher.try_match(expr)
                        }
                    } else if let Some(ref matcher) = entry.enhanced_matcher {
                        matcher.try_match(expr)
                    } else {
                        None
                    };

                    if let Some(bindings) = bindings {
                        // Match succeeded — commit choice point
                        arena.commit_choice_point();

                        // Phase 3.2-A: per-invocation freshening.
                        //
                        // We rename the RHS template's variables under a
                        // per-match `epoch` so sibling nondet branches of the
                        // same rule can't have their RHS sub-evaluations share
                        // `$__fr_N_*` keys (which would collide inside
                        // `compose_outer_inner_generic` chains and produce
                        // ghost `collapse-bind` pairs).
                        //
                        // The `bindings` themselves keep the ORIGINAL rule
                        // variable names (post-load-freshening, `$__fr_N_*`).
                        // The rule's pre-compiled bytecode (in
                        // `compiled_rhs`) references those same original
                        // names via `PushVariable` opcodes, so keeping the
                        // bindings' keys stable is REQUIRED for the
                        // bytecode VM path. For cross-branch isolation of
                        // rule-body variables (e.g. a `chain`'s bind
                        // target), the RHS freshening is what matters —
                        // `instantiated_rhs` below substitutes bindings
                        // into the freshened RHS, leaving freshened body-
                        // only vars (epoch-unique) for downstream.
                        //
                        // HE parity: mirrors `CachingMapper` per-query in
                        // `hyperon-space/src/index/trie.rs`.
                        // P2 + Option D: HE-style stored-side rename then scope tag.
                        //
                        // The matcher emits `bindings` keyed on rule's ORIGINAL
                        // LHS names (`entry.var_names`). When rule-LHS and
                        // caller-query share a name (e.g. both have `$who`),
                        // the matcher emits a SELF-REFERENTIAL alias
                        // `$who → atom("$who")` that scope tags alone cannot
                        // disambiguate (atom values lack scope). HE solves this
                        // by alpha-renaming the STORED rule's variables before
                        // matching (`CachingMapper` at hyperon-space trie.rs).
                        // We mirror that with `freshen_bindings_keys_with_epoch`
                        // which renames rule-LHS keys to `$__fr_E_*` so caller
                        // and rule-side names are textually distinct.
                        //
                        // Then `retag_rule_keys_at_scope` over the FRESHENED
                        // names attaches the dispatch scope. Both mechanisms
                        // co-exist: name-distinct + scope-distinct = robust.
                        use crate::backend::eval::freshening::{
                            allocate_epoch, freshen_bindings_keys_with_epoch,
                            freshen_variables_with_epoch, intern_fresh_name,
                        };
                        use crate::backend::models::generic_bindings::{
                            allocate_scope_id, ROOT_SCOPE,
                        };
                        let body_local_epoch = allocate_epoch();
                        let dispatch_scope = allocate_scope_id();
                        let prefix = format!("$__fr_{}_", body_local_epoch);
                        // Phase 11.A (2026-05-17) — gate the per-binding
                        // transitive substitution by whether any binding
                        // value actually contains a free variable that
                        // is also a key in `bindings` (a "cross-binding"
                        // reference, which can arise from bidirectional
                        // unify via repeated-var rules; the structural
                        // matcher itself never produces them).
                        //
                        // Background: commit `b359684` (P3) added an
                        // unconditional loop here. For PLN's
                        // `BestCandidate` / `LimitSize` / etc. recursions
                        // over 100-element lists with `$tuple` bound to
                        // the giant list tail, this is O(|bindings| ×
                        // |value_tree|) per match — the dominant
                        // component of the 60× Robot.metta regression.
                        //
                        // The gate restores the original cost (O(|bindings|)
                        // for the test) only when actually needed. For
                        // PLN's bindings (typically 3 keys, each bound to
                        // a ground value with no free variables), the
                        // gate trivially returns `false` and the loop
                        // does not run.
                        //
                        // Correctness: the gate uses
                        // `value_contains_any_key` (defined below) which
                        // walks the value tree once looking for any atom
                        // whose name matches a binding key. If false, the
                        // values are independent and pre-resolution is a
                        // no-op; if true, we run the loop as before. This
                        // is a STRICT optimization of identical behavior.
                        let needs_transitive_resolution = {
                            let keys: SmallVec<[&str; 8]> =
                                bindings.iter().map(|(k, _)| k).collect();
                            bindings
                                .iter()
                                .any(|(_, v)| value_contains_any_key(v, &keys))
                        };
                        let original_bindings = if needs_transitive_resolution {
                            let mut resolved = crate::backend::models::GenericBindings::new();
                            for (name, value) in bindings.iter() {
                                // Self-exclusion guard (2026-05-26): if a value
                                // textually contains its OWN key, the binding is
                                // self-referential by NAME — a rule-LHS variable
                                // (e.g. `$z` in `(= (id $z) $z)`) collided with a
                                // same-named FREE variable inside the matched
                                // argument (`(id (wrap $z))` binds `$z → (wrap $z)`).
                                // Resolving such a value against the FULL map makes
                                // `apply_bindings` substitute the inner `$z → (wrap $z)`
                                // transitively WITHOUT BOUND → stack overflow
                                // (matchnested2.metta; minimal repro `(= (id $z) $z)`
                                // + `!(id (wrap $z))`). The inner occurrence is the
                                // CALLER's distinct variable and must stay free here;
                                // the key-freshening below (`$z` → `$__fr_E_z`, caller
                                // `$z` unchanged) then disambiguates them. Excluding
                                // the self key is a STRICT no-op for every
                                // NON-self-referential binding — the legitimate
                                // transitive case `{$B → (Inheritance $1 …), $1 → Anna}`
                                // is unchanged (`$B`'s value does not contain `$B`), so
                                // PLN's resolution and all normal dispatch are
                                // unaffected.
                                let r = if value_contains_any_key(value, &[name]) {
                                    let mut filtered =
                                        crate::backend::models::GenericBindings::new();
                                    for (k, v) in bindings.iter() {
                                        if k != name {
                                            filtered.insert(k, v.clone());
                                        }
                                    }
                                    crate::backend::eval::bindings::apply_bindings_generic(
                                        value,
                                        &filtered,
                                        &self.factory,
                                    )
                                } else {
                                    crate::backend::eval::bindings::apply_bindings_generic(
                                        value,
                                        &bindings,
                                        &self.factory,
                                    )
                                };
                                resolved.insert(name, r);
                            }
                            resolved
                        } else {
                            bindings.clone()
                        };
                        let bindings = freshen_bindings_keys_with_epoch(
                            bindings,
                            body_local_epoch,
                            &entry.var_names,
                        );
                        let renamed_var_names: Vec<&'static str> = entry
                            .var_names
                            .iter()
                            .map(|n| intern_fresh_name(body_local_epoch, &n[1..]))
                            .collect();
                        let scoped_bindings =
                            crate::backend::eval::bindings::retag_rule_keys_at_scope(
                                bindings,
                                &renamed_var_names,
                                ROOT_SCOPE,
                                dispatch_scope,
                            );
                        let Some(exported_bindings) = export_rule_match_bindings(
                            &scoped_bindings,
                            expr,
                            &prefix,
                            dispatch_scope,
                            &self.factory,
                        ) else {
                            continue;
                        };
                        // Phase 3.2-B trace event: BindingsExtracted as before
                        // (legacy `iter()` shim drops scope; the trace event's
                        // contract is bare-name pairs).
                        #[cfg(feature = "trace")]
                        {
                            let bindings_tv: Vec<(String, trace_format::TraceValue)> =
                                scoped_bindings
                                    .iter()
                                    .map(|(k, v)| {
                                        (
                                            k.to_string(),
                                            crate::backend::trace::trace_value_generic(v),
                                        )
                                    })
                                    .collect();
                            let var_names_tv: Vec<String> =
                                entry.var_names.iter().map(|s| s.to_string()).collect();
                            crate::backend::trace::thread_local_sink::with_trace_collector_ref(
                                |tc| {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        0,
                                        crate::backend::trace::trace_value_generic(expr),
                                        vec![],
                                        None,
                                        trace_format::TraceEventKind::BindingsExtracted {
                                            source: "structural".to_string(),
                                            head: head.to_string(),
                                            arity: arity as u32,
                                            bindings: bindings_tv,
                                            var_names: var_names_tv,
                                        },
                                    );
                                },
                            );
                        }
                        let instantiated_rhs = if entry.rhs_has_variables {
                            // Option D: freshen RHS template too so its var
                            // names align with the renamed binding keys above.
                            // (Caller-side names embedded via bidirectional
                            // unify aliases stay untouched — they're not in
                            // `entry.var_names`, so freshen_variables_with_epoch's
                            // walk DOES rewrite them; that's a problem only if
                            // they appear in the RHS, which is rare. The
                            // bisimulation argument: caller-side names appear
                            // in the substituted RHS only when the matcher
                            // inserted a `(rule_var → atom("$caller_var"))`
                            // alias; with the renamed key, the alias becomes
                            // `($__fr_E_rule_var → atom("$caller_var"))` and
                            // the freshened RHS lookup of `$__fr_E_rule_var`
                            // resolves through that alias to the caller atom.)
                            let rhs_freshened = freshen_variables_with_epoch(
                                &entry.rhs,
                                body_local_epoch,
                                &self.factory,
                            );
                            // Phase 5 (Bug 1): caller-side outer_carrying threaded in.
                            // PT-canonical lazy mode (2026-05-21): wrap substituted
                            // vars in `Lazy(...)` when rule's LHS head is
                            // all-meta-typed.
                            if entry.lhs_head_all_meta_typed {
                                crate::backend::eval::bindings::apply_bindings_with_rename_scoped_lazy(
                                    &rhs_freshened,
                                    &scoped_bindings,
                                    &[dispatch_scope, ROOT_SCOPE],
                                    dispatch_scope,
                                    outer_carrying,
                                    &self.factory,
                                )
                            } else {
                                crate::backend::eval::bindings::apply_bindings_with_rename_scoped(
                                    &rhs_freshened,
                                    &scoped_bindings,
                                    &[dispatch_scope, ROOT_SCOPE],
                                    dispatch_scope,
                                    outer_carrying,
                                    &self.factory,
                                )
                            }
                        } else {
                            entry.rhs.clone()
                        };
                        let multiplicity = entry.multiplicity.max(1);
                        if multiplicity == 1 {
                            results.push(RuleMatchResult {
                                instantiated_rhs,
                                rhs_template: entry.rhs.clone(),
                                bindings: exported_bindings,
                                original_bindings,
                                rule_scope: dispatch_scope,
                                multiplicity: 1,
                                rhs_type: entry.rhs_type.clone(),
                                rhs_has_variables: entry.rhs_has_variables,
                                compiled_rhs: entry.compiled_rhs.clone(),
                                entry_specificity: entry.specificity,
                            });
                        } else {
                            for _ in 0..multiplicity {
                                results.push(RuleMatchResult {
                                    instantiated_rhs: instantiated_rhs.clone(),
                                    rhs_template: entry.rhs.clone(),
                                    bindings: exported_bindings.clone(),
                                    original_bindings: original_bindings.clone(),
                                    rule_scope: dispatch_scope,
                                    multiplicity,
                                    rhs_type: entry.rhs_type.clone(),
                                    rhs_has_variables: entry.rhs_has_variables,
                                    compiled_rhs: entry.compiled_rhs.clone(),
                                    entry_specificity: entry.specificity,
                                });
                            }
                        }
                    } else {
                        // I-3: Match failed — restore choice point (O(1) rollback)
                        arena.restore_choice_point();
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

                for (rule_idx, entry) in candidates.iter().enumerate() {
                    let _rule_idx_u32 = rule_idx as u32;
                    // Try structural matcher first (works regardless of MORK encoding)
                    if let Some(ref matcher) = entry.structural_matcher {
                        let try_result = matcher.try_match(expr);
                        // eval-trace: emit RuleMatchAttempt for the MORK fallback's
                        // structural-matcher pre-check (the path taken when expr
                        // can't be encoded as MORK bytes).
                        #[cfg(feature = "trace")]
                        if crate::backend::trace::rule_match::should_trace_match(head) {
                            let outcome = if let Some(ref b) = try_result {
                                trace_format::RuleMatchOutcome::Success {
                                    bindings: b.iter()
                                        .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                                        .collect(),
                                }
                            } else {
                                trace_format::RuleMatchOutcome::PathNavigateFailed {
                                    path: Vec::new(),
                                    var: None,
                                }
                            };
                            crate::backend::trace::rule_match::emit_outcome::<V>(
                                "structural-mork-fallback",
                                head,
                                arity as u32,
                                expr,
                                &entry.lhs,
                                None,
                                _rule_idx_u32,
                                outcome,
                                None,
                                0,
                            );
                        }
                        if let Some(bindings) = try_result {
                            // P2 + Option D: HE-style stored-side rename then scope tag.
                            use crate::backend::eval::freshening::{
                                allocate_epoch, freshen_bindings_keys_with_epoch,
                                freshen_variables_with_epoch, intern_fresh_name,
                            };
                            use crate::backend::models::generic_bindings::{
                                allocate_scope_id, ROOT_SCOPE,
                            };
                            let body_local_epoch = allocate_epoch();
                            let dispatch_scope = allocate_scope_id();
                            let prefix = format!("$__fr_{}_", body_local_epoch);
                            // Option A: snapshot pre-freshen bindings for VM frame.
                            let original_bindings = bindings.clone();
                            let bindings = freshen_bindings_keys_with_epoch(
                                bindings,
                                body_local_epoch,
                                &entry.var_names,
                            );
                            let renamed_var_names: Vec<&'static str> = entry
                                .var_names
                                .iter()
                                .map(|n| intern_fresh_name(body_local_epoch, &n[1..]))
                                .collect();
                            let scoped_bindings = crate::backend::eval::bindings::retag_rule_keys_at_scope(
                                bindings,
                                &renamed_var_names,
                                ROOT_SCOPE,
                                dispatch_scope,
                            );
                            let Some(exported_bindings) = export_rule_match_bindings(
                                &scoped_bindings,
                                expr,
                                &prefix,
                                dispatch_scope,
                                &self.factory,
                            ) else {
                                continue;
                            };
                            let instantiated_rhs = if entry.rhs_has_variables {
                                let rhs_freshened = freshen_variables_with_epoch(
                                    &entry.rhs,
                                    body_local_epoch,
                                    &self.factory,
                                );
                                // Phase 1 (Bug 1): outer_carrying empty here; Phase 5 wires in.
                                let empty_outer = crate::backend::models::GenericBindings::<V>::new();
                                crate::backend::eval::bindings::apply_bindings_with_rename_scoped(
                                    &rhs_freshened,
                                    &scoped_bindings,
                                    &[dispatch_scope, ROOT_SCOPE],
                                    dispatch_scope,
                                    &empty_outer,
                                    &self.factory,
                                )
                            } else {
                                entry.rhs.clone()
                            };
                            let multiplicity = entry.multiplicity.max(1);
                            if multiplicity == 1 {
                                results.push(RuleMatchResult {
                                    instantiated_rhs,
                                    rhs_template: entry.rhs.clone(),
                                    bindings: exported_bindings,
                                    original_bindings,
                                    rule_scope: dispatch_scope,
                                    multiplicity: 1,
                                    rhs_type: entry.rhs_type.clone(),
                                    rhs_has_variables: entry.rhs_has_variables,
                                    compiled_rhs: entry.compiled_rhs.clone(),
                                    entry_specificity: entry.specificity,
                                });
                            } else {
                                for _ in 0..multiplicity {
                                    results.push(RuleMatchResult {
                                        instantiated_rhs: instantiated_rhs.clone(),
                                        rhs_template: entry.rhs.clone(),
                                        bindings: exported_bindings.clone(),
                                        original_bindings: original_bindings.clone(),
                                        rule_scope: dispatch_scope,
                                        multiplicity,
                                        rhs_type: entry.rhs_type.clone(),
                                        rhs_has_variables: entry.rhs_has_variables,
                                        compiled_rhs: entry.compiled_rhs.clone(),
                                        entry_specificity: entry.specificity,
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
                        // Structural pattern match fallback for MORK-only candidates.
                        // Use the _with_factory variant so dotted-pair patterns
                        // `($x . $rest)` (rejected by StructuralMatcher) can
                        // bind the rest-var to an SExpr of remaining elements.
                        crate::backend::eval::bindings::pattern_match_generic_with_factory(
                            &entry.lhs, expr, &self.factory,
                        )
                    };

                    if let Some(bindings) = matched_bindings {
                        // P2 + Option D: HE-style stored-side rename then scope tag.
                        use crate::backend::eval::freshening::{
                            allocate_epoch, freshen_bindings_keys_with_epoch,
                            freshen_variables_with_epoch, intern_fresh_name,
                        };
                        use crate::backend::models::generic_bindings::{
                            allocate_scope_id, ROOT_SCOPE,
                        };
                        let body_local_epoch = allocate_epoch();
                        let dispatch_scope = allocate_scope_id();
                        let prefix = format!("$__fr_{}_", body_local_epoch);
                        // Option A: snapshot pre-freshen bindings for VM frame.
                        let original_bindings = bindings.clone();
                        let bindings = freshen_bindings_keys_with_epoch(
                            bindings,
                            body_local_epoch,
                            &entry.var_names,
                        );
                        let renamed_var_names: Vec<&'static str> = entry
                            .var_names
                            .iter()
                            .map(|n| intern_fresh_name(body_local_epoch, &n[1..]))
                            .collect();
                        let scoped_bindings = crate::backend::eval::bindings::retag_rule_keys_at_scope(
                            bindings,
                            &renamed_var_names,
                            ROOT_SCOPE,
                            dispatch_scope,
                        );
                        let Some(exported_bindings) = export_rule_match_bindings(
                            &scoped_bindings,
                            expr,
                            &prefix,
                            dispatch_scope,
                            &self.factory,
                        ) else {
                            continue;
                        };
                        let instantiated_rhs = if entry.rhs_has_variables {
                            let rhs_freshened = freshen_variables_with_epoch(
                                &entry.rhs,
                                body_local_epoch,
                                &self.factory,
                            );
                            // Phase 5 (Bug 1): caller-side outer_carrying threaded in.
                            // PT-canonical lazy mode (2026-05-21): wrap substituted
                            // vars in `Lazy(...)` when rule's LHS head is
                            // all-meta-typed.
                            if entry.lhs_head_all_meta_typed {
                                crate::backend::eval::bindings::apply_bindings_with_rename_scoped_lazy(
                                    &rhs_freshened,
                                    &scoped_bindings,
                                    &[dispatch_scope, ROOT_SCOPE],
                                    dispatch_scope,
                                    outer_carrying,
                                    &self.factory,
                                )
                            } else {
                                crate::backend::eval::bindings::apply_bindings_with_rename_scoped(
                                    &rhs_freshened,
                                    &scoped_bindings,
                                    &[dispatch_scope, ROOT_SCOPE],
                                    dispatch_scope,
                                    outer_carrying,
                                    &self.factory,
                                )
                            }
                        } else {
                            entry.rhs.clone()
                        };
                        let multiplicity = entry.multiplicity.max(1);
                        if multiplicity == 1 {
                            results.push(RuleMatchResult {
                                instantiated_rhs,
                                rhs_template: entry.rhs.clone(),
                                bindings: exported_bindings,
                                original_bindings,
                                rule_scope: dispatch_scope,
                                multiplicity: 1,
                                rhs_type: entry.rhs_type.clone(),
                                rhs_has_variables: entry.rhs_has_variables,
                                compiled_rhs: entry.compiled_rhs.clone(),
                                entry_specificity: entry.specificity,
                            });
                        } else {
                            for _ in 0..multiplicity {
                                results.push(RuleMatchResult {
                                    instantiated_rhs: instantiated_rhs.clone(),
                                    rhs_template: entry.rhs.clone(),
                                    bindings: exported_bindings.clone(),
                                    original_bindings: original_bindings.clone(),
                                    rule_scope: dispatch_scope,
                                    multiplicity,
                                    rhs_type: entry.rhs_type.clone(),
                                    rhs_has_variables: entry.rhs_has_variables,
                                    compiled_rhs: entry.compiled_rhs.clone(),
                                    entry_specificity: entry.specificity,
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
                ($entry:expr, $rule_idx:expr) => {
                    let entry = $entry;
                    let _rule_idx_u32 = $rule_idx as u32;

                    // Try structural matcher first (avoids MORK byte-level matching)
                    if let Some(ref matcher) = entry.structural_matcher {
                        let try_result = matcher.try_match(expr);
                        // eval-trace: emit RuleMatchAttempt for the MORK
                        // happy-path's structural-matcher pre-check.
                        #[cfg(feature = "trace")]
                        if crate::backend::trace::rule_match::should_trace_match(head) {
                            let outcome = if let Some(ref b) = try_result {
                                trace_format::RuleMatchOutcome::Success {
                                    bindings: b.iter()
                                        .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                                        .collect(),
                                }
                            } else {
                                trace_format::RuleMatchOutcome::PathNavigateFailed {
                                    path: Vec::new(),
                                    var: None,
                                }
                            };
                            crate::backend::trace::rule_match::emit_outcome::<V>(
                                "structural-mork-path",
                                head,
                                arity as u32,
                                expr,
                                &entry.lhs,
                                None,
                                _rule_idx_u32,
                                outcome,
                                None,
                                0,
                            );
                        }
                        if let Some(bindings) = try_result {
                            hits.push(MatchHit { entry, is_wide: false, precomputed_bindings: Some(bindings) });
                        }
                        // Structural matcher is authoritative — skip MORK
                    } else if entry.enhanced_matcher.is_none()
                        && crate::backend::eval::bindings::pattern_match_generic_with_factory(
                            &entry.lhs,
                            expr,
                            &self.factory,
                        )
                        .is_some()
                    {
                        // Dotted-pair / non-structural pattern fallback (2026-05-11).
                        // Patterns that StructuralMatcher and EnhancedMatcher both
                        // rejected — e.g. `($x . $rest)` — are routed through
                        // pattern_match_generic which understands cons-list head/tail
                        // binding. We re-compute bindings below via the standard
                        // extraction; here we just record the match for hits.
                        let bindings = crate::backend::eval::bindings::pattern_match_generic_with_factory(
                            &entry.lhs,
                            expr,
                            &self.factory,
                        );
                        if let Some(b) = bindings {
                            hits.push(MatchHit { entry, is_wide: false, precomputed_bindings: Some(b) });
                        }
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
                for (rule_idx, entry) in rule_index.get_candidates(head, arity, first_arg_head).enumerate() {
                    try_match_entry!(entry, rule_idx);
                }
            } else {
                for (rule_idx, entry) in rule_index.get_all_rules().enumerate() {
                    try_match_entry!(entry, rule_idx);
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

                // P2 + Option D: HE-style stored-side rename then scope tag.
                use crate::backend::eval::freshening::{
                    allocate_epoch, freshen_bindings_keys_with_epoch,
                    freshen_variables_with_epoch, intern_fresh_name,
                };
                use crate::backend::models::generic_bindings::{
                    allocate_scope_id, ROOT_SCOPE,
                };
                let body_local_epoch = allocate_epoch();
                let dispatch_scope = allocate_scope_id();
                let prefix = format!("$__fr_{}_", body_local_epoch);
                // Option A: snapshot pre-freshen bindings for VM frame.
                let original_bindings = bindings.clone();
                let bindings = freshen_bindings_keys_with_epoch(
                    bindings,
                    body_local_epoch,
                    &entry.var_names,
                );
                let renamed_var_names: Vec<&'static str> = entry
                    .var_names
                    .iter()
                    .map(|n| intern_fresh_name(body_local_epoch, &n[1..]))
                    .collect();
                let scoped_bindings = crate::backend::eval::bindings::retag_rule_keys_at_scope(
                    bindings,
                    &renamed_var_names,
                    ROOT_SCOPE,
                    dispatch_scope,
                );
                let Some(exported_bindings) = export_rule_match_bindings(
                    &scoped_bindings,
                    expr,
                    &prefix,
                    dispatch_scope,
                    &self.factory,
                ) else {
                    continue;
                };
                let instantiated_rhs = if entry.rhs_has_variables {
                    let rhs_freshened = freshen_variables_with_epoch(
                        &entry.rhs,
                        body_local_epoch,
                        &self.factory,
                    );
                    // Phase 1 (Bug 1): outer_carrying empty here; Phase 5 wires in.
                    let empty_outer = crate::backend::models::GenericBindings::<V>::new();
                    // PT-canonical lazy mode (2026-05-21).
                    if entry.lhs_head_all_meta_typed {
                        crate::backend::eval::bindings::apply_bindings_with_rename_scoped_lazy(
                            &rhs_freshened,
                            &scoped_bindings,
                            &[dispatch_scope, ROOT_SCOPE],
                            dispatch_scope,
                            &empty_outer,
                            &self.factory,
                        )
                    } else {
                        crate::backend::eval::bindings::apply_bindings_with_rename_scoped(
                            &rhs_freshened,
                            &scoped_bindings,
                            &[dispatch_scope, ROOT_SCOPE],
                            dispatch_scope,
                            &empty_outer,
                            &self.factory,
                        )
                    }
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
                        bindings: exported_bindings,
                        original_bindings,
                        rule_scope: dispatch_scope,
                        multiplicity: 1,
                        rhs_type: entry.rhs_type.clone(),
                        rhs_has_variables: entry.rhs_has_variables,
                        compiled_rhs: entry.compiled_rhs.clone(),
                        entry_specificity: entry.specificity,
                    });
                } else {
                    for _ in 0..multiplicity {
                        results.push(RuleMatchResult {
                            instantiated_rhs: instantiated_rhs.clone(),
                            rhs_template: entry.rhs.clone(),
                            bindings: exported_bindings.clone(),
                            original_bindings: original_bindings.clone(),
                            rule_scope: dispatch_scope,
                            multiplicity,
                            rhs_type: entry.rhs_type.clone(),
                            rhs_has_variables: entry.rhs_has_variables,
                            compiled_rhs: entry.compiled_rhs.clone(),
                            entry_specificity: entry.specificity,
                        });
                    }
                }
            }

            results
        })
    }

    /// Bidirectional-unify rule matcher — VM-tier fallback entry point.
    ///
    /// Invoked from `op_dispatch_rules` (`vm/mod.rs:5526`) when
    /// `match_rules_native` returns empty AND the query expression contains
    /// free variables. The structural matchers consumed by `match_rules_native`
    /// perform literal atom comparison; queries like `(father $who b)` against
    /// rule `(father a b)` fail there because `"a" != "$who"`. MeTTa HE
    /// resolves this via Prolog-style bidirectional unification — the query
    /// variable binds to the rule's atom and the RHS is produced as a result.
    ///
    /// **Why VM-only**: The trampoline tier already engages Step 3.5
    /// (`step/sexpr.rs:2442-2478`) which calls `enumerate_rules_via_unification`
    /// directly and preserves bindings end-to-end. The trampoline's
    /// `try_match_all_rules` (`engine.rs:543-547`) intentionally strips
    /// bindings via `Bindings::new()` for the cache contract, so calling this
    /// fallback from inside `match_rules_native` would feed binding-less
    /// matches to the trampoline and bypass Step 3.5. Keeping this entry
    /// point exclusively VM-side restores the per-tier separation.
    ///
    /// Mirrors the trampoline tier's
    /// [`crate::backend::eval::trampoline::engine::enumerate_rules_via_unification`].
    /// Returns an empty vec when no rules unify.
    ///
    /// **Type-erasure note**: `enumerate_rules_via_unification` is
    /// monomorphized to `MettaValue` / `GcFactory`. We TypeId-guard and
    /// transmute exactly as the parallel speculative path does. The only
    /// generic-V caller path uses `MettaValue` in practice.
    pub(crate) fn match_rules_via_unify(&self, expr: &V) -> Vec<RuleMatchResult<V>> {
        if std::any::TypeId::of::<V>()
            != std::any::TypeId::of::<crate::backend::models::MettaValue>()
        {
            return Vec::new();
        }
        // SAFETY: V == MettaValue (TypeId guard above) and, for the MettaValue
        // monomorphization, F is the active value factory `ActiveFactory`
        // (the slab `GcFactory` by default, `IndexFactory` under
        // `--features index-gc`). Reinterpreting `&F` as `*const ActiveFactory`
        // is therefore an identity cast.
        let factory_ptr = &self.factory as *const F as *const crate::backend::models::ActiveFactory;
        let gc_factory: crate::backend::models::ActiveFactory = unsafe { *factory_ptr };
        let expr_mv: &crate::backend::models::MettaValue =
            unsafe { &*(expr as *const V as *const crate::backend::models::MettaValue) };
        let env_ref: &crate::backend::eval::trampoline::engine::Environment = unsafe {
            &*(self as *const Self as *const crate::backend::eval::trampoline::engine::Environment)
        };

        let unified =
            crate::backend::eval::trampoline::engine::enumerate_rules_via_unification_detailed(
                expr_mv,
                env_ref,
                &gc_factory,
            );

        if unified.is_empty() {
            return Vec::new();
        }

        let mut results: Vec<RuleMatchResult<V>> = Vec::with_capacity(unified.len());
        for unified_match in unified {
            let instantiated_rhs_mv = unified_match.instantiated_rhs;
            let exported_bindings_mv = unified_match.exported_bindings;
            let original_bindings_mv = unified_match.original_bindings;
            let rule_scope = unified_match.rule_scope;
            let rhs_type_mv = unified_match.rhs_type;

            // SAFETY: V == MettaValue (TypeId checked at function entry).
            let instantiated_rhs: V = unsafe {
                std::mem::transmute_copy::<crate::backend::models::MettaValue, V>(
                    &instantiated_rhs_mv,
                )
            };
            let bindings: GenericBindings<V> =
                unsafe { move_metta_bindings_to_v_unchecked(exported_bindings_mv) };
            let original_bindings: GenericBindings<V> =
                unsafe { move_metta_bindings_to_v_unchecked(original_bindings_mv) };
            let rhs_type: Option<V> = rhs_type_mv.map(|t| unsafe {
                std::mem::transmute_copy::<crate::backend::models::MettaValue, V>(&t)
            });

            // The unify path produces a fully-instantiated RHS (all variables
            // substituted). Defaults: `rhs_template` reuses `instantiated_rhs`
            // (only consulted for compile-on-demand caching, which the unify
            // path bypasses); `rhs_has_variables = false`; `compiled_rhs = None`
            // forces the trampoline path at vm/mod.rs to evaluate
            // `instantiated_rhs` rather than re-execute compiled bytecode.
            results.push(RuleMatchResult {
                instantiated_rhs: instantiated_rhs.clone(),
                rhs_template: instantiated_rhs,
                bindings,
                original_bindings,
                rule_scope,
                multiplicity: 1,
                rhs_type,
                rhs_has_variables: false,
                compiled_rhs: None,
                // Unify-fallback path: entry reference isn't plumbed through;
                // default to 0 so the filter treats these as least-specific.
                // Rare path (only when structural matcher fails), so the
                // perf/expressiveness trade-off is negligible.
                entry_specificity: 0,
            });
        }
        // Apply the same SUPERSET specificity filter to the unify path so
        // tier behavior is consistent. With all entry_specificity==0, the
        // filter is a no-op (max==0 retains all), but if structural matches
        // also happen to feed in via mixed code paths the filter handles
        // them uniformly.
        self.apply_rule_fire_mode_filter(&mut results);
        results
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
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(head, arity as u8);
            if bloom_says_no && !self.shared.rule_index.read().has_wildcard_rules() {
                return Vec::new();
            }
        }

        let space = self.create_space();
        let rule_prefix_len = super::core::RULE_PREFIX_LEN;
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
                self.collect_rules_from_prefix(&space, &head_prefix, rule_prefix_len, &mut rules);
            }
        } else {
            // No head info — collect all rules under the rule prefix
            self.collect_rules_from_prefix(&space, &rule_prefix, rule_prefix_len, &mut rules);
        }

        // 2. Collect wildcard rules (LHS is atom/variable, not S-expression)
        self.collect_wildcard_rules(
            &space,
            &rule_prefix,
            rule_prefix_len,
            head,
            arity,
            &mut rules,
        );

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
            let full_rule =
                match mork_bytes_to_generic_value::<V, F, Multiplicity>(path, space, &self.factory)
                {
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

            let full_rule =
                match mork_bytes_to_generic_value::<V, F, Multiplicity>(path, space, &self.factory)
                {
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
            if !rule_head.is_empty() && (rule_head != query_head || lhs.get_arity() != query_arity)
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
        match with_mork_query_bytes(
            &rule_sexpr,
            &sm,
            self.mork_cache_epoch,
            |mork_bytes, _ctx| {
                let btm = self.shared.atom_space.btm.read();
                let count = get_multiplicity(&btm, mork_bytes);
                if count == 0 {
                    1
                } else {
                    count as usize
                }
            },
        ) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — check wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(
                    &rule_sexpr,
                    &mut wide_key,
                );
                let wbtm = self.shared.atom_space.wide_btm.read();
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                if count == 0 {
                    1
                } else {
                    count as usize
                }
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

        // Reset the override bitset — it will be rebuilt as rules are
        // re-inserted below. Without this, repeated rebuilds would
        // monotonically increase the per-name refcounts.
        self.shared.dispatch_overrides.reset();

        let space = self.create_space();
        let rule_prefix_len = super::core::RULE_PREFIX_LEN;

        for (path_bytes, multiplicity_val) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, rhs)) = extract_rule_parts(&value) {
                    // Update bloom filters + fuzzy matcher
                    let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
                    let arity = lhs.get_arity();
                    if let Some(ref head) = head_owned {
                        self.shared.fuzzy_matcher.write().insert(head);
                        self.shared
                            .atom_space
                            .head_arity_bloom
                            .write()
                            .insert(head, arity as u8);
                        self.shared
                            .atom_space
                            .rule_head_bloom
                            .write()
                            .insert(head, arity as u8);
                        // Re-bump override bitset (matches add_rule's behavior).
                        if let Some(id) = super::dispatch_overrides::overridable_op_id(head) {
                            self.shared.dispatch_overrides.note_user_rule_added(id);
                        }
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
                    let _ = with_mork_query_bytes(
                        &rule_sexpr,
                        &sm,
                        self.mork_cache_epoch,
                        |debruijn_bytes, ctx| {
                            // Split De Bruijn bytes to get LHS range
                            if debruijn_bytes.len() <= rule_prefix_len {
                                return;
                            }
                            let lhs_start = rule_prefix_len;
                            let lhs_byte_len = mork_expr_byte_len(&debruijn_bytes[lhs_start..]);
                            // Pad with 0x00 for ExprZipper read-past-end safety
                            let mut lhs_debruijn = Vec::with_capacity(lhs_byte_len + 1);
                            lhs_debruijn.extend_from_slice(
                                &debruijn_bytes[lhs_start..lhs_start + lhs_byte_len],
                            );
                            lhs_debruijn.push(0x00);

                            // Save full rule bytes for alpha-equivalent removal via remove_rule_by_debruijn.
                            let full_debruijn = debruijn_bytes.to_vec();

                            let lhs_var_count = count_newvar_tags(&lhs_debruijn);
                            let (var_names, wildcard_indices) =
                                build_var_names_and_wildcards(&ctx.var_names, lhs_var_count);

                            // Phase 8.1: Compute RHS type for branch pruning
                            let rhs_type = {
                                use crate::backend::eval::types::infer_type_generic;
                                let inferred = infer_type_generic(&rhs, &self.factory, self);
                                if inferred.as_atom() == Some("%Undefined%") {
                                    None
                                } else {
                                    Some(inferred)
                                }
                            };

                            // Phase 10.1: Register inferred return type (bulk path).
                            // PLN-fix 2026-04: same freshened-var gate as the
                            // single-rule path above.
                            if let Some(ref rt) = rhs_type {
                                if let Some(ref head) = head_owned {
                                    if !type_contains_freshened_var(rt) {
                                        self.register_inferred_type(head, rt);
                                    }
                                }
                            }

                            // Phase 10.4: Synthesize arrow type (bulk path).
                            if let Some(ref head) = head_owned {
                                let has_declared_arrow =
                                    self.get_types_generic(head).iter().any(|t| {
                                        t.as_sexpr().and_then(|items| {
                                            items.first().and_then(|v| v.as_atom())
                                        }) == Some("->")
                                    });
                                if !has_declared_arrow {
                                    use crate::backend::eval::types::infer_arrow_type_from_rule;
                                    if let Some(arrow) = infer_arrow_type_from_rule(
                                        &lhs,
                                        &rhs,
                                        rhs_type.as_ref(),
                                        &self.factory,
                                        self,
                                    ) {
                                        // PLN-fix 2026-04: same gate.
                                        if !type_contains_freshened_var(&arrow) {
                                            self.register_inferred_type(head, &arrow);
                                        }
                                    }
                                }
                            }

                            let structural_matcher = StructuralMatcher::analyze(&lhs);
                            let enhanced_matcher = if structural_matcher.is_none() {
                                crate::backend::eval::cesk::EnhancedMatcher::analyze(&lhs)
                            } else {
                                None
                            };
                            let compiled_rhs: Option<
                                std::sync::Arc<dyn std::any::Any + Send + Sync>,
                            > = if crate::backend::bytecode::can_compile_with_env(&rhs) {
                                crate::backend::bytecode::compile_bytecode_arc("rule_rhs", &rhs)
                                    .ok()
                                    .map(|chunk| {
                                        chunk as std::sync::Arc<dyn std::any::Any + Send + Sync>
                                    })
                            } else {
                                None
                            };
                            // Check if the inferred RHS type is monadic (IO, StateMonad, etc.)
                            let has_monadic_effect = rhs_type.as_ref().map_or(false, |t| {
                                t.is_monadic_type() || t.is_arrow_returning_monadic()
                            });
                            let requires_non_empty_first_arg =
                                rule_requires_non_empty_first_arg(&lhs, &rhs);
                            let body_wants_lazy_args = rhs_head_is_lazy_form(&rhs);
                            let lhs_head_all_meta_typed =
                                lhs_head_signature_all_meta_typed(&lhs, self);
                            let body_contains_cut = expr_contains_cut(&rhs);
                            let entry = RuleEntry {
                                lhs: lhs.clone(),
                                rhs_has_variables: rhs.contains_variables(),
                                rhs: rhs.clone(),
                                lhs_debruijn,
                                lhs_wide_debruijn: Vec::new(), // Bulk path uses MORK encoding
                                full_debruijn,
                                var_names,
                                wildcard_indices,
                                specificity: lhs_specificity(&lhs),
                                multiplicity,
                                rhs_type,
                                structural_matcher,
                                enhanced_matcher,
                                rule_index_in_group: 0,
                                global_rule_index: 0, // Assigned by RuleIndex::add_rule
                                compiled_rhs,
                                has_monadic_effect,
                                requires_non_empty_first_arg,
                                body_wants_lazy_args,
                                lhs_head_all_meta_typed,
                                body_contains_cut,
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
                        },
                    );
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
        match with_mork_query_bytes(
            rule_sexpr,
            &sm,
            self.mork_cache_epoch,
            |mork_bytes, _ctx| {
                let mut btm = self.shared.atom_space.btm.write();
                let new_count = increment_multiplicity(&mut btm, mork_bytes);
                drop(btm);

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            },
        ) {
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
                        if let Some(entry) = idx
                            .wildcard
                            .iter_mut()
                            .find(|e| e.lhs == lhs && e.rhs == rhs)
                        {
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
        match with_mork_query_bytes(
            rule_sexpr,
            &sm,
            self.mork_cache_epoch,
            |mork_bytes, _ctx| {
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

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            },
        ) {
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

        match with_mork_bytes(
            value,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |mork_bytes| {
                let mut btm = self.shared.atom_space.btm.write();
                let new_count = increment_multiplicity(&mut btm, mork_bytes);
                drop(btm);

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            },
        ) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — use wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                super::multiplicity::add_atom(&mut wbtm, &wide_key);
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                drop(wbtm);

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_add(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                count as usize
            }
        }
    }

    /// Decrement multiplicity for ANY atom.
    pub fn decrement_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned();

        match with_mork_bytes(
            value,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |mork_bytes| {
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

                self.shared
                    .atom_space
                    .total_atoms
                    .fetch_sub(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            },
        ) {
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
                    self.shared
                        .atom_space
                        .total_atoms
                        .fetch_sub(1, Ordering::Relaxed);
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
                if count == 0 {
                    1
                } else {
                    count as usize
                }
            }) {
                Ok(count) => count,
                Err(_) => {
                    // Wide expression (arity >= 64) — check wide_btm
                    let mut wide_key = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                    let wbtm = self.shared.atom_space.wide_btm.read();
                    let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                    if count == 0 {
                        1
                    } else {
                        count as usize
                    }
                }
            }
        } else {
            match with_mork_bytes(
                value,
                &self.shared_mapping,
                self.mork_cache_epoch,
                |mork_bytes| {
                    let btm = self.shared.atom_space.btm.read();
                    let count = get_multiplicity(&btm, mork_bytes);
                    if count == 0 {
                        1
                    } else {
                        count as usize
                    }
                },
            ) {
                Ok(count) => count,
                Err(_) => {
                    // Wide expression (arity >= 64) — check wide_btm
                    let mut wide_key = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                    let wbtm = self.shared.atom_space.wide_btm.read();
                    let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                    if count == 0 {
                        1
                    } else {
                        count as usize
                    }
                }
            }
        }
    }

    /// Get atom multiplicity from raw MORK bytes.
    pub fn get_multiplicity_from_mork_bytes(&self, mork_bytes: &[u8]) -> usize {
        let btm = self.shared.atom_space.btm.read();
        let count = get_multiplicity(&btm, mork_bytes);
        if count == 0 {
            1
        } else {
            count as usize
        }
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

#[cfg(test)]
mod bloom_depopulation_tests {
    //! Phase 11.A follow-up (2026-05-18) — regression coverage for the
    //! per-head RHS-atom bloom (`PerHeadAtomIndex`) depopulation gap
    //! along the `remove_rule_by_debruijn` path. Without these tests, the
    //! `note_rule_removed` call site at `RuleIndex::remove_rule_by_debruijn`
    //! can silently regress to a no-op (soft false-positive — overrouting
    //! to the trampoline path but still correct), which is hard to spot
    //! from the outside.
    //!
    //! Strategy: add a rule via `MettaEnvironment::add_rule` (real
    //! end-to-end path that populates `full_debruijn`), capture the
    //! stored entry's bytes, then directly call
    //! `RuleIndex::remove_rule_by_debruijn` (the path exercised in
    //! production at `core.rs::remove_from_space`).
    use super::*;
    use crate::backend::models::MettaValue;

    /// Helper: collect the `full_debruijn` bytes for the rule whose RHS
    /// is a single atom named `rhs_atom` within the (head, arity) group.
    /// Returns the first match.
    fn find_full_debruijn(
        idx: &RuleIndex<MettaValue>,
        head: &'static str,
        arity: usize,
        rhs_atom: &str,
    ) -> Vec<u8> {
        let group = idx
            .by_head_arity
            .get(&(head, arity))
            .expect("rule group exists after add_rule");
        for entry in group.all_entries() {
            if entry.rhs.as_atom() == Some(rhs_atom) {
                return entry.full_debruijn.clone();
            }
        }
        panic!("no entry with rhs atom = {rhs_atom:?} under head={head:?} arity={arity}");
    }

    #[test]
    fn remove_rule_by_debruijn_depopulates_bloom_on_full_removal() {
        // Rule: (= (foo arg-val) bloom_witness_atom)
        // Head = "foo", arity = 1 (one arg), RHS atom = "bloom_witness_atom"
        let mut env = MettaEnvironment::default();
        let head = crate::backend::models::gc_allocator::global_allocator().alloc_str("foo");
        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("arg-val".to_string()),
        ]);
        let rhs = MettaValue::Atom("bloom_witness_atom".to_string());

        env.add_rule(lhs.clone(), rhs.clone());

        // Bloom should reflect the new rule.
        {
            let idx = env.shared.rule_index.read();
            assert!(
                idx.rule_rhs_atoms.contains("foo", "bloom_witness_atom"),
                "bloom should contain (foo, bloom_witness_atom) after add_rule"
            );
            assert_eq!(idx.rule_rhs_atoms.len(), 1);
        }

        // Capture stored entry's full_debruijn and invoke removal.
        let bytes = {
            let idx = env.shared.rule_index.read();
            find_full_debruijn(&idx, head, 1, "bloom_witness_atom")
        };

        {
            let mut idx = env.shared.rule_index.write();
            let outcome = idx.remove_rule_by_debruijn(&bytes);
            assert_eq!(
                outcome,
                Some(true),
                "remove_rule_by_debruijn should report full removal"
            );
        }

        // Bloom must be depopulated symmetrically.
        let idx = env.shared.rule_index.read();
        assert!(
            !idx.rule_rhs_atoms.contains("foo", "bloom_witness_atom"),
            "bloom must drop (foo, bloom_witness_atom) after remove_rule_by_debruijn"
        );
        assert_eq!(
            idx.rule_rhs_atoms.len(),
            0,
            "bloom must be empty after the only rule referencing the atom was removed"
        );
    }

    #[test]
    fn remove_rule_by_debruijn_preserves_bloom_on_decrement() {
        // Same rule added twice: multiplicity = 2. First removal merely
        // decrements; the bloom MUST remain populated. Second removal
        // drops the entry and the bloom.
        let mut env = MettaEnvironment::default();
        let bar = crate::backend::models::gc_allocator::global_allocator().alloc_str("bar");
        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom("bar".to_string()),
            MettaValue::Atom("arg-val".to_string()),
        ]);
        let rhs = MettaValue::Atom("dup_witness".to_string());

        env.add_rule(lhs.clone(), rhs.clone());
        env.add_rule(lhs.clone(), rhs.clone()); // multiplicity = 2

        let bytes = {
            let idx = env.shared.rule_index.read();
            assert!(idx.rule_rhs_atoms.contains("bar", "dup_witness"));
            // Bloom refcount must be 1, not 2 — duplicate adds touch
            // multiplicity, not the bloom.
            assert_eq!(idx.rule_rhs_atoms.len(), 1);
            find_full_debruijn(&idx, bar, 1, "dup_witness")
        };

        // First debruijn removal: decrements multiplicity, bloom unchanged.
        {
            let mut idx = env.shared.rule_index.write();
            assert_eq!(idx.remove_rule_by_debruijn(&bytes), Some(false));
            assert!(
                idx.rule_rhs_atoms.contains("bar", "dup_witness"),
                "bloom must remain populated while multiplicity > 0"
            );
        }

        // Second removal: entry deleted, bloom must drop.
        {
            let mut idx = env.shared.rule_index.write();
            assert_eq!(idx.remove_rule_by_debruijn(&bytes), Some(true));
            assert!(
                !idx.rule_rhs_atoms.contains("bar", "dup_witness"),
                "bloom must drop (bar, dup_witness) once multiplicity reaches 0"
            );
            assert_eq!(idx.rule_rhs_atoms.len(), 0);
        }
    }

    #[test]
    fn remove_rule_by_debruijn_keeps_bloom_for_other_rules_sharing_atom() {
        // Two distinct rules under the same head, both referencing the
        // same RHS atom. Removing one drops the bloom refcount from 2 to
        // 1; the atom stays in the bloom until the second is removed.
        let mut env = MettaEnvironment::default();
        let rhs_atom = "shared_witness";

        // Rule 1: (= (baz a) shared_witness)
        let lhs1 = MettaValue::SExpr(vec![
            MettaValue::Atom("baz".to_string()),
            MettaValue::Atom("a".to_string()),
        ]);
        // Rule 2: (= (baz b) shared_witness)
        let lhs2 = MettaValue::SExpr(vec![
            MettaValue::Atom("baz".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        let rhs = MettaValue::Atom(rhs_atom.to_string());

        env.add_rule(lhs1.clone(), rhs.clone());
        env.add_rule(lhs2.clone(), rhs.clone());

        // Bloom should hold (baz, shared_witness) with refcount 2 — but
        // `contains` is membership-only, so we sanity-check via removal.
        {
            let idx = env.shared.rule_index.read();
            assert!(idx.rule_rhs_atoms.contains("baz", rhs_atom));
        }

        let bytes1 = {
            let idx = env.shared.rule_index.read();
            // Pull the entry whose LHS matches rule 1 by full-equality on
            // the original-name (lhs1, rhs) — bypassing the alpha-rename
            // surface to keep the test self-contained.
            let group = idx
                .by_head_arity
                .get(&("baz", 1))
                .expect("baz/2 group exists");
            let mut found = None;
            for entry in group.all_entries() {
                let lhs_items = entry.lhs.as_sexpr().expect("sexpr lhs");
                if lhs_items[1].as_atom() == Some("a") {
                    found = Some(entry.full_debruijn.clone());
                    break;
                }
            }
            found.expect("rule 1 stored under baz/2")
        };

        // Remove rule 1: refcount drops 2 → 1, but bloom membership stays.
        {
            let mut idx = env.shared.rule_index.write();
            assert_eq!(idx.remove_rule_by_debruijn(&bytes1), Some(true));
            assert!(
                idx.rule_rhs_atoms.contains("baz", rhs_atom),
                "bloom must still report membership while rule 2 references the atom"
            );
        }

        // Remove rule 2: refcount drops 1 → 0, bloom empty.
        let bytes2: Vec<u8> = {
            let idx = env.shared.rule_index.read();
            let group = idx
                .by_head_arity
                .get(&("baz", 1))
                .expect("baz/2 group still exists");
            let entry = group.all_entries().next().expect("rule 2 still present");
            // Materialize the clone into a new binding so the iterator
            // temporary is dropped before the read-guard. Without this,
            // the impl-Iterator destructor's borrow of `group` (which
            // borrows `idx`) extends past the read-guard's drop point.
            let owned = entry.full_debruijn.clone();
            drop(idx);
            owned
        };
        {
            let mut idx = env.shared.rule_index.write();
            assert_eq!(idx.remove_rule_by_debruijn(&bytes2), Some(true));
            assert!(
                !idx.rule_rhs_atoms.contains("baz", rhs_atom),
                "bloom must be depopulated once the last rule referencing the atom is removed"
            );
        }
    }

    #[test]
    fn remove_rule_by_debruijn_returns_none_when_not_found() {
        // No matching rule → None. Defensively guards against a regression
        // that would treat "not found" as "successfully removed" and
        // perturb the bloom (or the higher-level structural fallback).
        let mut env = MettaEnvironment::default();
        let lhs = MettaValue::SExpr(vec![MettaValue::Atom("qux".to_string())]);
        let rhs = MettaValue::Atom("present".to_string());
        env.add_rule(lhs, rhs);

        let mut idx = env.shared.rule_index.write();
        // Bytes that don't match any stored rule.
        let bogus_bytes: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(idx.remove_rule_by_debruijn(&bogus_bytes), None);
        assert!(
            idx.rule_rhs_atoms.contains("qux", "present"),
            "bloom must remain untouched on a not-found removal"
        );
    }
}
