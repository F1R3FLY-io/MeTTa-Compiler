//! Dispatch Hints — Memoization and Type-Driven Evaluation Shortcuts
//!
//! Optimization helpers extracted from `eval_loop.rs` to improve
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

use std::cell::{Cell, RefCell};
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
/// When an S-expression evaluates to itself (fixpoint), its content hash is
/// inserted into this filter. Subsequent evaluations of the same structure skip
/// the entire eval_step_generic call.
///
/// Invalidated on atom-space mutations and at top-level query boundaries. New
/// rules or facts may make previously normal-form expressions reducible, while
/// query boundaries prevent bloom false positives from leaking across unrelated
/// evaluations.
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

/// Debug-only counter incremented every time `memoize_normal_form` silently
/// rejects a value because its head is reducible (or a variable).
///
/// Replaces the H12 `debug_assert!` panic at the same site: VM and JIT
/// dispatch paths legitimately pass reducible-head expressions through
/// `op_dispatch_rules` → `memoize_normal_form` (e.g., `(map-atom ...)`,
/// `(first-from-pair ...)`), and the writer-side filter mirrors the
/// reader-side filter at `is_memoized_normal_form` so the bloom never
/// receives them. The counter exists for regression detection: tests
/// can sample it to confirm rejections are happening (canary at the
/// bottom of this file).
#[cfg(debug_assertions)]
static NORMAL_FORM_REJECT_COUNT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Read the current rejection count (debug-only). Used by the canary
/// regression test.
#[cfg(test)]
pub(crate) fn normal_form_reject_count() -> u64 {
    #[cfg(debug_assertions)]
    {
        NORMAL_FORM_REJECT_COUNT.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(debug_assertions))]
    {
        0
    }
}

/// Check if a value is memoized as being in normal form (Phase 9.5).
///
/// H12 (2026-05-05): Structural prefilter converts soft-correctness
/// (bloom may have FPR or hash-cons aliasing artifacts) into
/// hard-correctness (a reducible head is *never* normal form by
/// definition). Cheap: one atom lookup + one match. The bloom is
/// purely an optimization hint — wrong answers must never let us
/// skip evaluation of an expression that has rules to apply.
/// DIAGNOSTIC kill-switch (env `METTATRON_DISABLE_NORMAL_FORM_BLOOM=1`): when set,
/// `is_memoized_normal_form` always returns `false`, forcing every expression to be
/// evaluated (the bloom skip-eval is disabled entirely). Dormant by default (unset ⇒
/// byte-identical). Used to discriminate whether a residual DEDICATED=1 wrong-subset
/// corruption flows through the bloom/skip-eval decision (a stale bloom / VALUE_HASH_CACHE
/// probe under the index collector) vs elsewhere — a normal-form skip is purely an
/// optimization hint, so disabling it is always semantically safe (only slower).
fn normal_form_bloom_disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("METTATRON_DISABLE_NORMAL_FORM_BLOOM")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

#[inline]
pub fn is_memoized_normal_form<V: MettaValueTrait>(value: &V) -> bool {
    // DIAGNOSTIC kill-switch (dormant by default; see `normal_form_bloom_disabled`).
    if normal_form_bloom_disabled() {
        return false;
    }
    // Fast path: if no entries have been inserted since the last clear,
    // skip bloom hash computation entirely.
    if !NORMAL_FORM_BLOOM_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    // H12 structural correctness gate — before trusting the bloom,
    // verify the expression's head cannot itself be reducible. This
    // protects against bloom collisions and the H12 ~1% FPR.
    if let Some(items) = value.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            if is_reducible_head(head) {
                return false;
            }
            if head.starts_with('$') {
                return false;
            }
        }
    }
    // Option B fix (2026-05-06): bloom keyed on content-hash, not
    // `inner_ptr`. Pre-fix, `inner_ptr()` returned the OUTER
    // MettaValueInner pointer including any Spanned wrapper. When a
    // freeze-tuple-memoized value flowed through let/chain/Done/Resume
    // continuations, it could pick up a Spanned wrapper or hash-cons
    // alias to a different slab slot — producing a different `inner_ptr`
    // that missed the bloom even though the value was structurally
    // unchanged. Direct.metta test 2 hit this ~6.5% of runs (depending
    // on parallel scheduler decisions). `hash_value` is fully Spanned-
    // transparent (`metta_value.rs:212-214`) and content-stable across
    // slab re-allocations, eliminating the leak. The thread-local
    // VALUE_HASH_CACHE makes warm lookups near-O(1).
    let key = value.hash_value();
    NORMAL_FORM_BLOOM.may_contain(&key.to_le_bytes())
}

