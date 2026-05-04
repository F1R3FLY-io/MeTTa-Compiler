//! Generic Variable Bindings for Pattern Matching
//!
//! This module provides `GenericBindings<V>`, a generic bindings type that stores
//! values of any type implementing `MettaValueTrait`. This eliminates boundary
//! conversions during pattern matching by allowing bindings to store values in
//! their native representation (either heap-allocated `MettaValue` or arena-allocated
//! `MettaValue`).
//!
//! ## Design
//!
//! The type mirrors `SmartBindings` but is parameterized over the value type `V`:
//! - Empty: Zero-cost for no bindings
//! - Single: Inline for 1 binding (eliminates iterator/closure overhead)
//! - Small: SmallVec for 2-8 bindings (stack-allocated, cache-friendly)
//! - Large: SmallVec spills to heap for >8 bindings
//!
//! ## Performance
//!
//! By using `GenericBindings<V>`, we avoid O(n) deep allocations during pattern
//! matching and binding application:
//! - `MettaValue.clone()` = O(1) Arc increment
//! - `MettaValue.clone()` = O(1) pointer copy
//! - No conversions between types during evaluation

use smallvec::SmallVec;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};

use super::MettaValueTrait;

/// Per-rule-invocation scope tag.
///
/// Each rule dispatch mints a fresh `ScopeId` so its bindings can co-exist
/// in a composed map with sibling and parent dispatches that bind variables
/// of the same bare name. The user-typed query scope (top-level `!`,
/// `let`/`let*`-introduced variables, conjunction free vars) lives at
/// [`ROOT_SCOPE`] and shares one bare-name → value entry with every other
/// caller-level reference to that name.
///
/// See `docs/allocator-and-gc/06-formal-verification.md` for the migration
/// story (Phase P0 of the Scoped-Bindings plan).
pub type ScopeId = u64;

/// The caller / top-level scope. All user-typed variables (and every
/// pre-Phase-P2 binding) live here.
pub const ROOT_SCOPE: ScopeId = 0;

