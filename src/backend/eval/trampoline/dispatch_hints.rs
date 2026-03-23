//! Dispatch Hints — Memoization and Type-Driven Evaluation Shortcuts
//!
//! Optimization helpers extracted from `generic_trampoline.rs` to improve
//! icache locality for the hot trampoline loop. These functions are
//! infrequently called relative to the main evaluation path and benefit
//! from being in a separate compilation unit.
//!
//! ## Contents
//!
//! - **Normal-form memoization**: Bloom filter for fixpoint-detected expressions
//! - **Expected-type derivation**: `derive_arg_expected_type` for applicative evaluation
//! - **Eval memo cache**: Thread-local LRU for pure expression memoization
//! - **Match result cache**: Thread-local LRU for rule match result memoization

use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::sync::LazyLock;

use lru::LruCache;
use smallvec::SmallVec;

use crate::backend::builtin_signatures;
use crate::backend::environment::bloom::AtomicBloomFilter;
use crate::backend::hash_utils::IdentityU64BuildHasher;
use std::sync::atomic::Ordering;

use crate::backend::environment::rule_management::RULE_EPOCH;
use crate::backend::models::{GenericBindings, MettaValue, MettaValueFactory, MettaValueTrait};

use super::context::EvalContext;

// ============================================================================
// GC Epoch Tracking — REMOVED (I-9: Deterministic GC)
// ============================================================================
//
// The epoch-based cache invalidation system (GC_SWEEP_EPOCH, LOCAL_GC_EPOCH,
// check_gc_epoch) has been removed. With deterministic GC always-on, the
// nursery collector keeps the state space garbage-free at every safepoint.
// Caches never hold stale pointers, so epoch checking is unnecessary.

// ============================================================================
// Phase 9.5: Evaluated-expression normal-form memoization
// ============================================================================

/// Global bloom filter for normal-form expressions (Phase 9.5).
///
/// When an S-expression evaluates to itself (fixpoint), its `inner_ptr`
/// is inserted into this filter. Subsequent evaluations of the same pointer
/// skip the entire eval_step_generic call.
///
/// Invalidated on `add_rule()` since new rules may make previously
/// normal-form expressions reducible. `add_rule()` is O(N) during loading
/// and never during evaluation, so invalidation has zero eval-time cost.
///
/// False positives are benign: they cause us to skip evaluation for an
/// expression that might have been reducible, but since the expression was
/// never inserted (only a hash collision), this is extremely rare and bounded
/// by the bloom filter's ~1% FPR.
static NORMAL_FORM_BLOOM: LazyLock<AtomicBloomFilter> =
    LazyLock::new(|| AtomicBloomFilter::new(100_000));

/// Whether the normal-form bloom filter has any entries since its last clear.
///
/// When `false`, `is_memoized_normal_form` returns `false` immediately without
/// computing a bloom hash or touching atomic memory — eliminating ~1.5% overhead
/// in workloads where `add-atom` / `add_rule` calls keep clearing the filter
/// (e.g., PLN Robot demo with dynamic rule addition).
static NORMAL_FORM_BLOOM_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Check if a value is memoized as being in normal form (Phase 9.5).
#[inline]
pub fn is_memoized_normal_form<V: MettaValueTrait>(value: &V) -> bool {
    // Fast path: if no entries have been inserted since the last clear,
    // skip bloom hash computation entirely.
    if !NORMAL_FORM_BLOOM_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    // I-9: Epoch check removed — deterministic GC keeps state garbage-free.
    if !NORMAL_FORM_BLOOM_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let ptr = value.inner_ptr() as usize;
    NORMAL_FORM_BLOOM.may_contain(&ptr.to_le_bytes())
}

