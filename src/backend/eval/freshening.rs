//! Variable Freshening for Space Operations
//!
//! Provides variable freshening (alpha-renaming) for MeTTa values to prevent
//! variable capture during bidirectional matching and get-atoms operations.
//!
//! ## MeTTa HE Semantics
//!
//! MeTTa HE calls `make_variables_unique()` on stored atoms before matching and
//! when returning atoms via `get-atoms`. This ensures cross-atom variable isolation:
//! two stored atoms `(foo $x)` and `(bar $x)` get independent freshened names so
//! binding `$x` in one doesn't affect the other.
//!
//! ## Implementation
//!
//! Uses the same iterative work-stack pattern as `seal_variables_iterative_generic`
//! from `bindings.rs`. Variables (`$`-prefixed atoms) are renamed to
//! `$__fr_{epoch}_{name}` where epoch is a globally unique counter. Non-variable
//! atoms (`&self`, `&kb`, `&stack`, literals) pass through unchanged.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use smallvec::SmallVec;

use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

/// Global counter for freshening epochs. Each call to `freshen_variables_generic`
/// gets a unique epoch to ensure cross-call variable isolation.
static FRESHEN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Work items for the iterative freshening algorithm.
enum FreshenWork<'a, V> {
    /// Process a value (check if variable, recurse into compounds)
    Process(&'a V),
    /// Build an S-expression from the top `count` items on the result stack
    BuildSExpr(usize),
    /// Build a conjunction from the top `count` items on the result stack
    BuildConjunction(usize),
}

/// Freshen (alpha-rename) all variables in a value.
///
/// Renames `$x` → `$__fr_{epoch}_x` for all `$`-prefixed atoms. Non-variable
/// atoms (including `&self`, `&kb`, `&stack`, `_` wildcards) pass through unchanged.
///
/// ## Fast Path
///
/// Returns the value unchanged (no allocation) if it contains no variables.
/// Use `has_variables_generic()` from `mork_forms.rs` to pre-check.
///
/// ## Epoch Isolation
///
/// Each call uses a unique epoch from `FRESHEN_COUNTER`, ensuring that variables
/// freshened in separate calls get distinct names even if the original names match.
pub fn freshen_variables_generic<V, F>(value: &V, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let epoch = FRESHEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    freshen_with_epoch(value, epoch, factory)
}

/// Allocate a unique freshening epoch from the global counter.
///
/// Used to mint per-invocation epochs at rule-match time so each rule
/// dispatch produces globally-distinct `$__fr_{epoch}_*` variable
/// names (mirrors HE's `CachingMapper::new(|v| v.make_unique())` at
/// `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/hyperon-space/src/index/trie.rs:262`).
#[inline]
pub fn allocate_epoch() -> u64 {
    FRESHEN_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Per-invocation rename context. See spec §03.3 (CachingMapper pattern).
///
/// Applies `$x → $__fr_{epoch}_x` renaming on demand. Each distinct bare
/// variable name is interned once per epoch via `intern_fresh_name`
/// (`FRESH_NAME_CACHE` at line 97); repeated renames hit the cache.
///
/// Used by `apply_bindings_with_rename_generic` to fuse rule-match
/// freshening with binding substitution into a single tree walk,
/// replacing the former three-walk sequence:
///   freshen_variables_with_epoch + freshen_bindings_keys_with_epoch
///   + apply_bindings_generic
///
/// Wildcards `_` and `$_` are preserved unchanged per MeTTaTron's
/// wildcard extension.
///
/// Owns a per-dispatch `atom_cache` that memoizes `factory.atom(renamed)`
/// results keyed on the interned `&'static str` returned by `rename()`.
/// Lifetime is bounded to the rule dispatch (the `CachingRename` instance),
/// so cached values stay rooted by the live work-stack throughout the
/// dispatch and the cache drops with it — no thread-local GC concerns.
/// Values are type-erased via `Box<dyn Any>` to keep the struct
/// non-generic at the API boundary; downcast on lookup is `O(1)`
/// (TypeId comparison + clone of the cached `V`).
pub struct CachingRename {
    epoch: u64,
    atom_cache: RefCell<HashMap<&'static str, Box<dyn Any>>>,
}

impl std::fmt::Debug for CachingRename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachingRename")
            .field("epoch", &self.epoch)
            .field("atom_cache_len", &self.atom_cache.borrow().len())
            .finish()
    }
}