/// Global monotonic counter for minting new dispatch scopes.
static SCOPE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh scope ID, distinct from `ROOT_SCOPE` and every prior
/// allocation. Used at rule-dispatch entry points to tag the rule's
/// match bindings so sibling-branch and recursive-call invocations don't
/// alias caller-level keys with the same bare name.
#[inline]
pub fn allocate_scope_id() -> ScopeId {
    SCOPE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Generic bindings structure optimized for common cases.
///
/// Parameterized over the value type `V`, enabling zero-conversion pattern
/// matching with both heap and arena allocation strategies.
///
/// ## Scoped keys
///
/// Each entry is a `(ScopeId, name, value)` triple. Two entries with the
/// same bare `name` but different `ScopeId`s are independent (they bind
/// the variable in distinct lexical scopes — caller scope vs. rule
/// dispatch scope, or two sibling rule invocations). Same scope + same
/// name is a candidate for the alias / rewrite-stage / ground-ground
/// arms in `compose_outer_inner_*_generic`.
///
/// During Phase P0 of the migration, every entry is created at
/// [`ROOT_SCOPE`] via [`insert`](Self::insert) — the legacy callers
/// don't yet know about scopes — and the legacy `get` / `iter` /
/// `merge` / `compose` accessors operate on bare names, scanning all
/// scopes (which in P0 are all `ROOT_SCOPE`). Phase P1 rewrites
/// `compose_outer_inner_*_generic` to honor the full
/// `(scope, name)` key; Phase P2 starts producing entries at
/// non-root scopes.
//
// Clippy warns about the large size difference between variants (Empty: 0 bytes,
// Single: varies, Small: varies), recommending we Box the SmallVec to reduce
// the enum size.
//
// However, benchmarking shows that the unboxed version provides significant performance
// improvements in the pattern matching hot path (see SmartBindings benchmarks).
// The same rationale applies here:
// - Passed by reference (no copy overhead from large size)
// - Short-lived (created during pattern matching, quickly dropped)
// - Used in performance-critical code paths
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum GenericBindings<V: MettaValueTrait + Clone> {
    /// No bindings (zero-cost)
    Empty,
    /// Single binding (inline, no allocation)
    Single((ScopeId, &'static str, V)),
    /// 2-8 bindings (stack-allocated via SmallVec)
    /// >8 bindings (SmallVec spills to heap automatically)
    Small(SmallVec<[(ScopeId, &'static str, V); 8]>),
}

impl<V: MettaValueTrait + Clone> GenericBindings<V> {
    /// Create empty bindings
    #[inline]
    pub fn new() -> Self {
        GenericBindings::Empty
    }

    // -------- Legacy bare-name accessors (operate as if every entry is at
    // ROOT_SCOPE; in P0 this is exact because every insert routes through
    // `insert` → ROOT_SCOPE. Phases P1+ keep these as transition shims.)

    /// Get a binding by bare name. Scans all scopes and returns the first
    /// match — sufficient while every entry lives at [`ROOT_SCOPE`].
    /// Post-P2 callers should prefer [`get_scoped`](Self::get_scoped) or
    /// [`get_chain`](Self::get_chain).
    #[inline]
    pub fn get(&self, name: &str) -> Option<&V> {
        match self {
            GenericBindings::Empty => None,
            GenericBindings::Single((_, n, v)) => {
                if *n == name {
                    Some(v)
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => vec
                .iter()
                .find(|(_, n, _)| *n == name)
                .map(|(_, _, v)| v),
        }
    }

    /// Insert a binding at [`ROOT_SCOPE`].
    ///
    /// Transitions:
    /// - Empty → Single
    /// - Single → Small (with 2 elements)
    /// - Small → Small (push)
    ///
    /// Always appends — duplicate `(scope, name)` pairs are possible. Callers
    /// that want overwrite-semantics should use
    /// [`insert_or_replace`](Self::insert_or_replace).
    #[inline]
    pub fn insert(&mut self, name: &'static str, value: V) {
        self.insert_scoped(ROOT_SCOPE, name, value);
    }

    /// Iterate over all bindings as `(name, value)` pairs, dropping scope.
    /// Order matches insertion order. Scope-aware callers should use
    /// [`iter_full`](Self::iter_full) or [`iter_scoped`](Self::iter_scoped).
    pub fn iter(&self) -> GenericBindingsIter<'_, V> {
        GenericBindingsIter {
            bindings: self,
            index: 0,
        }
    }

    /// Get the number of bindings
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            GenericBindings::Empty => 0,
            GenericBindings::Single(_) => 1,
            GenericBindings::Small(vec) => vec.len(),
        }
    }

    /// Check if there are no bindings
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self, GenericBindings::Empty)
    }

    /// Merge bindings from another GenericBindings, checking for conflicts.
    ///
    /// Returns `true` if merge was successful, `false` if there was a conflict
    /// (same variable bound to different values).
    ///
    /// Scope-preserving: each `other` entry is inserted under its own scope.
    /// Conflicts are detected on the full `(scope, name)` key so cross-scope
    /// same-name entries co-exist.
    pub fn merge(&mut self, other: &GenericBindings<V>) -> bool
    where
        V: PartialEq,
    {
        for (scope, name, value) in other.iter_full() {
            if let Some(existing) = self.get_scoped(scope, name) {
                // Check for conflict
                if existing != value {
                    return false;
                }
                // Same value, skip insertion
            } else {
                self.insert_scoped(scope, name, value.clone());
            }
        }
        true
    }

    /// Extend bindings from an iterator of `(name, value)` pairs at
    /// [`ROOT_SCOPE`]. Scope-aware extend should use
    /// [`extend_scoped`](Self::extend_scoped).
    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (&'static str, V)>,
    {
        for (name, value) in iter {
            self.insert(name, value);
        }
    }

    /// Insert or replace at [`ROOT_SCOPE`]: if the bare name already exists
    /// at any scope, update the *first* match's value. Otherwise append at
    /// `ROOT_SCOPE`. This is the legacy shim — scope-aware callers should
    /// use [`insert_or_replace_scoped`](Self::insert_or_replace_scoped).
    #[inline]
    pub fn insert_or_replace(&mut self, name: &'static str, value: V) {
        match self {
            GenericBindings::Empty => {
                *self = GenericBindings::Single((ROOT_SCOPE, name, value));
            }
            GenericBindings::Single((_existing_scope, existing_name, existing_value)) => {
                if *existing_name == name {
                    *existing_value = value;
                } else {
                    let existing_scope = *_existing_scope;
                    let existing_name = *existing_name;
                    let existing_value = existing_value.clone();
                    let mut vec = SmallVec::new();
                    vec.push((existing_scope, existing_name, existing_value));
                    vec.push((ROOT_SCOPE, name, value));
                    *self = GenericBindings::Small(vec);
                }
            }
            GenericBindings::Small(vec) => {
                for entry in vec.iter_mut() {
                    if entry.1 == name {
                        entry.2 = value;
                        return;
                    }
                }
                vec.push((ROOT_SCOPE, name, value));
            }
        }
    }

    /// Compose two binding sets for scope chaining: `self` is the outer scope,
    /// `inner` bindings shadow outer bindings on name conflicts.
    ///
    /// This does NOT perform substitutive composition (i.e., it does NOT apply
    /// inner bindings to the values of outer bindings). The evaluator handles
    /// transitive variable resolution through recursive `EvalWithBindings` dispatch.
    pub fn compose(&self, inner: &GenericBindings<V>) -> GenericBindings<V> {
        if self.is_empty() {
            return inner.clone();
        }
        if inner.is_empty() {
            return self.clone();
        }
        let mut result = self.clone();
        for (scope, name, value) in inner.iter_full() {
            result.insert_or_replace_scoped(scope, name, value.clone());
        }
        result
    }

    // -------- Scope-aware accessors (Phase P0+; preferred over the legacy
    // bare-name accessors above). The legacy accessors continue to work
    // because every binding produced before Phase P2 lives at ROOT_SCOPE.

    /// Get the value bound to `(scope, name)`, or `None` if no such entry.
    #[inline]
    pub fn get_scoped(&self, scope: ScopeId, name: &str) -> Option<&V> {
        match self {
            GenericBindings::Empty => None,
            GenericBindings::Single((s, n, v)) => {
                if *s == scope && *n == name {
                    Some(v)
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => vec
                .iter()
                .find(|(s, n, _)| *s == scope && *n == name)
                .map(|(_, _, v)| v),
        }
    }

    /// Walk a list of scopes and return the value bound to `name` in the
    /// first scope that has one. Used by template-walk callers that look
    /// up rule-local atoms at the dispatch scope, falling back to the
    /// caller scope ([`ROOT_SCOPE`]) for atoms that originated outside
    /// the rule.
    #[inline]
    pub fn get_chain(&self, scope_chain: &[ScopeId], name: &str) -> Option<&V> {
        for &scope in scope_chain {
            if let Some(v) = self.get_scoped(scope, name) {
                return Some(v);
            }
        }
        None
    }

    /// Append a binding at `(scope, name)`. Always appends — duplicate keys
    /// are possible; use [`insert_or_replace_scoped`](Self::insert_or_replace_scoped)
    /// for overwrite-on-collision.
    #[inline]
    pub fn insert_scoped(&mut self, scope: ScopeId, name: &'static str, value: V) {
        match self {
            GenericBindings::Empty => {
                *self = GenericBindings::Single((scope, name, value));
            }
            GenericBindings::Single(existing) => {
                let mut vec = SmallVec::new();
                vec.push(existing.clone());
                vec.push((scope, name, value));
                *self = GenericBindings::Small(vec);
            }
            GenericBindings::Small(vec) => {
                vec.push((scope, name, value));
            }
        }
    }

    /// Insert or overwrite the value at `(scope, name)`. If an entry with the
    /// exact same `(scope, name)` exists, its value is replaced; otherwise
    /// a new entry is appended. Same-name entries at *different* scopes
    /// co-exist — they are independent bindings.
    #[inline]
    pub fn insert_or_replace_scoped(
        &mut self,
        scope: ScopeId,
        name: &'static str,
        value: V,
    ) {
        match self {
            GenericBindings::Empty => {
                *self = GenericBindings::Single((scope, name, value));
            }
            GenericBindings::Single((existing_scope, existing_name, existing_value)) => {
                if *existing_scope == scope && *existing_name == name {
                    *existing_value = value;
                } else {
                    let triple = (
                        *existing_scope,
                        *existing_name,
                        existing_value.clone(),
                    );
                    let mut vec = SmallVec::new();
                    vec.push(triple);
                    vec.push((scope, name, value));
                    *self = GenericBindings::Small(vec);
                }
            }
            GenericBindings::Small(vec) => {
                for entry in vec.iter_mut() {
                    if entry.0 == scope && entry.1 == name {
                        entry.2 = value;
                        return;
                    }
                }
                vec.push((scope, name, value));
            }
        }
    }

    /// Extend with `(scope, name, value)` triples.
    pub fn extend_scoped<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (ScopeId, &'static str, V)>,
    {
        for (scope, name, value) in iter {
            self.insert_scoped(scope, name, value);
        }
    }

    /// Iterate over `(scope, name, value)` triples in insertion order.
    pub fn iter_full(&self) -> GenericBindingsFullIter<'_, V> {
        GenericBindingsFullIter {
            bindings: self,
            index: 0,
        }
    }

    /// Iterate over `(name, value)` pairs whose entries match `scope`.
    pub fn iter_scoped(
        &self,
        scope: ScopeId,
    ) -> impl Iterator<Item = (&'static str, &V)> + '_ {
        self.iter_full()
            .filter_map(move |(s, n, v)| if s == scope { Some((n, v)) } else { None })
    }
}

impl<V: MettaValueTrait + Clone> Default for GenericBindings<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: MettaValueTrait + Clone + PartialEq> PartialEq for GenericBindings<V> {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        for (scope, name, value) in self.iter_full() {
            match other.get_scoped(scope, name) {
                Some(other_value) if other_value == value => continue,
                _ => return false,
            }
        }
        true
    }
}

/// Iterator over generic bindings yielding `(name, value)` pairs (legacy shim,
/// drops scope info).
pub struct GenericBindingsIter<'a, V: MettaValueTrait + Clone> {
    bindings: &'a GenericBindings<V>,
    index: usize,
}

impl<'a, V: MettaValueTrait + Clone> Iterator for GenericBindingsIter<'a, V> {
    type Item = (&'static str, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self.bindings {
            GenericBindings::Empty => None,
            GenericBindings::Single((_, n, v)) => {
                if self.index == 0 {
                    self.index += 1;
                    Some((*n, v))
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => {
                if self.index < vec.len() {
                    let result = &vec[self.index];
                    self.index += 1;
                    Some((result.1, &result.2))
                } else {
                    None
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = match self.bindings {
            GenericBindings::Empty => 0,
            GenericBindings::Single(_) => {
                if self.index == 0 {
                    1
                } else {
                    0
                }
            }
            GenericBindings::Small(vec) => vec.len().saturating_sub(self.index),
        };
        (remaining, Some(remaining))
    }
}

impl<'a, V: MettaValueTrait + Clone> ExactSizeIterator for GenericBindingsIter<'a, V> {}

/// Iterator yielding full `(ScopeId, &'static str, &V)` triples.
///
/// Use this for scope-aware compose / merge / round-trip emission. The
/// legacy [`GenericBindingsIter`] discards the scope and is retained as a
/// compatibility shim.
pub struct GenericBindingsFullIter<'a, V: MettaValueTrait + Clone> {
    bindings: &'a GenericBindings<V>,
    index: usize,
}

impl<'a, V: MettaValueTrait + Clone> Iterator for GenericBindingsFullIter<'a, V> {
    type Item = (ScopeId, &'static str, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self.bindings {
            GenericBindings::Empty => None,
            GenericBindings::Single((s, n, v)) => {
                if self.index == 0 {
                    self.index += 1;
                    Some((*s, *n, v))
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => {
                if self.index < vec.len() {
                    let result = &vec[self.index];
                    self.index += 1;
                    Some((result.0, result.1, &result.2))
                } else {
                    None
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = match self.bindings {
            GenericBindings::Empty => 0,
            GenericBindings::Single(_) => {
                if self.index == 0 {
                    1
                } else {
                    0
                }
            }
            GenericBindings::Small(vec) => vec.len().saturating_sub(self.index),
        };
        (remaining, Some(remaining))
    }
}

impl<'a, V: MettaValueTrait + Clone> ExactSizeIterator for GenericBindingsFullIter<'a, V> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;
    use crate::backend::models::gc_allocator::global_allocator;

    #[test]
    fn test_empty_bindings() {
        let bindings: GenericBindings<MettaValue> = GenericBindings::new();
        assert!(bindings.is_empty());
        assert_eq!(bindings.len(), 0);
        assert_eq!(bindings.get("$x"), None);
    }

    #[test]
    fn test_single_binding() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x", MettaValue::Long(42));

        assert!(!bindings.is_empty());
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
        assert_eq!(bindings.get("$y"), None);

        // Check variant
        assert!(matches!(bindings, GenericBindings::Single(_)));
    }

    #[test]
    fn test_transition_to_small() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x", MettaValue::Long(42));
        bindings.insert("$y", MettaValue::Long(43));

        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
        assert_eq!(bindings.get("$y"), Some(&MettaValue::Long(43)));

        // Check variant transitioned to Small
        assert!(matches!(bindings, GenericBindings::Small(_)));
    }

    #[test]
    fn test_small_bindings() {
        let alloc = global_allocator();
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        for i in 0..5 {
            bindings.insert(alloc.alloc_str(&format!("$v{}", i)), MettaValue::Long(i as i64));
        }

        assert_eq!(bindings.len(), 5);
        for i in 0..5 {
            assert_eq!(
                bindings.get(&format!("$v{}", i)),
                Some(&MettaValue::Long(i as i64))
            );
        }
    }

    #[test]
    fn test_iterator() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x", MettaValue::Long(1));
        bindings.insert("$y", MettaValue::Long(2));
        bindings.insert("$z", MettaValue::Long(3));

        let collected: Vec<_> = bindings.iter().collect();
        assert_eq!(collected.len(), 3);

        // Check all bindings are present
        let has_x = collected
            .iter()
            .any(|(n, v)| n == &"$x" && **v == MettaValue::Long(1));
        let has_y = collected
            .iter()
            .any(|(n, v)| n == &"$y" && **v == MettaValue::Long(2));
        let has_z = collected
            .iter()
            .any(|(n, v)| n == &"$z" && **v == MettaValue::Long(3));
        assert!(has_x && has_y && has_z);
    }

    #[test]
    fn test_empty_iterator() {
        let bindings: GenericBindings<MettaValue> = GenericBindings::new();
        let collected: Vec<_> = bindings.iter().collect();
        assert_eq!(collected.len(), 0);
    }

    #[test]
    fn test_single_iterator() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x", MettaValue::Long(42));

        let collected: Vec<_> = bindings.iter().collect();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, "$x");
        assert_eq!(*collected[0].1, MettaValue::Long(42));
    }

    #[test]
    fn test_merge_success() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x", MettaValue::Long(1));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$y", MettaValue::Long(2));

        assert!(bindings1.merge(&bindings2));
        assert_eq!(bindings1.len(), 2);
        assert_eq!(bindings1.get("$x"), Some(&MettaValue::Long(1)));
        assert_eq!(bindings1.get("$y"), Some(&MettaValue::Long(2)));
    }

    #[test]
    fn test_merge_conflict() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x", MettaValue::Long(1));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$x", MettaValue::Long(2)); // Different value!

        assert!(!bindings1.merge(&bindings2)); // Conflict!
    }

    #[test]
    fn test_merge_same_value() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x", MettaValue::Long(1));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$x", MettaValue::Long(1)); // Same value

        assert!(bindings1.merge(&bindings2)); // No conflict
        assert_eq!(bindings1.len(), 1); // Still just one binding
    }

    #[test]
    fn test_equality() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x", MettaValue::Long(1));
        bindings1.insert("$y", MettaValue::Long(2));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$y", MettaValue::Long(2));
        bindings2.insert("$x", MettaValue::Long(1));

        assert_eq!(bindings1, bindings2);
    }
}