/// Record a value as being in normal form (Phase 9.5).
#[inline]
pub fn memoize_normal_form<V: MettaValueTrait>(value: &V) {
    let ptr = value.inner_ptr() as usize;
    NORMAL_FORM_BLOOM.insert(&ptr.to_le_bytes());
    NORMAL_FORM_BLOOM_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Invalidate the normal-form memoization bloom filter (Phase 9.5).
///
/// Called from `add_rule()` when new rules are added. New rules may make
/// previously normal-form expressions reducible, so the entire filter
/// must be cleared.
pub fn invalidate_normal_form_memo() {
    // Only clear if the LazyLock has been initialized (avoid initializing
    // it just to clear it during early startup)
    if LazyLock::get(&NORMAL_FORM_BLOOM).is_some() {
        NORMAL_FORM_BLOOM.clear();
        NORMAL_FORM_BLOOM_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

// ============================================================================
// Phase 9.2 + 9.3: expected_type derivation for applicative evaluation
// ============================================================================

/// Derive `expected_type` for an argument at `arg_idx` (1-based item position)
/// of the parent operator. Uses two tiers:
///
/// 1. **Builtin signatures** (Phase 9.2): Looks up the parent op's builtin
///    signature and extracts the formal type at the arg position.
/// 2. **User-declared arrow types** (Phase 9.3): If no builtin sig, checks
///    `env.get_types_generic(op)` for arrow types and extracts the consistent
///    arg type at the position.
///
/// Returns `None` if:
/// - The parent op has no type info, or
/// - The formal type is a meta-type (Atom, Expression, etc.), or
/// - Multiple arrow types disagree on the arg type at this position.
pub(super) fn derive_arg_expected_type<C: EvalContext>(
    items: &[C::Value],
    arg_idx: usize,
    env: &super::context::ContextEnv<C>,
    factory: &C::Factory,
) -> Option<C::Value>
where
    C::Value: Clone,
{
    let op = items.first().and_then(|v| v.as_atom())?;
    let arg_pos = arg_idx.checked_sub(1)?; // Convert to 0-based arg position

    // Tier 1: Builtin signatures (Phase 9.2)
    if let Some(name) = builtin_signatures::get_signature(op)
        .and_then(|sig| builtin_signatures::get_expected_type_at_position(sig, arg_pos))
        .and_then(builtin_signatures::type_expr_to_expected_type_name)
    {
        return Some(factory.atom(name));
    }

    // Tier 2: User-declared arrow types (Phase 9.3)
    extract_consistent_arg_type_from_env::<C>(op, arg_pos, env, factory)
}

/// Phase 9.3: Extract a consistent expected type at `arg_pos` from all
/// user-declared arrow types for `op`.
///
/// If ALL arrow types agree on a non-meta value type at the position,
/// returns `Some(type_atom)`. If they disagree or are meta-types, returns `None`
/// (conservative — don't prune).
fn extract_consistent_arg_type_from_env<C: EvalContext>(
    op: &str,
    arg_pos: usize,
    env: &super::context::ContextEnv<C>,
    factory: &C::Factory,
) -> Option<C::Value>
where
    C::Value: Clone,
{
    use super::super::step::{extract_arg_types, is_meta_type};

    let types = env.get_types_generic(op);
    if types.is_empty() {
        return None;
    }

    // Collect the type at arg_pos from each arrow type declaration.
    // We map to a &'static str (concrete type name) to avoid lifetime issues
    // with the temporary Vec<V> returned by extract_arg_types.
    let mut consistent_name: Option<&'static str> = None;
    let mut found_arrow = false;

    for t in &types {
        if let Some(arg_types) = extract_arg_types(t) {
            found_arrow = true;
            if let Some(arg_type) = arg_types.get(arg_pos) {
                if is_meta_type(arg_type) {
                    // Meta-type at this position → don't constrain
                    return None;
                }
                if let Some(name) = arg_type.as_atom() {
                    // Map to static str for concrete value types only
                    let static_name: &'static str = match name {
                        "Number" => "Number",
                        "Bool" => "Bool",
                        "String" => "String",
                        _ => return None, // Unknown/user-defined type → conservative
                    };
                    match consistent_name {
                        None => consistent_name = Some(static_name),
                        Some(prev) if prev != static_name => return None, // Disagree
                        _ => {} // Same name — still consistent
                    }
                } else {
                    // Structural type (e.g., (List Number)) — can't map to atom name
                    return None;
                }
            }
            // If arg_pos is out of bounds for this arrow, skip (variadic)
        }
    }

    if !found_arrow {
        return None;
    }

    consistent_name.map(|name| factory.atom(name))
}

// ============================================================================
// Phase 3: Thread-local Evaluation Memoization Cache
// ============================================================================
//
// Pure expression memoization: caches evaluation results keyed by content hash.
// Eliminates redundant computation of overlapping subproblems in PLN reasoning
// (e.g., `(Truth_Deduction (stv 0.9 0.9) (stv 0.8 0.9))` evaluated many times).
//
// Impure operations (space mutations, state, I/O) are excluded by head symbol.
// The cache is cleared on space mutation (add-atom/remove-atom) and at GC
// safepoints to ensure correctness.

/// Check whether a head symbol names an impure operation that must NOT be
/// memoized.  These operations have side effects or depend on mutable state.
///
/// Implemented as a `match` expression so the compiler can emit a perfect-hash
/// jump table — O(1) regardless of the number of heads (vs O(n) linear scan
/// through an `&[&str]` slice).
#[inline]
fn is_impure_head(head: &str) -> bool {
    matches!(
        head,
        "add-atom" | "remove-atom" | "get-atoms"
            | "new-state" | "change-state!" | "get-state"
            | "match" | "match-or"
            | "import!" | "include"
            | "println!" | "trace!" | "nop"
            | "new-space" | "mod-space!"
            | "bind!"
            | "new-memo" | "memo" | "clear-memo!" | "memo-stats"
            | "pragma!"
            | "=" | ":" | ":<"
    )
}

thread_local! {
    /// Thread-local evaluation memoization cache.
    ///
    /// Key: content hash of the S-expression (via `hash_value()`)
    /// Value: cached evaluation results (SmallVec avoids heap for ≤4 results)
    ///
    /// 8192 entries × ~40 bytes avg = ~320 KB per thread. LRU eviction bounds memory.
    static EVAL_MEMO: RefCell<LruCache<u64, SmallVec<[MettaValue; 4]>, IdentityU64BuildHasher>> =
        RefCell::new(LruCache::with_hasher(NonZeroUsize::new(8192).expect("non-zero"), IdentityU64BuildHasher));
}

/// Check if an S-expression should be memoized (pure head, ≥2 items,
/// no free variables).
#[inline]
pub fn should_memoize<V: MettaValueTrait>(value: &V) -> bool {
    if let Some(items) = value.as_sexpr() {
        // Allow zero-arity function calls (items.len() == 1, e.g. `(kbstatic)`)
        // to be memoized. Only reject truly empty S-expressions (len == 0 → Unit).
        if items.is_empty() {
            return false;
        }
        // Expressions containing variables produce context-dependent results
        // (the same expression text evaluates differently depending on which
        // bindings are active in the enclosing let/pattern match). Memoizing
        // these by content hash would conflate different binding contexts.
        if value.has_variables_fast() {
            return false;
        }
        if let Some(head) = items.first().and_then(|h| h.as_atom()) {
            // Skip impure operations
            if is_impure_head(head) {
                return false;
            }
            // `!` and `eval` are transparent wrappers — their purity
            // depends on the inner expression, so recurse.
            if (head == "!" || head == "eval") && items.len() == 2 {
                return should_memoize(&items[1]);
            }
            true
        } else {
            // Variable-headed S-expressions — don't memoize (result depends
            // on which rules match the resolved head)
            false
        }
    } else {
        false
    }
}

/// Look up cached evaluation results for an expression hash.
///
/// Returns `Some(results)` on cache hit. The caller should push a Resume
/// with these results instead of evaluating the expression.
///
/// GC safety: Before accessing cached values, checks the GC sweep epoch.
/// If GC freed slab slots since the cache was populated, the entire cache
/// is cleared (returning `None`) to avoid use-after-free on stale pointers.
#[inline]
pub fn eval_memo_get(expr_hash: u64) -> Option<Vec<MettaValue>> {
    // I-9: Epoch check removed — deterministic GC keeps state garbage-free.
    EVAL_MEMO.with(|memo_cell| {
        let mut memo = memo_cell.borrow_mut();
        memo.get(&expr_hash).map(|entries| entries.to_vec())
    })
}

/// Store evaluation results in the memo cache.
#[inline]
pub fn eval_memo_put(expr_hash: u64, results: &[MettaValue]) {
    let entries: SmallVec<[MettaValue; 4]> = results.iter().copied().collect();
    EVAL_MEMO.with(|memo_cell| {
        memo_cell.borrow_mut().put(expr_hash, entries);
    });
}

/// Collect all MettaValue roots from the eval memo cache.
///
/// Called during GC safepoint root collection to ensure cached values survive
/// the mark-sweep cycle.
pub fn collect_eval_memo_roots(out: &mut Vec<MettaValue>) {
    EVAL_MEMO.with(|memo_cell| {
        let memo = memo_cell.borrow();
        for (_hash, entries) in memo.iter() {
            out.extend_from_slice(entries);
        }
    });
}

/// Clear the eval memo cache.
///
/// Called on space mutation (add-atom/remove-atom) to invalidate cached results
/// that may depend on the changed rules/facts.
pub fn clear_eval_memo() {
    EVAL_MEMO.with(|memo_cell| {
        memo_cell.borrow_mut().clear();
    });
}

// ============================================================================
// Phase 5: Thread-local Match Result Cache
// ============================================================================
//
// Caches the output of `try_match_all_rules_generic` — the set of matching
// rules (RHS template, bindings, return type) for a given expression.
//
// Key: expression content hash (u64).
// Stored alongside each entry: the rule epoch at insertion time and the
// expression arity as a collision guard. On lookup, the cache validates
// that the rule epoch hasn't advanced and the arity matches.
//
// Invalidated:
// - Rule epoch change (automatic via stored epoch comparison)
// - GC sweep epoch change (via `check_gc_epoch()` → cache clear)
//
// This eliminates redundant MORK serialization + extract_data calls when
// the same expression is matched multiple times within the same rule epoch
// (common in PLN nondeterministic branching).

/// Cached match result: (rhs_template, bindings, rhs_type).
type MatchResultEntry = SmallVec<[(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>); 4]>;

thread_local! {
    /// Thread-local rule-match result cache.
    ///
    /// Key: expression content hash (via `hash_value()`)
    /// Value: (rule_epoch, arity, cached match results)
    ///
    /// 4096 entries × ~120 bytes avg = ~480 KB per thread. LRU eviction bounds memory.
    static MATCH_RESULT_CACHE: RefCell<LruCache<u64, (u64, usize, MatchResultEntry), IdentityU64BuildHasher>> =
        RefCell::new(LruCache::with_hasher(NonZeroUsize::new(4096).expect("non-zero"), IdentityU64BuildHasher));
}

/// Look up cached match results for an expression.
///
/// Returns `Some(results)` if the cache contains a valid entry for the given
/// expression hash at the current rule epoch. The caller should use these
/// results instead of calling `try_match_all_rules_generic`.
///
/// GC safety: checks GC sweep epoch before access (via `check_gc_epoch()`).
#[inline]
pub fn match_result_get(
    expr_hash: u64,
    expr_arity: usize,
) -> Option<Vec<(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>)>> {
    // I-9: Epoch check removed — deterministic GC keeps state garbage-free.
    let current_epoch = RULE_EPOCH.load(Ordering::Acquire);
    MATCH_RESULT_CACHE.with(|cache_cell| {
        let mut cache = cache_cell.borrow_mut();
        if let Some((stored_epoch, stored_arity, entries)) = cache.get(&expr_hash) {
            if *stored_epoch == current_epoch && *stored_arity == expr_arity {
                return Some(entries.iter().cloned().collect());
            }
        }
        None
    })
}

/// Store match results in the cache.
#[inline]
pub fn match_result_put(
    expr_hash: u64,
    expr_arity: usize,
    results: &[(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>)],
) {
    let current_epoch = RULE_EPOCH.load(Ordering::Acquire);
    let entries: MatchResultEntry = results.iter().cloned().collect();
    MATCH_RESULT_CACHE.with(|cache_cell| {
        cache_cell.borrow_mut().put(expr_hash, (current_epoch, expr_arity, entries));
    });
}

/// Collect all MettaValue roots from the match result cache.
///
/// Called during GC safepoint root collection to ensure cached values survive
/// the mark-sweep cycle.
pub fn collect_match_result_roots(out: &mut Vec<MettaValue>) {
    MATCH_RESULT_CACHE.with(|cache_cell| {
        let cache = cache_cell.borrow();
        for (_hash, (_epoch, _arity, entries)) in cache.iter() {
            for (rhs, bindings, rhs_type) in entries.iter() {
                out.push(*rhs);
                for (_name, val) in bindings.iter() {
                    out.push(val.clone());
                }
                if let Some(t) = rhs_type {
                    out.push(*t);
                }
            }
        }
    });
}

/// Clear the match result cache.
///
/// Called on space mutation (add-atom/remove-atom) to invalidate cached results.
pub fn clear_match_result_cache() {
    MATCH_RESULT_CACHE.with(|cache_cell| {
        cache_cell.borrow_mut().clear();
    });
}

// ============================================================================
// Phase E: Operator Inline Cache — Thread-Local Rule Metadata Cache
// ============================================================================
//
// Caches per-operator metadata to skip hash computation and bloom filter
// lookups when all rule candidates have structural matchers. Keyed by
// (head_symbol, arity), validated by rule epoch. When `all_structural` is
// true, `try_match_all_rules_generic` can skip `hash_value()` (7.48% CPU)
// and the match result cache entirely — going straight to structural matching.

/// Cached metadata about an operator's rule candidates.
#[derive(Clone, Debug)]
pub struct OperatorCacheEntry {
    /// Rule epoch at cache insertion time — used for staleness check.
    pub rule_epoch: u64,
    /// Whether ALL candidates for this (head, arity) have structural matchers.
    pub all_structural: bool,
    /// Number of rule candidates for this (head, arity).
    pub candidate_count: usize,
}

thread_local! {
    /// Thread-local operator metadata cache.
    ///
    /// Key: combined u64 from (interned head symbol pointer, arity).
    /// Using interned string pointer as key component avoids hashing
    /// the string on every lookup. Since MeTTa atoms are interned via
    /// the slab allocator, the same symbol always has the same `&'static str` pointer.
    ///
    /// 512 entries × ~40 bytes = ~20 KB per thread. LRU eviction bounds memory.
    static OPERATOR_CACHE: RefCell<LruCache<u64, OperatorCacheEntry, IdentityU64BuildHasher>> =
        RefCell::new(LruCache::with_hasher(NonZeroUsize::new(512).expect("non-zero"), IdentityU64BuildHasher));
}

/// Combine head pointer and arity into a single u64 key.
/// Uses Fibonacci mixing on the pointer to spread aligned addresses,
/// then XORs with arity to differentiate same-head different-arity ops.
#[inline(always)]
fn op_cache_key(head: &str, arity: usize) -> u64 {
    let ptr = head.as_ptr() as u64;
    // Fibonacci hash mixing for the pointer (spread aligned addresses)
    let mixed = ptr.wrapping_mul(0x517cc1b727220a95);
    mixed ^ (arity as u64)
}

/// Look up cached operator metadata.
///
/// Returns `Some(entry)` if a valid entry exists for this (head, arity)
/// at the current rule epoch. Returns `None` on cache miss or stale entry.
#[inline]
pub fn operator_cache_get(head: &str, arity: usize) -> Option<OperatorCacheEntry> {
    let current_epoch = RULE_EPOCH.load(Ordering::Acquire);
    let key = op_cache_key(head, arity);
    OPERATOR_CACHE.with(|cache_cell| {
        let mut cache = cache_cell.borrow_mut();
        if let Some(entry) = cache.get(&key) {
            if entry.rule_epoch == current_epoch {
                return Some(entry.clone());
            }
        }
        None
    })
}

/// Store operator metadata in the cache.
#[inline]
pub fn operator_cache_put(head: &str, arity: usize, entry: OperatorCacheEntry) {
    let key = op_cache_key(head, arity);
    OPERATOR_CACHE.with(|cache_cell| {
        cache_cell.borrow_mut().put(key, entry);
    });
}

/// Clear the operator cache.
///
/// Called on GC epoch change (pointer-keyed entries may be stale).
pub fn clear_operator_cache() {
    OPERATOR_CACHE.with(|cache_cell| {
        cache_cell.borrow_mut().clear();
    });
}

// ============================================================================
// Phase 6 (revised): Normal-Form Short-Circuit in dispatch_rule_matches
// ============================================================================
//
// After apply_bindings_generic produces an instantiated RHS, many values are
// already in normal form (data tuples, ground atoms) and the trampoline just
// returns them unchanged — wasting a pop/push/dispatch cycle per value.
//
// This module provides a bounded static predicate that identifies such values
// without speculation. Combined with the existing NORMAL_FORM_BLOOM filter
// (Phase 9.5), this short-circuits the trampoline for cold and hot paths.

use phf::phf_set;

/// Union of all head symbols that are reducible in `eval_sexpr_step_generic`
/// (special forms) and `has_generic_grounded_op` (arithmetic/comparison ops).
///
/// An S-expression `(head ...)` is NOT in normal form if `head` is in this set,
/// because the trampoline will dispatch it for evaluation.
///
/// Maintained in sync with `eval_sexpr_step_generic` match arms,
/// `GROUNDED_OPS`, `SPECIAL_FORMS_REDISPATCH`, and `EAGER_SPECIAL_FORMS`
/// via the `reducible_heads_covers_all_known_sets` test below.
pub(crate) static REDUCIBLE_HEADS: phf::Set<&'static str> = phf_set! {
    // === Special forms (eval_sexpr_step_generic match arms) ===
    "=", "!", "quote", "unquote",
    "if", "if-reducible", "if-equal",
    "error", "Error", "is-error", "catch",
    "eval", "function", "return", "chain",
    "match", "match-or", "case",
    "switch", "switch-minimal", "switch-internal",
    "let", "let*", "unify", "sealed", "atom-subst",
    ":<", ":",
    "get-type", "check-type", "validate-atom", "get-type-space",
    "is-function", "type-cast", "metta",
    "match-types", "match-type-or", "first-from-pair",
    "map-atom", "filter-atom", "foldl-atom",
    "car-atom", "cdr-atom", "cons-atom", "decons-atom", "size-atom",
    "max-atom", "min-atom", "index-atom",
    "tuple-concat", "tuple-count", "without", "element-of",
    "range", "reverse-atom", "flatten-atom", "zip-atom",
    "take-atom", "drop-atom", "sort-tuple", "best-candidate",
    "new-space", "add-atom", "remove-atom",
    "collapse", "collapse-bind", "superpose", "amb",
    "guard", "commit", "backtrack",
    "get-atoms",
    "new-state", "get-state", "change-state!",
    "new-memo", "memo", "memo-first", "clear-memo!", "memo-stats",
    "bind!", "println!", "trace!", "nop",
    "repr", "format-args",
    "empty", "get-metatype",
    "include", "import!", "mod-space!", "print-mods!",
    "exec", "coalg", "lookup", "rulify",
    "=alpha",
    "unique-atom", "union-atom", "intersection-atom", "subtraction-atom",
    "assertEqual", "assertAlphaEqual",
    "assertEqualMsg", "assertAlphaEqualMsg",
    "assertEqualToResult", "assertAlphaEqualToResult",
    "assertEqualToResultMsg", "assertAlphaEqualToResultMsg",
    "pragma!",
    // === Grounded operations (has_generic_grounded_op) ===
    "+", "-", "*", "/", "%", "min", "max",
    "<", "<=", ">", ">=", "==", "!=",
    "and", "or", "not", "xor",
    "/safe", "clamp",
    // === Extended grounded ops (GROUNDED_OPS PHF in helpers.rs) ===
    "pow", "abs", "floor", "ceil", "round", "sqrt",
    "floor-div",
    "pow-math", "sqrt-math", "abs-math", "log-math", "trunc-math",
    "ceil-math", "floor-math", "round-math",
    "sin-math", "asin-math", "cos-math", "acos-math",
    "tan-math", "atan-math",
    "isnan-math", "isinf-math",
};

/// Check if a value is in normal form with bounded recursion.
///
/// Returns `true` only for values guaranteed to pass through
/// `eval_step_generic_inner` and `eval_sexpr_step_generic` unchanged.
/// Conservative: returns `false` for anything uncertain.
///
/// `max_depth` limits S-expression child recursion:
/// - `0`: reject S-expr children (shallow check only)
/// - `2`: recurse up to 2 levels (catches nested PLN data tuples)
///
/// Uses `MettaValueTrait` accessor methods (which are Spanned-transparent)
/// rather than pattern matching on `MettaValueInner` directly, avoiding
/// type mismatches between `V` and the concrete `MettaValue` stored in
/// `MettaValueInner` fields.
#[inline]
pub fn is_normal_form_bounded<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static>(
    value: &V,
    env: &crate::backend::environment::generic::GenericEnvironment<V, impl MettaValueFactory<V> + Copy + Clone>,
    max_depth: u8,
) -> bool {
    // S-expressions: check head + children
    if let Some(items) = value.as_sexpr() {
        if items.is_empty() { return true; }
        // Head must be a plain atom (not a variable or nested expr)
        let head = match items[0].as_atom() {
            Some(name) => name,
            None => return false,
        };
        // Head must not be variable, special form, or grounded op
        if head.starts_with('$') { return false; }
        if REDUCIBLE_HEADS.contains(head) { return false; }
        // Head must not have user-defined rules
        if env.may_have_rules_for(head, items.len() - 1) { return false; }
        // Check children within depth budget
        return items[1..].iter().all(|child| is_child_normal_form(child, env, max_depth));
    }
    // Atoms: normal form UNLESS &self (resolves to space) or
    // starts with $ (variable). Tokenizer bindings (bind!) are
    // rare and caught by bloom filter on second eval.
    if let Some(name) = value.as_atom() {
        return name != "&self" && !name.starts_with('$');
    }
    // Conjunctions: need eval_conjunction_step_generic
    if value.as_conjunction().is_some() { return false; }
    // Ground types (Bool, Long, Float, String, Unit, Space, State, Memo),
    // Error, Empty, Type, Quoted — all immediately return Done.
    // This covers all remaining MettaValueInner variants.
    true
}

/// Check if an S-expression child element is in normal form.
///
/// For non-S-expression leaves, this is a direct check via trait accessors.
/// For S-expression children, recurse with decremented depth budget.
#[inline]
fn is_child_normal_form<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static>(
    child: &V,
    env: &crate::backend::environment::generic::GenericEnvironment<V, impl MettaValueFactory<V> + Copy + Clone>,
    max_depth: u8,
) -> bool {
    // S-expression children: recurse with depth budget
    if child.as_sexpr().is_some() {
        if max_depth == 0 { return false; }
        return is_normal_form_bounded(child, env, max_depth - 1);
    }
    // Atoms: OK unless &self or variable
    if let Some(name) = child.as_atom() {
        return name != "&self" && !name.starts_with('$');
    }
    // Conjunctions: reducible
    if child.as_conjunction().is_some() { return false; }
    // All other types (ground, error, empty, type, quoted): normal form
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reducible_heads_covers_all_known_sets() {
        use crate::backend::eval::helpers::{is_grounded_op, needs_special_form_redispatch, is_eager_special_form};

        // Check GROUNDED_OPS coverage
        let grounded_ops = [
            "+", "-", "*", "/", "%", "min", "max",
            "pow", "abs", "floor", "ceil", "round", "sqrt",
            "floor-div",
            "pow-math", "sqrt-math", "abs-math", "log-math", "trunc-math",
            "ceil-math", "floor-math", "round-math",
            "sin-math", "asin-math", "cos-math", "acos-math",
            "tan-math", "atan-math",
            "isnan-math", "isinf-math",
            "<", "<=", ">", ">=", "==", "!=",
            "not", "and", "or", "xor",
            "get-type", "get-metatype", "validate-atom", "get-type-space",
            "car-atom", "cdr-atom", "cons-atom", "decons-atom", "size-atom",
            "max-atom", "min-atom", "index-atom",
            "tuple-concat", "tuple-count", "without", "element-of",
            "range", "reverse-atom", "flatten-atom", "zip-atom", "take-atom", "drop-atom",
            "sort-tuple", "best-candidate",
            "/safe", "clamp",
        ];
        for op in &grounded_ops {
            assert!(is_grounded_op(op), "GROUNDED_OPS has '{}' but is_grounded_op doesn't recognize it", op);
            assert!(REDUCIBLE_HEADS.contains(op), "REDUCIBLE_HEADS missing grounded op: {}", op);
        }

        // Check SPECIAL_FORMS_REDISPATCH coverage
        let special_forms = [
            "map-atom", "filter-atom", "foldl-atom",
            "sort-tuple", "best-candidate",
            "if", "if-equal", "if-reducible", "case", "switch", "switch-minimal", "switch-internal",
            "let", "let*", "unify",
            "chain", "function", "return",
            "sealed", "atom-subst", "match", "match-or",
            "catch", "is-error",
            "eval", "quote", "unquote",
            "collapse", "collapse-bind", "amb", "guard",
            "new-state", "get-state", "change-state!",
            "println!", "trace!",
            "unique-atom", "union-atom", "intersection-atom", "subtraction-atom",
            "=alpha",
            "match-types",
            "assertEqual", "assertAlphaEqual",
            "assertEqualMsg", "assertAlphaEqualMsg",
            "assertEqualToResult", "assertAlphaEqualToResult",
            "assertEqualToResultMsg", "assertAlphaEqualToResultMsg",
        ];
        for op in &special_forms {
            assert!(needs_special_form_redispatch(op), "SPECIAL_FORMS_REDISPATCH has '{}' but needs_special_form_redispatch doesn't recognize it", op);
            assert!(REDUCIBLE_HEADS.contains(op), "REDUCIBLE_HEADS missing special form: {}", op);
        }

        // Check EAGER_SPECIAL_FORMS coverage
        let eager_forms = [
            "map-atom", "filter-atom", "foldl-atom",
            "sort-tuple", "best-candidate",
            "eval", "unquote",
            "collapse", "collapse-bind", "superpose",
            "get-state",
            "catch",
            "get-metatype", "validate-atom", "get-type-space",
            "repr", "format-args",
            "unique-atom", "union-atom", "intersection-atom", "subtraction-atom",
            "=alpha",
        ];
        for op in &eager_forms {
            assert!(is_eager_special_form(op), "EAGER_SPECIAL_FORMS has '{}' but is_eager_special_form doesn't recognize it", op);
            assert!(REDUCIBLE_HEADS.contains(op), "REDUCIBLE_HEADS missing eager form: {}", op);
        }

        // Check has_generic_grounded_op coverage
        let generic_grounded = [
            "+", "-", "*", "/", "%", "min", "max",
            "<", "<=", ">", ">=", "==", "!=",
            "and", "or", "not", "xor",
            "/safe", "clamp",
        ];
        for op in &generic_grounded {
            assert!(
                crate::backend::grounded::has_generic_grounded_op(op),
                "has_generic_grounded_op doesn't recognize: {}", op
            );
            assert!(REDUCIBLE_HEADS.contains(op), "REDUCIBLE_HEADS missing generic grounded op: {}", op);
        }
    }
}