/// Record a value as being in normal form (Phase 9.5).
///
/// Writer-side structural guard (mirrors `is_memoized_normal_form`'s
/// reader-side filter at `dispatch_hints.rs:86-98`). When the head is
/// reducible OR is a variable, silently no-op — the bloom is purely
/// advisory and the read side rejects the entry anyway. Restores
/// writer/reader symmetry without changing observable behavior at any
/// tier (VM, JIT, trampoline).
///
/// Originally added (H12) as a panic-on-bug `debug_assert!` canary, but
/// VM/JIT dispatch paths (`bytecode/vm/mod.rs:6036, 6045`,
/// `bytecode/jit/runtime/call_support.rs:370, 590, 704, 812`) legitimately
/// pass reducible-head expressions through `op_dispatch_rules` and could
/// not be converted to per-caller gates without duplicating logic across
/// 6 call sites + 4 freeze-tuple sites. The single writer-side guard is
/// total, symmetric, and cannot be violated by a missing caller-side check.
/// Debug-only `NORMAL_FORM_REJECT_COUNT` provides the regression-canary
/// signal that the panic used to provide.
#[inline]
pub fn memoize_normal_form<V: MettaValueTrait>(value: &V) {
    if let Some(items) = value.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            if is_reducible_head(head) || head.starts_with('$') {
                #[cfg(debug_assertions)]
                NORMAL_FORM_REJECT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    }
    // Option B fix (2026-05-06): bloom keyed on content-hash. See
    // `is_memoized_normal_form` for rationale.
    let key = value.hash_value();
    NORMAL_FORM_BLOOM.insert(&key.to_le_bytes());
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

/// H12 (2026-05-05): clear the normal-form bloom at a top-level `!` query
/// boundary, eliminating cross-query memo bleed.
///
/// Bloom entries record `inner_ptr`s that the runtime believed were normal
/// form at *some* point during a particular query. With hash-cons aliasing
/// (two ground sexprs share the same `inner_ptr` by content) and the bloom's
/// intrinsic ~1% FPR, an entry inserted during query N can poison query N+1
/// — most visibly with `freeze-tuple` outputs colliding with reducible
/// expressions in later queries (Direct.metta / Toothbrush observed
/// `(reduce (eval (grandfather a c)))` short-circuiting on a stale hit).
///
/// Called from `eval_loop.rs` alongside `increment_query_generation()` so
/// that every top-level `!` invocation starts with a clean bloom. Within a
/// single query, the bloom still optimizes repeated normal-form checks for
/// `freeze-tuple` and friends — only cross-query reuse is sacrificed.
pub fn clear_normal_form_memo_for_new_query() {
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
    items: &[MettaValue],
    arg_idx: usize,
    env: &super::context::MettaEnvironment,
    factory: &crate::backend::models::ActiveFactory,
) -> Option<MettaValue> {
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
    env: &super::context::MettaEnvironment,
    factory: &crate::backend::models::ActiveFactory,
) -> Option<MettaValue> {
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
// The cache is cleared on space mutation (add-atom/remove-atom) and its values
// are registered as GC roots instead of being dropped at safepoints.

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
            | "match" | "match-or" | "unify"
            | "import!" | "git-import!" | "git-module!" | "include" | "register-module!"
            | "println!" | "print-alternatives!" | "trace!" | "nop"
            | "new-space" | "mod-space!"
            | "bind!"
            // Phase 1 cut-barrier (2026-05-26): `(cut)` is side-effecting — it
            // latches the thread-local CUT_SIGNAL via `set_cut_active`. It must
            // NEVER be memoized: two textually-identical `(cut)` calls in
            // distinct cut scopes (e.g. an inner cut-rule's `(cut)` and an outer
            // clause's `(cut)`) hash identically, so memoizing the first
            // returns its cached `(unit)` for the second WITHOUT re-firing the
            // signal — the outer cut then never prunes (cut_nested divergence:
            // MTT committed to LAST not FIRST). `expression_involves_cut_rules`
            // only catches RULES whose RHS contains cut, not a bare `(cut)`.
            | "cut"
            // Phase 2 (2026-05-26): `(once X)` desugars to a barrier-scoped
            // `(cut)` committing X to its first answer. It must NOT be
            // memoized: (1) the desugar fires the `(cut)` side-effect (the
            // cut_nested class — a memoized `(once …)` returns a cached value
            // without re-opening the barrier / re-pruning); (2) X may be impure
            // (e.g. `(once (match &self …))` over a mutated space → stale).
            | "once"
            | "new-memo" | "memo" | "clear-memo!" | "memo-stats"
            | "pragma!"
            | "=" | ":" | ":<"
            | "ground-with-bindings" | "freeze-tuple"
            // Plan 1 audit (2026-05-06): assertion + test ops have observable
            // diagnostic output (println!-style); memoizing them suppresses
            // diagnostic prints on repeat calls. `exec` is presumed
            // side-effecting per its name.
            | "test"
            | "assertEqual" | "assertAlphaEqual"
            | "assertEqualMsg" | "assertAlphaEqualMsg"
            | "assertEqualToResult" | "assertAlphaEqualToResult"
            | "assertEqualToResultMsg" | "assertAlphaEqualToResultMsg"
            | "exec"
            // T07/021-022: fileio + random ops mutate global registries
            // and/or hit the filesystem. Memoizing them would erase ordering
            // semantics for back-to-back calls (e.g. reading after writing
            // the same file) and would defeat seeded-RNG determinism (a
            // memoized random-int call would always return the same value).
            | "file-open!" | "file-read-to-string!" | "file-write!"
            | "file-seek!" | "file-read-exact!" | "file-get-size!"
            | "new-random-generator" | "random-int" | "random-float"
            | "set-random-seed" | "reset-random-generator" | "flip"
            // Phase I (2026-05-20): concurrency primitives are side-
            // effecting (state mutation via spawn body, observer events,
            // CAS). Must force eager eval through let* lazy-binding gates.
            | "spawn!" | "await!" | "await-barrier!"
            | "compare-and-swap-state!" | "loop-until-state"
            | "new-das!" | "new-distributed-space" | "das-barrier!"
            | "add-observer!"
            | "snapshot!" | "partition-space"
            // Stage 5a ACT out-of-core (2026-05-27): `save-space!`/`load-space!`
            // hit the filesystem (`/dev/shm/<name>.act`) and `load-space!`
            // mutates the atom space; `query-act` reads an mmap'd file whose
            // contents can change between calls (a later `save-space!`). All
            // three must re-execute, never serve a memoized result — same
            // rationale as the `file-*!` ops above.
            | "save-space!" | "load-space!" | "query-act"
            // Stage 5a LSM-tiered ACT base: `attach-act-base!`/`detach-act-base!`
            // mutate the space's tiering (attach/drop the immutable base + clear
            // tombstones); `compact-space!` rewrites the on-disk base and folds the
            // overlay into it. All change the atom space and/or filesystem state, so
            // they must re-execute, never serve a memoized result — same rationale.
            | "attach-act-base!" | "detach-act-base!" | "compact-space!"
    )
}