impl CachingRename {
    #[inline]
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            atom_cache: RefCell::new(HashMap::with_capacity(8)),
        }
    }

    #[inline]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Rename `name` ($-prefixed, not wildcard) to its epoch-qualified
    /// fresh form. Returns a `&'static str` via the thread-local intern
    /// cache (amortized O(1) per unique `(epoch, bare_name)` pair).
    ///
    /// Already-freshened variables (`$__fr_*`) pass through unchanged
    /// to prevent compound nesting (`$__fr_84___fr_83_x`). Variable
    /// identity is preserved across rule invocations.
    ///
    /// Caller must ensure `name` is a $-prefixed variable and not `$_`.
    #[inline]
    pub fn rename(&self, name: &'static str) -> &'static str {
        if name.starts_with("$__fr_") {
            return name;
        }
        let bare = &name[1..]; // strip leading '$'; inherits 'static from name
        intern_fresh_name(self.epoch, bare)
    }

    /// Allocate (or retrieve from the per-dispatch cache) the freshened
    /// atom value `V` for `name`.
    ///
    /// Equivalent to `factory.atom(self.rename(name))`, but caches the
    /// resulting `V` keyed on the interned `&'static str`. Repeated
    /// freshenings of the same variable within one rule dispatch
    /// (e.g. `$x` appearing 5× in the RHS) bypass `factory.atom()`'s
    /// `alloc_str` + `alloc_value` slab allocations after the first hit,
    /// returning the cached `V` via cheap `Clone`. This is the dominant
    /// allocation-rate driver in HE-bisimilar per-invocation freshening
    /// (`engine.rs:818-849`); reducing it lowers the rate at which
    /// `committed_bytes` grows past `gc_threshold` and helps the safepoint
    /// alloc-delta path stay below `SAFEPOINT_ALLOC_THRESHOLD` per worker.
    ///
    /// Caller must ensure `name` is a $-prefixed variable and not `$_`.
    #[inline]
    pub fn fresh_atom<V, F>(&self, name: &'static str, factory: &F) -> V
    where
        V: MettaValueTrait + Clone + 'static,
        F: MettaValueFactory<V>,
    {
        let interned = self.rename(name);
        if let Some(boxed) = self.atom_cache.borrow().get(interned) {
            if let Some(v) = boxed.downcast_ref::<V>() {
                return v.clone();
            }
        }
        let value = factory.atom(interned);
        self.atom_cache
            .borrow_mut()
            .insert(interned, Box::new(value.clone()));
        value
    }
}

/// Freshen all variables in `value` under a caller-supplied `epoch`.
///
/// Unlike `freshen_variables_generic` which mints a fresh epoch per
/// call, this variant reuses the caller's epoch so multiple calls
/// within the same rule dispatch (LHS + RHS + bindings keys) share
/// variable identity. Mirrors HE's per-query `CachingMapper` which
/// caches the renaming within a single query invocation.
pub fn freshen_variables_with_epoch<V, F>(value: &V, epoch: u64, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    freshen_with_epoch(value, epoch, factory)
}

/// Thread-local cache of interned fresh names keyed on `(epoch, bare_name)`.
///
/// Amortizes the `format!("$__fr_{epoch}_{bare}")` + `alloc_str` cost
/// when a single rule match freshens the same variable name multiple
/// times (LHS pattern, bindings keys, RHS template all share the
/// rule's variable set). Because the epoch changes per match, entries
/// age out quickly — the cache bounds size, evicting oldest via simple
/// HashMap with periodic clear.
thread_local! {
    static FRESH_NAME_CACHE: RefCell<HashMap<(u64, &'static str), &'static str>> =
        RefCell::new(HashMap::with_capacity(64));
}

/// Cap the thread-local cache size so long-running processes don't
/// accumulate unboundedly. 1024 entries ≈ 128 epochs × 8 vars/epoch.
const FRESH_NAME_CACHE_CAP: usize = 1024;

/// Look up or allocate an interned fresh name for `(epoch, bare_name)`.
///
/// Returns a `&'static str` pointing into the global slab allocator.
///
/// **Defensive guard**: if `bare_name` is already a freshened form
/// (starts with `__fr_`), the input is returned `$`-prefixed unchanged
/// rather than compounded into `$__fr_{epoch}___fr_{old_epoch}_*`.
/// This prevents the unbounded name growth observed when freshened
/// values flow back into rule RHS templates (e.g., via runtime
/// rule construction or substitution into recursive proof terms).
#[inline]
pub fn intern_fresh_name(epoch: u64, bare_name: &'static str) -> &'static str {
    if bare_name.starts_with("__fr_") {
        // Already freshened — return $-prefixed unchanged. Cache by
        // (0, bare_name) to amortize the alloc_str call across
        // repeated lookups of the same already-freshened variable.
        return FRESH_NAME_CACHE.with(|cache| {
            let mut c = cache.borrow_mut();
            if let Some(&s) = c.get(&(0, bare_name)) {
                return s;
            }
            if c.len() >= FRESH_NAME_CACHE_CAP {
                c.clear();
            }
            let formatted = format!("${}", bare_name);
            let interned: &'static str =
                crate::backend::models::global_allocator().alloc_str(&formatted);
            c.insert((0, bare_name), interned);
            interned
        });
    }
    FRESH_NAME_CACHE.with(|cache| {
        let mut c = cache.borrow_mut();
        if let Some(&s) = c.get(&(epoch, bare_name)) {
            return s;
        }
        if c.len() >= FRESH_NAME_CACHE_CAP {
            c.clear();
        }
        let formatted = format!("$__fr_{}_{}", epoch, bare_name);
        let interned: &'static str =
            crate::backend::models::global_allocator().alloc_str(&formatted);
        c.insert((epoch, bare_name), interned);
        interned
    })
}

/// Rename `GenericBindings` keys in-place using a per-match epoch.
///
/// For every binding `(name, value)`: if `name` is listed in
/// `rule_var_names` (the rule's LHS/RHS variable set), replace it with
/// `$__fr_{epoch}_{bare_name}` (same format as
/// `freshen_variables_with_epoch`). Otherwise, keep the key unchanged.
///
/// **Use case**: `StructuralMatcher::try_match` produces bindings keyed
/// on the rule's ORIGINAL variable names (`$a`, `$b`). Immediately
/// after the match, this function rewrites those keys to per-epoch
/// unique names so they cannot collide with a sibling nondet branch's
/// bindings for the same rule variables. Mirrors HE's
/// `Bindings::from(CachingMapper(rule_vars))` pattern.
///
/// Bindings WHOSE KEY IS NOT IN `rule_var_names` are preserved
/// verbatim — these are caller-level variables (e.g., query vars like
/// `$who`) that must retain their user-facing names.
///
/// Values are NOT rewritten here. A rule's bindings values rarely
/// contain its own LHS vars (they contain caller-side atoms), but if
/// that case arises (e.g. bidirectional unify of `(f $x)` against
/// `(f $y)` producing `$x → $y`), the caller should additionally run
/// `freshen_value_occurrences_with_epoch`.
pub fn freshen_bindings_keys_with_epoch<V>(
    bindings: GenericBindings<V>,
    epoch: u64,
    rule_var_names: &[&'static str],
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone,
{
    if bindings.is_empty() || rule_var_names.is_empty() {
        return bindings;
    }
    let mut renamed = GenericBindings::<V>::new();
    for (name, value) in bindings.iter() {
        if let Some(idx) = rule_var_names.iter().position(|n| *n == name) {
            let bare = &rule_var_names[idx][1..]; // strip leading '$'
            let fresh = intern_fresh_name(epoch, bare);
            renamed.insert_or_replace(fresh, value.clone());
        } else {
            renamed.insert_or_replace(name, value.clone());
        }
    }
    renamed
}

/// Rename only the variable occurrences within `value` whose bare name
/// matches one in `rule_var_names`, using the per-match `epoch`.
///
/// Used when a bound value contains rule-LHS variable references (the
/// bidirectional-unify case). Variables NOT in `rule_var_names` (e.g.
/// caller-level vars like `$who`) pass through unchanged, unlike
/// `freshen_variables_with_epoch` which renames every `$`-prefixed
/// atom.
pub fn freshen_value_occurrences_with_epoch<V, F>(
    value: &V,
    epoch: u64,
    rule_var_names: &[&'static str],
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if rule_var_names.is_empty() {
        return value.clone();
    }
    selective_freshen_with_epoch(value, epoch, rule_var_names, factory)
}

/// Selective freshening: only rewrites variable atoms whose exact name
/// appears in `only`. Mirrors the structure of `freshen_with_epoch`
/// but uses the rule's variable-name set as a filter.
fn selective_freshen_with_epoch<V, F>(
    value: &V,
    epoch: u64,
    only: &[&'static str],
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let mut work_stack: SmallVec<[FreshenWork<V>; 16]> = SmallVec::new();
    let mut result_stack: SmallVec<[V; 16]> = SmallVec::new();

    work_stack.push(FreshenWork::Process(value));

    while let Some(work) = work_stack.pop() {
        match work {
            FreshenWork::Process(val) => {
                if let Some(name) = val.as_atom() {
                    // `$_` is a wildcard, not a variable — pass through
                    // unchanged. See `bindings::is_wildcard_atom`.
                    if name.starts_with('$')
                        && name != "$_"
                        && only.iter().any(|n| *n == name)
                    {
                        let bare = &name[1..];
                        let fresh = intern_fresh_name(epoch, bare);
                        result_stack.push(factory.atom(fresh));
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(FreshenWork::BuildSExpr(items.len()));
                        for item in items.iter().rev() {
                            work_stack.push(FreshenWork::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(FreshenWork::BuildConjunction(goals.len()));
                        for goal in goals.iter().rev() {
                            work_stack.push(FreshenWork::Process(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            FreshenWork::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let result = factory.sexpr_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
            FreshenWork::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let result = factory.conjunction_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
        }
    }

    result_stack
        .pop()
        .expect("Result stack should not be empty after freshening")
}

/// Freshen variables with a specific epoch (used internally and by tests).
fn freshen_with_epoch<V, F>(value: &V, epoch: u64, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let mut work_stack: SmallVec<[FreshenWork<V>; 16]> = SmallVec::new();
    let mut result_stack: SmallVec<[V; 16]> = SmallVec::new();

    work_stack.push(FreshenWork::Process(value));

    while let Some(work) = work_stack.pop() {
        match work {
            FreshenWork::Process(val) => {
                // Structural sharing: subtrees with no variables cannot
                // produce any rename, so the original pointer is identical
                // to the rewritten result.
                if !val.has_variables_fast() {
                    result_stack.push(val.clone());
                    continue;
                }
                if let Some(name) = val.as_atom() {
                    // `$_` is a wildcard, not a variable — each occurrence
                    // matches independently and never binds. Passing it
                    // through unchanged keeps the wildcard semantics intact
                    // across rule-load freshening and per-invocation
                    // freshening. See `bindings::is_wildcard_atom`.
                    if name.starts_with('$') && name != "$_" {
                        // Skip already-freshened variables to prevent nested
                        // compounding (`$__fr_84___fr_83_x`). When a value
                        // containing freshened variables flows back into a
                        // rule's RHS template (via runtime construction or
                        // binding substitution), this guard keeps the
                        // freshened name stable across subsequent matches.
                        // Variable identity is preserved — distinct freshened
                        // variables retain distinct epoch tags.
                        if name.starts_with("$__fr_") {
                            result_stack.push(val.clone());
                        } else {
                            // Rename $varname → $__fr_{epoch}_{varname_without_dollar}
                            let bare_name = &name[1..]; // strip leading '$'
                            result_stack.push(factory.atom(&format!("$__fr_{}_{}", epoch, bare_name)));
                        }
                    } else {
                        // Non-variable atom: pass through unchanged
                        // (includes &self, &kb, &stack, _, $_, literals, etc.)
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(FreshenWork::BuildSExpr(items.len()));
                        for item in items.iter().rev() {
                            work_stack.push(FreshenWork::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(FreshenWork::BuildConjunction(goals.len()));
                        for goal in goals.iter().rev() {
                            work_stack.push(FreshenWork::Process(goal));
                        }
                    }
                } else {
                    // Ground types (Bool, Long, Float, String, Unit, Space, State, etc.)
                    result_stack.push(val.clone());
                }
            }
            FreshenWork::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let result = factory.sexpr_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
            FreshenWork::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let result = factory.conjunction_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
        }
    }

    result_stack.pop().expect("Result stack should not be empty after freshening")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValueInner};

    fn factory() -> GcFactory {
        GcFactory::default()
    }

    #[test]
    fn test_ground_value_unchanged() {
        let f = factory();
        let val = f.long(42);
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result, val, "Ground values should pass through unchanged");
    }

    #[test]
    fn test_unit_unchanged() {
        let f = factory();
        let val = f.unit();
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result, val);
    }

    #[test]
    fn test_literal_atom_unchanged() {
        let f = factory();
        let val = f.atom("foo");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("foo"));
    }

    #[test]
    fn test_space_ref_unchanged() {
        let f = factory();
        let val = f.atom("&self");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("&self"), "&self should not be freshened");
    }

    #[test]
    fn test_wildcard_unchanged() {
        let f = factory();
        let val = f.atom("_");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("_"), "Wildcards should not be freshened");
    }

    #[test]
    fn test_single_variable_freshened() {
        let f = factory();
        let val = f.atom("$x");
        let result = freshen_with_epoch(&val, 42, &f);
        assert_eq!(result.as_atom(), Some("$__fr_42_x"));
    }

    #[test]
    fn test_nested_sexpr_freshened() {
        let f = factory();
        let val = f.sexpr(vec![
            f.atom("foo"),
            f.atom("$x"),
            f.sexpr(vec![f.atom("bar"), f.atom("$y")]),
        ]);
        let result = freshen_with_epoch(&val, 7, &f);

        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items[0].as_atom(), Some("foo"));
        assert_eq!(items[1].as_atom(), Some("$__fr_7_x"));
        let inner = items[2].as_sexpr().expect("should be inner sexpr");
        assert_eq!(inner[0].as_atom(), Some("bar"));
        assert_eq!(inner[1].as_atom(), Some("$__fr_7_y"));
    }

    #[test]
    fn test_repeated_vars_same_epoch() {
        let f = factory();
        let val = f.sexpr(vec![f.atom("$x"), f.atom("$x")]);
        let result = freshen_with_epoch(&val, 5, &f);

        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items[0].as_atom(), Some("$__fr_5_x"));
        assert_eq!(items[1].as_atom(), Some("$__fr_5_x"));
        // Same variable name → same freshened name within the same epoch
        assert_eq!(items[0], items[1]);
    }

    #[test]
    fn test_different_epochs_different_names() {
        let f = factory();
        let val = f.atom("$x");
        let r1 = freshen_with_epoch(&val, 10, &f);
        let r2 = freshen_with_epoch(&val, 11, &f);
        assert_ne!(r1, r2, "Different epochs should produce different names");
        assert_eq!(r1.as_atom(), Some("$__fr_10_x"));
        assert_eq!(r2.as_atom(), Some("$__fr_11_x"));
    }

    #[test]
    fn test_global_counter_increments() {
        let f = factory();
        let val = f.atom("$z");
        let r1 = freshen_variables_generic(&val, &f);
        let r2 = freshen_variables_generic(&val, &f);
        // Each call should use a different epoch
        assert_ne!(r1, r2, "Consecutive calls should use different epochs");
    }

    #[test]
    fn test_ampersand_variable_not_freshened() {
        let f = factory();
        // &kb and &stack are space references, not variables to freshen
        let val = f.atom("&kb");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("&kb"));
    }

    #[test]
    fn test_empty_sexpr_unchanged() {
        let f = factory();
        let val = f.unit();
        let result = freshen_with_epoch(&val, 99, &f);
        assert!(matches!(result.inner(), MettaValueInner::Unit));
    }
}