thread_local! {
    /// Thread-local evaluation memoization cache.
    ///
    /// Key: content hash of the S-expression (via `hash_value()`)
    /// Value: cached evaluation results (SmallVec avoids heap for ≤4 results)
    ///
    /// 16384 entries × ~40 bytes avg = ~640 KB per thread. LRU eviction bounds memory.
    /// Doubled from 8192 (sweet spot found via benchmarking) to reduce eviction
    /// churn for Robot's deep recursive PLN inference. Larger sizes (32768)
    /// cause LRU lookup overhead that exceeds the benefit.
    /// Each entry stores (query_gen, mutation_epoch, scope_gen, results) so lookups can
    /// validate freshness, scope visibility, and cross-query isolation without
    /// clearing the entire cache on every mutation or branch transition.
    /// The query_gen is bumped at each top-level `!` so entries from prior
    /// top-level queries are naturally invalidated (LRU reclaims them lazily).
    static EVAL_MEMO: RefCell<LruCache<u64, (u64, u64, u64, SmallVec<[MettaValue; 4]>), IdentityU64BuildHasher>> =
        RefCell::new(LruCache::with_hasher(NonZeroUsize::new(16384).expect("non-zero"), IdentityU64BuildHasher));

    /// Mutation epoch counter for cache correctness.
    ///
    /// Incremented on every impure operation (add-atom, remove-atom,
    /// change-state!, new-state, println!, bind!, new-space). When a
    /// `MemoizeResult` continuation fires, the result is only cached if the
    /// epoch hasn't advanced since the continuation was pushed — ensuring
    /// that functions which transitively trigger side effects are not
    /// incorrectly memoized.
    static MUTATION_EPOCH: Cell<u64> = const { Cell::new(0) };

    /// Monotonically increasing scope generation for cache isolation between
    /// nondeterministic branches. Incremented at each branch boundary.
    static CACHE_GENERATION: Cell<u64> = const { Cell::new(0) };

    /// Watermark: the generation at the most recent fork point.
    /// Entries with scope_gen <= watermark are pre-fork (visible to all branches).
    static SCOPE_WATERMARK: Cell<u64> = const { Cell::new(0) };

    /// Fast-path flag: true when inside a nondeterministic fork.
    /// When false, is_scope_visible() returns true immediately (no overhead).
    static FORK_ACTIVE: Cell<bool> = const { Cell::new(false) };

    /// Stack of watermarks for nested forks.
    static WATERMARK_STACK: RefCell<SmallVec<[u64; 4]>> = RefCell::new(SmallVec::new());

    /// Query generation — bumped at every top-level `!` evaluation boundary.
    ///
    /// Used to invalidate EVAL_MEMO and MATCH_RESULT_CACHE across top-level
    /// queries without bulk-clearing them. Entries carry the query_gen at
    /// insertion time; lookups reject entries whose stored gen differs from
    /// the current thread-local gen. LRU eviction reclaims stale entries
    /// lazily, preserving within-query cache utility.
    static QUERY_GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// Returns the current query generation for this thread.
///
/// Bumped at top-level `!` boundaries (see eval_loop.rs). Cache entries
/// tagged with a different query_gen are from a prior top-level query
/// and must not be served to the current query.
#[inline]
pub fn query_generation() -> u64 {
    QUERY_GENERATION.with(|g| g.get())
}

/// Increment the query generation.
///
/// Called at the start of each top-level `!` evaluation (when
/// `!is_resuming`). Invalidates all EVAL_MEMO and MATCH_RESULT_CACHE
/// entries from previous top-level queries without touching LRU state.
#[inline]
pub fn increment_query_generation() {
    QUERY_GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
}

/// Returns the current mutation epoch for this thread.
#[inline]
pub fn mutation_epoch() -> u64 {
    MUTATION_EPOCH.with(|e| e.get())
}

/// Increments the mutation epoch, invalidating any in-flight memoization
/// guards that were recorded before this point.
#[inline]
pub fn increment_mutation_epoch() {
    MUTATION_EPOCH.with(|e| e.set(e.get().wrapping_add(1)));
}

/// Restore the mutation epoch to a saved value.
/// Used by `ProcessRuleMatches` to isolate sequential nondeterministic branches:
/// each branch starts with the pre-fork epoch so that side effects from branch N
/// don't invalidate SubgoalTable/eval_memo entries for branch N+1.
#[inline]
pub fn set_mutation_epoch(epoch: u64) {
    MUTATION_EPOCH.with(|e| e.set(epoch));
}

// ============================================================================
// Scope Generation — Cache Isolation for Nondeterministic Branches
// ============================================================================
//
// Each nondeterministic fork (ProcessRuleMatches) creates a scope boundary.
// Cache entries tagged with a generation > watermark AND != current generation
// are from a sibling branch and must not be visible. Entries with generation
// <= watermark were created before the fork and are visible to all branches.

/// Returns the current scope generation for this thread.
#[inline]
pub fn cache_generation() -> u64 {
    CACHE_GENERATION.with(|g| g.get())
}

/// Check whether a cache entry with `entry_gen` is visible in the current scope.
///
/// Visible if:
/// - Not inside a fork (fast path), OR
/// - Entry was created before the fork (entry_gen <= watermark), OR
/// - Entry was created in the current branch (entry_gen == current generation)
#[inline(always)]
pub fn is_scope_visible(entry_gen: u64) -> bool {
    if !FORK_ACTIVE.with(|f| f.get()) {
        return true;
    }
    let watermark = SCOPE_WATERMARK.with(|w| w.get());
    let current = CACHE_GENERATION.with(|g| g.get());
    entry_gen <= watermark || entry_gen == current
}

/// Enter a nondeterministic fork scope. Pushes the current generation as a
/// watermark and advances the generation for the first branch.
///
/// Returns the pre-fork generation (to be stored in the continuation and
/// passed to `leave_fork_scope` on completion).
#[inline]
pub fn enter_fork_scope() -> u64 {
    let pre_fork = CACHE_GENERATION.with(|g| g.get());
    WATERMARK_STACK.with(|stack| stack.borrow_mut().push(pre_fork));
    SCOPE_WATERMARK.with(|w| w.set(pre_fork));
    FORK_ACTIVE.with(|f| f.set(true));
    CACHE_GENERATION.with(|g| g.set(pre_fork + 1));
    pre_fork
}

/// Advance the scope generation for the next branch within a fork.
/// Called between sequential nondeterministic branches.
#[inline]
pub fn next_branch_scope() {
    CACHE_GENERATION.with(|g| g.set(g.get() + 1));
}

/// Leave a nondeterministic fork scope. Pops the watermark stack and, if no
/// outer fork remains, clears the FORK_ACTIVE flag for fast-path bypass.
#[inline]
pub fn leave_fork_scope(_pre_fork_gen: u64) {
    WATERMARK_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        stack.pop();
        if let Some(&top) = stack.last() {
            SCOPE_WATERMARK.with(|w| w.set(top));
        } else {
            FORK_ACTIVE.with(|f| f.set(false));
        }
    });
}

/// Check if an S-expression should be memoized (pure head, ≥2 items,
/// no free variables).
#[inline]
pub fn should_memoize<V: MettaValueTrait>(value: &V) -> bool {
    if let Some(items) = value.as_sexpr() {
        if items.is_empty() {
            return false;
        }
        if value.has_variables_fast() {
            return false;
        }
        if let Some(head) = items.first().and_then(|h| h.as_atom()) {
            if is_impure_head(head) {
                return false;
            }
            if (head == "!" || head == "eval" || head == "capture") && items.len() == 2 {
                return should_memoize(&items[1]);
            }
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// Environment-aware memoization check that also rejects expressions
/// whose head has an inferred monadic return type (transitively impure).
///
/// This catches user-defined functions like `(treat_step "tt")` that
/// call `println!` in their body — Phase 10 inference propagates the
/// `(IO Unit)` return type from `println!` through the function chain,
/// including through let*/let/if/case/chain control flow forms.
pub fn should_memoize_with_env(
    value: &MettaValue,
    env: &crate::backend::eval::trampoline::MettaEnvironment,
) -> bool {
    if !should_memoize(value) {
        return false;
    }
    if let Some(items) = value.as_sexpr() {
        if let Some(head) = items.first().and_then(|h| h.as_atom()) {
            // Inferred monadic return type (from Phase 10).
            // O(1) bloom filter check + DashMap lookup.
            if env.has_inferred_type(head) {
                let inferred_types = env.get_inferred_fn_types(head);
                for t in &inferred_types {
                    if t.is_monadic_type() || t.is_arrow_returning_monadic() {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Look up cached evaluation results for an expression hash.
///
/// Returns `Some(results)` on cache hit. The caller should push a Resume
/// with these results instead of evaluating the expression.
///
/// GC safety: Before accessing cached values, checks the GC sweep epoch.
/// If GC freed slab slots since the cache was populated, the entire cache
/// is cleared (returning `None`) to avoid use-after-free on stale pointers.
/// Fold the collapse-bind tracked-vars state into the eval-memo key.
///
/// An expression's evaluation result depends on the active collapse-bind
/// `tracked_vars` (they govern which match bindings survive projection, and
/// thus the numeric result threaded through e.g. PLN's truth-value formulas).
/// Results computed under DIFFERENT collapse-bind contexts must therefore NOT
/// be shared. Without this, a bare `(reduce X)` evaluated with NO collapse-bind
/// active poisons a later `(collapse (reduce X))` evaluated WITH collapse-bind
/// active — exactly PLN's `?` macro `(progn (reduce $term) <fold>)` double-reduce,
/// which collapsed the mother-branch confidence to 0.0 (Direct.metta phantom).
#[inline]
pub fn eval_memo_key(expr_hash: u64, tracked_key: u64) -> u64 {
    expr_hash ^ tracked_key.wrapping_mul(0x9e3779b97f4a7c15)
}

#[inline]
/// DIAGNOSTIC kill-switch (env `METTATRON_DISABLE_EVAL_CACHES=1`): when set, the
/// value-bearing eval-scoped memos (EVAL_MEMO + MATCH_RESULT_CACHE) always MISS,
/// forcing re-evaluation / re-matching. Dormant by default (unset ⇒ byte-identical).
/// A memo miss is always semantically safe (recompute), so this is correctness-
/// preserving (only slower). Used to discriminate whether a residual DEDICATED=1
/// wrong-subset corruption flows through a STALE MEMO VALUE (a worker memo entry whose
/// cached σ-`Addr` was swept+reused in the no-park window) vs VALUE_HASH_CACHE (now
/// epoch-healed), the bloom (its own kill-switch), or a MISSED ROOT (none of the caches
/// — which would redirect the diagnosis to rooting).
fn eval_caches_disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("METTATRON_DISABLE_EVAL_CACHES")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

thread_local! {
    /// The `gc_sweep_epoch` this thread's EVAL_MEMO + MATCH_RESULT_CACHE were last known
    /// coherent for. Index `Addr`-reuse after a dedicated-collector sweep can make a memo
    /// entry's cached σ-`Addr` stale (the slot was bump-reused for new content), so when
    /// the global epoch advances (the index collector bumps it post-sweep — slab parity),
    /// the next memo lookup on THIS thread clears both memos before returning. This is the
    /// cross-thread LAZY self-heal that reaches work-pool workers which held their
    /// EvalGuard across a witness-gated cycle WITHOUT ever hitting a park safepoint (the
    /// VM/JIT poll cadence) — the no-park window that both the eager sweep-thread clear
    /// (index_heap.rs:2128) and the park/teardown hygiene miss. (VALUE_HASH_CACHE already
    /// self-heals this way; these two value-memos did NOT — the residual ~3.75% DEDICATED=1
    /// wrong-subset corruption, confirmed by the eval-caches-disabled discriminator 0/40.)
    #[cfg(feature = "index-gc")]
    static EVAL_CACHES_GC_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Clear EVAL_MEMO + MATCH_RESULT_CACHE on THIS thread if a GC sweep advanced the epoch
/// since the last check — slab-parity with `metta_value::ensure_value_hash_cache_epoch_current`.
/// A memo MISS is always safe (recompute), so this is correctness-by-construction.
/// `#[cfg(index-gc)]`: the slab build protects these memos via query-gen/mutation-epoch (NOT
/// gc_sweep_epoch), so adding a gc-epoch clear there would change slab behaviour; gating to
/// the index build keeps the slab path byte-identical.
#[cfg(feature = "index-gc")]
#[inline]
fn ensure_eval_caches_gc_epoch_current() {
    let current = crate::backend::models::gc_allocator::gc_sweep_epoch();
    EVAL_CACHES_GC_EPOCH.with(|e| {
        if e.get() != current {
            EVAL_MEMO.with(|c| c.borrow_mut().clear());
            MATCH_RESULT_CACHE.with(|c| c.borrow_mut().clear());
            e.set(current);
        }
    });
}

pub fn eval_memo_get(expr_hash: u64, tracked_key: u64) -> Option<Vec<MettaValue>> {
    if eval_caches_disabled() {
        return None;
    }
    #[cfg(feature = "index-gc")]
    ensure_eval_caches_gc_epoch_current();
    let expr_hash = eval_memo_key(expr_hash, tracked_key);
    let current_epoch = mutation_epoch();
    let current_query_gen = query_generation();
    EVAL_MEMO.with(|memo_cell| {
        let mut memo = memo_cell.borrow_mut();
        // get_mut: single hash lookup for the hot path (valid hit).
        // NLL allows pop after the if-let borrow ends.
        let mut stale = false;
        if let Some((cached_query_gen, cached_epoch, cached_gen, entries)) =
            memo.get_mut(&expr_hash)
        {
            if *cached_query_gen == current_query_gen
                && *cached_epoch == current_epoch
                && is_scope_visible(*cached_gen)
            {
                return Some(entries.to_vec());
            }
            stale = true;
        }
        if stale {
            memo.pop(&expr_hash);
        }
        None
    })
}

/// Store evaluation results in the memo cache.
#[inline]
pub fn eval_memo_put(expr_hash: u64, tracked_key: u64, results: &[MettaValue]) {
    let expr_hash = eval_memo_key(expr_hash, tracked_key);
    let query_gen = query_generation();
    let epoch = mutation_epoch();
    let gen = cache_generation();
    let entries: SmallVec<[MettaValue; 4]> = results.iter().copied().collect();
    EVAL_MEMO.with(|memo_cell| {
        memo_cell
            .borrow_mut()
            .put(expr_hash, (query_gen, epoch, gen, entries));
    });
}

/// Collect all MettaValue roots from the eval memo cache.
///
/// Called during GC safepoint root collection to ensure cached values survive
/// the mark-sweep cycle.
pub fn collect_eval_memo_roots(out: &mut Vec<MettaValue>) {
    EVAL_MEMO.with(|memo_cell| {
        let memo = memo_cell.borrow();
        for (_hash, (_query_gen, _epoch, _gen, entries)) in memo.iter() {
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
// Caches the output of `try_match_all_rules` — the set of matching
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
type MatchResultEntry =
    SmallVec<[(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>); 4]>;

thread_local! {
    /// Thread-local rule-match result cache.
    ///
    /// Key: expression content hash (via `hash_value()`)
    /// Value: (query_gen, rule_epoch, mutation_epoch, arity, cached match results)
    ///
    /// The query_gen is bumped at each top-level `!` so entries from prior
    /// top-level queries are naturally invalidated (LRU reclaims them lazily).
    /// The mutation_epoch catches add-atom / remove-atom invalidations within
    /// the same top-level query. The rule_epoch catches environment-wide rule
    /// changes (e.g., module loads).
    ///
    /// 4096 entries × ~128 bytes avg = ~512 KB per thread. LRU eviction bounds memory.
    static MATCH_RESULT_CACHE: RefCell<LruCache<u64, (u64, u64, u64, usize, MatchResultEntry), IdentityU64BuildHasher>> =
        RefCell::new(LruCache::with_hasher(NonZeroUsize::new(4096).expect("non-zero"), IdentityU64BuildHasher));
}

/// Look up cached match results for an expression.
///
/// Returns `Some(results)` if the cache contains a valid entry for the given
/// expression hash at the current rule epoch. The caller should use these
/// results instead of calling `try_match_all_rules`.
///
/// GC safety: checks GC sweep epoch before access (via `check_gc_epoch()`).
#[inline]
pub fn match_result_get(
    expr_hash: u64,
    expr_arity: usize,
) -> Option<Vec<(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>)>> {
    if eval_caches_disabled() {
        return None;
    }
    #[cfg(feature = "index-gc")]
    ensure_eval_caches_gc_epoch_current();
    // I-9: Epoch check removed — deterministic GC keeps state garbage-free.
    let current_rule_epoch = RULE_EPOCH.load(Ordering::Acquire);
    let current_mutation_epoch = mutation_epoch();
    let current_query_gen = query_generation();
    MATCH_RESULT_CACHE.with(|cache_cell| {
        let mut cache = cache_cell.borrow_mut();
        if let Some((
            stored_query_gen,
            stored_rule_epoch,
            stored_mutation_epoch,
            stored_arity,
            entries,
        )) = cache.get(&expr_hash)
        {
            if *stored_query_gen == current_query_gen
                && *stored_rule_epoch == current_rule_epoch
                && *stored_mutation_epoch == current_mutation_epoch
                && *stored_arity == expr_arity
            {
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
    let current_rule_epoch = RULE_EPOCH.load(Ordering::Acquire);
    let current_mutation_epoch = mutation_epoch();
    let current_query_gen = query_generation();
    let entries: MatchResultEntry = results.iter().cloned().collect();
    MATCH_RESULT_CACHE.with(|cache_cell| {
        cache_cell.borrow_mut().put(
            expr_hash,
            (
                current_query_gen,
                current_rule_epoch,
                current_mutation_epoch,
                expr_arity,
                entries,
            ),
        );
    });
}

/// Collect all MettaValue roots from the match result cache.
///
/// Called during GC safepoint root collection to ensure cached values survive
/// the mark-sweep cycle.
pub fn collect_match_result_roots(out: &mut Vec<MettaValue>) {
    MATCH_RESULT_CACHE.with(|cache_cell| {
        let cache = cache_cell.borrow();
        for (_hash, (_query_gen, _rule_epoch, _mutation_epoch, _arity, entries)) in cache.iter() {
            for (rhs, bindings, rhs_type) in entries.iter() {
                out.push(*rhs);
                for (_scope, _name, val) in bindings.iter_full() {
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
// true, `try_match_all_rules` can skip `hash_value()` (7.48% CPU)
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
    /// PT-canonical rule-body preservation gate. True iff ANY candidate
    /// rule's RHS top-level head is in `is_lazy_body_form` (e.g. `add-atom`,
    /// `quote`, `if`, `case`, `let`, `chain`, `match`, ...). When true, the
    /// dispatcher skips Step-2 pre-evaluation of the call's args so that the
    /// rule body sees them verbatim. Required for PLN's `=>` macro pattern.
    pub any_rule_wants_lazy_args: bool,
    /// PT-canonical meta-typed signature gate. True iff ANY candidate rule
    /// for this (head, arity) has `RuleEntry::lhs_head_all_meta_typed=true`
    /// — i.e. the head has at least one declared arrow type where all args
    /// AND the return type are meta-types. When true, the rule-firing path
    /// returns the substituted RHS VERBATIM (no re-evaluation), matching
    /// PeTTa's data-in / data-out semantic for `(-> Expression Atom)`-class
    /// predicates. Required for PLN's `(? $term)` pattern.
    pub lhs_head_all_meta_typed: bool,
    /// Phase 1 cut-barrier: true iff ANY candidate rule for this (head, arity)
    /// has `RuleEntry::body_contains_cut = true` — i.e. its RHS lexically
    /// contains an applied `(cut ...)` (quote-aware, precomputed at add time).
    /// O(1) precomputed aggregate of the per-rule flag, mirroring
    /// `any_rule_wants_lazy_args`. `dispatch_rule_matches` independently
    /// re-confirms cut presence on the INSTANTIATED matched RHS (which is what
    /// actually executes) before opening a barrier; this aggregate is the
    /// canonical per-(head,arity) store and is consulted as an O(1) fast
    /// precheck. PLN's `=>` macro (`(= (=> (cons , $args) ...) (progn (cut)
    /// ...))`) sets it.
    pub any_rule_body_contains_cut: bool,
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
// After apply_bindings produces an instantiated RHS, many values are
// already in normal form (data tuples, ground atoms) and the trampoline just
// returns them unchanged — wasting a pop/push/dispatch cycle per value.
//
// This module provides a bounded static predicate that identifies such values
// without speculation. Combined with the existing NORMAL_FORM_BLOOM filter
// (Phase 9.5), this short-circuits the trampoline for cold and hot paths.

/// True iff `head` names an embedded kernel instruction per MeTTa spec §06.3.4.
///
/// The 12 kernel instructions (plus aliases): `eval`, `evalc`, `chain`, `unify`,
/// `cons-atom`, `decons-atom`, `function`, `return`, `collapse-bind`,
/// `superpose-bind`, `metta`, `call-native`, `context-space`.
///
/// **Use case**: chain/let*/etc. must dispatch only ONE kernel step on their
/// expression arg (§06.6.3 E-CHAIN-SUBST-DONE). If the expr's head is a kernel
/// op, that step runs (evaluating eval/chain/unify nests). If the head is
/// anything else — a grounded op like `+`, a user-defined rule head, or a
/// literal — the expr is treated as DATA and bound as-is without reduction.
/// This prevents combinatorial explosion in recursive inference chains.
///
/// MeTTaTron also treats `evalc` as a synonym of `eval` in the kernel; both
/// route through the eval handler at `eval_loop.rs:3063`.
#[inline(always)]
pub(crate) fn is_embedded_kernel_op(head: &str) -> bool {
    matches!(
        head,
        // Spec §06.3.6 embedded kernel ops
        // PT-canonical (2026-05-21): `cons`/`decons` are PeTTa aliases
        // (PeTTa src/metta.pl:142-143).
        "eval" | "capture" | "evalc" | "chain" | "unify"
        | "cons-atom" | "decons-atom" | "cons" | "decons"
        | "function" | "return"
        | "collapse-bind" | "superpose-bind"
        | "metta" | "call-native" | "context-space"
        // MeTTaTron-specific kernel-level ops: pure transforms, idempotent,
        // bound to terminate. Treated as kernel ops for spec §06.6.3 purposes
        // so chain dispatches one kernel step rather than binding the
        // unreduced expression as data. Without this, PLN's
        //     (chain (ground-with-bindings $term $binds) $grounded body)
        // never invokes the handler — `$grounded` binds to the literal
        // S-expression and downstream `(eval $grounded)` / `freeze-tuple`
        // operations see unreduced data instead of the substituted template.
        | "ground-with-bindings" | "freeze-tuple"
        // Plan 1 (2026-05-06): higher-order tuple ops are pure, bounded
        // transforms over finite tuples (sexpr.rs:1100/1154/1216 dispatch
        // them to native StartMapAtom/StartFilterAtom/StartFoldlAtom
        // iterators). HE bisimulates these via stdlib rule lookup at
        // hyperon-experimental/lib/src/metta/runner/stdlib/stdlib.metta:455,
        // 475, 495 — its `chain` evaluates them eagerly via the rule.
        // Without this MeTTaTron's chain takes the data branch and
        // substitutes the literal expression into body, breaking PLN's
        // `?` macro shape (`Direct.metta:32-59`):
        //     (chain (foldl-atom (filter-atom ...) ...) $evidence
        //       (let (stv $s $c) $evidence
        //         (if (== $c 0.0) (empty)
        //             (freeze-tuple $grounded $evidence))))
        // — `$evidence` would carry the unreduced foldl-atom into
        // freeze-tuple's second arg.
        | "map-atom" | "filter-atom" | "foldl-atom"
        // Plan 1 audit follow-up (2026-05-06): same shape as the higher-
        // order tuple-op family. sort-tuple (`sexpr.rs:1539` →
        // `StartSortTuple` at `eval_loop.rs:3458`) and best-candidate
        // (`sexpr.rs:1597` → `StartBestCandidate` at `eval_loop.rs:3507`)
        // are native iterators producing tuple/value results that chain
        // bodies may structurally destructure. Adding for consistency.
        | "sort-tuple" | "best-candidate"
    )
}

/// Union of all head symbols that are reducible in `eval_sexpr_step_generic`
/// (special forms) and `has_grounded_op` (arithmetic/comparison ops).
///
/// An S-expression `(head ...)` is NOT in normal form if `head` is in this set,
/// because the trampoline will dispatch it for evaluation.
///
/// Maintained in sync with `eval_sexpr_step_generic` match arms,
/// `GROUNDED_OPS`, `SPECIAL_FORMS_REDISPATCH`, and `EAGER_SPECIAL_FORMS`
/// via the `reducible_heads_covers_all_known_sets` test below.
#[inline(always)]
pub(crate) fn is_reducible_head(head: &str) -> bool {
    matches!(
        head,
        // === Special forms (eval_sexpr_step_generic match arms) ===
        "=" | "!" | "quote" | "unquote" | "noreduce" | "noeval"
        | "if" | "if-reducible" | "if-equal"
        | "error" | "Error" | "is-error" | "catch" | "if-error"
        | "eval" | "capture" | "reduce" | "progn" | "function" | "return" | "chain"
        | "match" | "match-or" | "case"
        | "switch" | "switch-minimal" | "switch-internal"
        | "let" | "let*" | "unify" | "sealed" | "atom-subst"
        | ":<" | ":"
        | "get-type" | "check-type" | "validate-atom" | "get-type-space"
        | "is-function" | "type-cast" | "metta"
        | "match-types" | "match-type-or" | "first-from-pair"
        | "map-atom" | "filter-atom" | "foldl-atom"
        // PT-canonical (2026-05-21): `cons`/`decons` are PeTTa aliases.
        | "car-atom" | "cdr-atom" | "cons-atom" | "decons-atom" | "size-atom"
        | "cons" | "decons"
        | "max-atom" | "min-atom" | "index-atom"
        | "tuple-concat" | "tuple-count" | "without" | "element-of"
        | "range" | "reverse-atom" | "flatten-atom" | "zip-atom"
        | "take-atom" | "drop-atom" | "sort-tuple" | "best-candidate"
        // PeTTa-compatible helpers (Arm B overridable list ops)
        | "is-member" | "append" | "length" | "exclude-item" | "msort" | "cut"
        | "struct-unique-atom"
        | "new-space" | "add-atom" | "remove-atom"
        | "collapse" | "collapse-bind" | "superpose" | "amb" | "ground-with-bindings" | "freeze-tuple"
        | "guard" | "commit" | "backtrack"
        | "get-atoms"
        | "new-state" | "get-state" | "change-state!"
        | "new-memo" | "memo" | "memo-first" | "clear-memo!" | "memo-stats"
        | "bind!" | "println!" | "print-alternatives!" | "trace!" | "nop"
        | "repr" | "format-args"
        | "empty" | "get-metatype"
        | "include" | "register-module!" | "import!" | "git-import!" | "git-module!" | "mod-space!" | "print-mods!" | "get-modules"
        | "exec" | "coalg" | "lookup" | "rulify"
        | "=alpha"
        | "unique-atom" | "alpha-unique-atom" | "union-atom" | "intersection-atom" | "subtraction-atom"
        | "unique" | "union" | "intersection" | "subtraction"
        | "test"
        | "assertEqual" | "assertAlphaEqual"
        | "assertEqualMsg" | "assertAlphaEqualMsg"
        | "assertEqualToResult" | "assertAlphaEqualToResult"
        | "assertEqualToResultMsg" | "assertAlphaEqualToResultMsg"
        | "pragma!"
        // === Grounded operations ===
        // Plan 1 audit (2026-05-06): mod/negate are bytecode/JIT synonyms.
        | "+" | "-" | "*" | "/" | "%" | "mod" | "negate" | "min" | "max"
        | "<" | "<=" | ">" | ">=" | "==" | "!="
        | "and" | "or" | "not" | "xor"
        | "/safe" | "clamp"
        | "pow" | "abs" | "floor" | "ceil" | "round" | "sqrt"
        | "floor-div"
        | "pow-math" | "sqrt-math" | "abs-math" | "log-math" | "trunc-math"
        | "ceil-math" | "floor-math" | "round-math"
        | "sin-math" | "asin-math" | "cos-math" | "acos-math"
        | "tan-math" | "atan-math"
        | "isnan-math" | "isinf-math"
        // === String operations (Workstream X.5a) ===
        | "stringToChars"
        // === String operations (T06/060 — HE-aligned) ===
        | "sort-strings"
        // === Meta / polymorphic operations (T06/037 — HE-aligned) ===
        | "id"
        // === JSON module (T07/019-020, HE-aligned: `json` builtin) ===
        | "json-encode" | "json-decode"
        // === FileIO module (T07/021, HE-aligned: `fileio` builtin) ===
        | "file-open!" | "file-read-to-string!" | "file-write!"
        | "file-seek!" | "file-read-exact!" | "file-get-size!"
        // === Random module (T07/022, HE-aligned: `random` builtin) ===
        | "new-random-generator" | "random-int" | "random-float"
        | "set-random-seed" | "reset-random-generator" | "flip"
    )
}

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
    env: &crate::backend::environment::core::GenericEnvironment<
        V,
        impl MettaValueFactory<V> + Copy + Clone,
    >,
    max_depth: u8,
) -> bool {
    // S-expressions: check head + children
    if let Some(items) = value.as_sexpr() {
        if items.is_empty() {
            return true;
        }
        // Head must be a plain atom (not a variable or nested expr)
        let head = match items[0].as_atom() {
            Some(name) => name,
            None => return false,
        };
        // Head must not be variable, special form, or grounded op
        if head.starts_with('$') {
            return false;
        }
        if is_reducible_head(head) {
            return false;
        }
        // Head must not have user-defined rules.
        // Uses rule-only bloom (not atom bloom) to avoid false positives
        // from data constructors added via add-atom (e.g., (Type "$a")).
        if env.may_have_rule_head(head, items.len() - 1) {
            return false;
        }
        // Check children within depth budget
        return items[1..]
            .iter()
            .all(|child| is_child_normal_form(child, env, max_depth));
    }
    // Atoms: normal form UNLESS &self (resolves to space) or
    // starts with $ (variable). Tokenizer bindings (bind!) are
    // rare and caught by bloom filter on second eval.
    if let Some(name) = value.as_atom() {
        return name != "&self" && !name.starts_with('$');
    }
    // Conjunctions: need eval_conjunction_step_generic
    if value.as_conjunction().is_some() {
        return false;
    }
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
    env: &crate::backend::environment::core::GenericEnvironment<
        V,
        impl MettaValueFactory<V> + Copy + Clone,
    >,
    max_depth: u8,
) -> bool {
    // S-expression children: recurse with depth budget
    if child.as_sexpr().is_some() {
        if max_depth == 0 {
            return false;
        }
        return is_normal_form_bounded(child, env, max_depth - 1);
    }
    // Atoms: OK unless &self or variable
    if let Some(name) = child.as_atom() {
        return name != "&self" && !name.starts_with('$');
    }
    // Conjunctions: reducible
    if child.as_conjunction().is_some() {
        return false;
    }
    // All other types (ground, error, empty, type, quoted): normal form
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reducible_heads_covers_all_known_sets() {
        use crate::backend::eval::helpers::{
            is_eager_special_form, is_grounded_op, needs_special_form_redispatch,
        };

        // Check GROUNDED_OPS coverage
        let grounded_ops = [
            "+",
            "-",
            "*",
            "/",
            "%",
            "min",
            "max",
            "pow",
            "abs",
            "floor",
            "ceil",
            "round",
            "sqrt",
            "floor-div",
            "pow-math",
            "sqrt-math",
            "abs-math",
            "log-math",
            "trunc-math",
            "ceil-math",
            "floor-math",
            "round-math",
            "sin-math",
            "asin-math",
            "cos-math",
            "acos-math",
            "tan-math",
            "atan-math",
            "isnan-math",
            "isinf-math",
            "<",
            "<=",
            ">",
            ">=",
            "==",
            "!=",
            "not",
            "and",
            "or",
            "xor",
            "get-type",
            "get-metatype",
            "validate-atom",
            "get-type-space",
            "car-atom",
            "cdr-atom",
            "cons-atom",
            "decons-atom",
            "size-atom",
            "max-atom",
            "min-atom",
            "index-atom",
            "tuple-concat",
            "tuple-count",
            "without",
            "element-of",
            "range",
            "reverse-atom",
            "flatten-atom",
            "zip-atom",
            "take-atom",
            "drop-atom",
            "sort-tuple",
            "best-candidate",
            "/safe",
            "clamp",
            "stringToChars",
            "sort-strings",
            "id",
            // Bare set-op aliases (T06/108-111). Listed in is_grounded_op so
            // nested calls trigger pre-evaluation; the bare-form dispatch in
            // step/sexpr.rs desugars them to `(superpose (op-atom (collapse arg)…))`.
            "unique",
            "union",
            "intersection",
            "subtraction",
        ];
        for op in &grounded_ops {
            assert!(
                is_grounded_op(op),
                "GROUNDED_OPS has '{}' but is_grounded_op doesn't recognize it",
                op
            );
            assert!(
                is_reducible_head(op),
                "REDUCIBLE_HEADS missing grounded op: {}",
                op
            );
        }

        // Check SPECIAL_FORMS_REDISPATCH coverage
        let special_forms = [
            "map-atom",
            "filter-atom",
            "foldl-atom",
            "sort-tuple",
            "best-candidate",
            "if",
            "if-equal",
            "if-reducible",
            "case",
            "switch",
            "switch-minimal",
            "switch-internal",
            "let",
            "let*",
            "unify",
            "chain",
            "function",
            "return",
            "sealed",
            "atom-subst",
            "match",
            "match-or",
            "catch",
            "is-error",
            "eval",
            "capture",
            "quote",
            "unquote",
            "collapse",
            "collapse-bind",
            "amb",
            "guard",
            "ground-with-bindings",
            "freeze-tuple",
            "new-state",
            "get-state",
            "change-state!",
            "println!",
            "print-alternatives!",
            "trace!",
            "unique-atom",
            "union-atom",
            "intersection-atom",
            "subtraction-atom",
            // Bare set-op aliases (T06/108-111) — redispatch path must
            // recognize the bare names so the wrapped form
            // `(superpose (op-atom (collapse arg)…))` re-enters the dispatch.
            "unique",
            "union",
            "intersection",
            "subtraction",
            "=alpha",
            "match-types",
            "assertEqual",
            "assertAlphaEqual",
            "assertEqualMsg",
            "assertAlphaEqualMsg",
            "assertEqualToResult",
            "assertAlphaEqualToResult",
            "assertEqualToResultMsg",
            "assertAlphaEqualToResultMsg",
        ];
        for op in &special_forms {
            assert!(needs_special_form_redispatch(op), "SPECIAL_FORMS_REDISPATCH has '{}' but needs_special_form_redispatch doesn't recognize it", op);
            assert!(
                is_reducible_head(op),
                "REDUCIBLE_HEADS missing special form: {}",
                op
            );
        }

        // Check EAGER_SPECIAL_FORMS coverage
        let eager_forms = [
            "map-atom",
            "filter-atom",
            "foldl-atom",
            "sort-tuple",
            "best-candidate",
            "eval",
            "capture",
            "unquote",
            "collapse",
            "collapse-bind",
            "superpose",
            "get-state",
            "catch",
            "get-metatype",
            "validate-atom",
            "get-type-space",
            "repr",
            "format-args",
            "unique-atom",
            "union-atom",
            "intersection-atom",
            "subtraction-atom",
            // Bare set-op aliases (T06/108-111) — eager so nested usage is
            // pre-evaluated before being passed to outer rules.
            "unique",
            "union",
            "intersection",
            "subtraction",
            "=alpha",
        ];
        for op in &eager_forms {
            assert!(
                is_eager_special_form(op),
                "EAGER_SPECIAL_FORMS has '{}' but is_eager_special_form doesn't recognize it",
                op
            );
            assert!(
                is_reducible_head(op),
                "REDUCIBLE_HEADS missing eager form: {}",
                op
            );
        }

        // Check has_grounded_op coverage
        let generic_grounded = [
            "+",
            "-",
            "*",
            "/",
            "%",
            "min",
            "max",
            "<",
            "<=",
            ">",
            ">=",
            "==",
            "!=",
            "and",
            "or",
            "not",
            "xor",
            "/safe",
            "clamp",
            "stringToChars",
            "sort-strings",
            "id",
        ];
        for op in &generic_grounded {
            assert!(
                crate::backend::grounded::has_grounded_op(op),
                "has_grounded_op doesn't recognize: {}",
                op
            );
            assert!(
                is_reducible_head(op),
                "REDUCIBLE_HEADS missing generic grounded op: {}",
                op
            );
        }
    }

    /// Canary regression test for the writer-side structural guard in
    /// `memoize_normal_form`. Replaces the H12 `debug_assert!` panic.
    ///
    /// VM/JIT dispatch paths legitimately pass reducible-head expressions
    /// through `op_dispatch_rules` → `memoize_normal_form` (e.g.,
    /// `(map-atom ...)`, `(first-from-pair ...)`). The guard at lines
    /// 117-141 silently rejects such values and increments
    /// `NORMAL_FORM_REJECT_COUNT`. This test confirms:
    ///   1. The rejection counter increments on a reducible-head call.
    ///   2. The bloom does NOT contain the rejected value (reader returns
    ///      `false`).
    ///
    /// If a future regression re-introduces the H12 invariant violation
    /// (e.g., the writer-side guard is removed), the bloom would receive
    /// a poisoned entry but the reader's structural prefilter would still
    /// reject it on read — so this test instead exercises the WRITER's
    /// rejection signal.
    #[test]
    fn canary_reducible_head_rejected_silently() {
        use crate::backend::models::{global_factory, MettaValueFactory};

        let pre = normal_form_reject_count();
        let factory = global_factory();
        let v = factory.sexpr(vec![factory.atom("map-atom"), factory.atom("$x")]);
        memoize_normal_form(&v);
        // Counter only increments in debug builds.
        #[cfg(debug_assertions)]
        assert!(
            normal_form_reject_count() > pre,
            "rejection counter should increment for reducible head"
        );
        // In both debug and release: bloom must NOT contain the value
        // (writer rejected; even if it didn't, the reader's structural
        // prefilter at line 86-98 would reject the lookup).
        assert!(
            !is_memoized_normal_form(&v),
            "reducible-head value must NOT be reported as in normal form"
        );
        // Sanity: a non-reducible-head value DOES enter the bloom and
        // is reported on lookup.
        let plain = factory.sexpr(vec![factory.atom("FooData"), factory.atom("a")]);
        memoize_normal_form(&plain);
        assert!(
            is_memoized_normal_form(&plain),
            "non-reducible-head value should be memoized"
        );
    }
}
