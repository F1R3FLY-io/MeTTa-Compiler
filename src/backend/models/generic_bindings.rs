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
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::backend::symbol::{intern as intern_symbol, Symbol};

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

/// GC-independent variable binding key.
///
/// Binding keys outlive the immediate `MettaValue::as_atom()` borrow that
/// produced them, so they must not point into GC-managed slab string storage.
/// Source/rule variable names are canonicalized through the backend symbol
/// table. Generated textual fresh names remain process-reclaimable by using
/// `Arc<str>` instead of the global interner.
#[derive(Clone, Debug)]
pub enum BindingName {
    Stable(Symbol),
    Ephemeral(Arc<str>),
}

impl BindingName {
    #[inline]
    pub fn new(name: &str) -> Self {
        if name.starts_with("$__fr_") {
            Self::Ephemeral(Arc::<str>::from(name))
        } else {
            Self::Stable(intern_symbol(name))
        }
    }

    #[inline]
    pub fn stable(name: &str) -> Self {
        Self::Stable(intern_symbol(name))
    }

    #[inline]
    pub fn ephemeral(name: impl Into<Arc<str>>) -> Self {
        Self::Ephemeral(name.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Stable(symbol) => symbol.as_str(),
            Self::Ephemeral(name) => name.as_ref(),
        }
    }

    #[inline]
    pub fn matches(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl From<&str> for BindingName {
    #[inline]
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<&String> for BindingName {
    #[inline]
    fn from(value: &String) -> Self {
        Self::new(value.as_str())
    }
}

impl From<String> for BindingName {
    #[inline]
    fn from(value: String) -> Self {
        if value.starts_with("$__fr_") {
            Self::Ephemeral(Arc::<str>::from(value))
        } else {
            Self::Stable(intern_symbol(&value))
        }
    }
}

impl From<Symbol> for BindingName {
    #[inline]
    fn from(value: Symbol) -> Self {
        Self::Stable(value)
    }
}

impl From<&BindingName> for BindingName {
    #[inline]
    fn from(value: &BindingName) -> Self {
        value.clone()
    }
}

impl AsRef<str> for BindingName {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for BindingName {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Stable(a), Self::Stable(b)) => a == b,
            (Self::Ephemeral(a), Self::Ephemeral(b)) => a == b,
            _ => self.as_str() == other.as_str(),
        }
    }
}

impl Eq for BindingName {}

impl Hash for BindingName {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl PartialEq<str> for BindingName {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.matches(other)
    }
}

impl PartialEq<&str> for BindingName {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.matches(other)
    }
}

impl PartialOrd for BindingName {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BindingName {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
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
    Single((ScopeId, BindingName, V)),
    /// 2-8 bindings (stack-allocated via SmallVec)
    /// >8 bindings (SmallVec spills to heap automatically)
    Small(SmallVec<[(ScopeId, BindingName, V); 8]>),
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
                if n.matches(name) {
                    Some(v)
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => {
                vec.iter().find(|(_, n, _)| n.matches(name)).map(|(_, _, v)| v)
            }
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
    pub fn insert<N>(&mut self, name: N, value: V)
    where
        N: Into<BindingName>,
    {
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
    pub fn extend<I, N>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<BindingName>,
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
    pub fn insert_or_replace<N>(&mut self, name: N, value: V)
    where
        N: Into<BindingName>,
    {
        let name = name.into();
        match self {
            GenericBindings::Empty => {
                *self = GenericBindings::Single((ROOT_SCOPE, name, value));
            }
            GenericBindings::Single((_existing_scope, existing_name, existing_value)) => {
                if existing_name == &name {
                    *existing_value = value;
                } else {
                    let existing_scope = *_existing_scope;
                    let existing_name = existing_name.clone();
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
                if *s == scope && n.matches(name) {
                    Some(v)
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => vec
                .iter()
                .find(|(s, n, _)| *s == scope && n.matches(name))
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
    pub fn insert_scoped<N>(&mut self, scope: ScopeId, name: N, value: V)
    where
        N: Into<BindingName>,
    {
        let name = name.into();
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
    pub fn insert_or_replace_scoped<N>(&mut self, scope: ScopeId, name: N, value: V)
    where
        N: Into<BindingName>,
    {
        let name = name.into();
        match self {
            GenericBindings::Empty => {
                *self = GenericBindings::Single((scope, name, value));
            }
            GenericBindings::Single((existing_scope, existing_name, existing_value)) => {
                if *existing_scope == scope && existing_name == &name {
                    *existing_value = value;
                } else {
                    let triple = (*existing_scope, existing_name.clone(), existing_value.clone());
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
    pub fn extend_scoped<I, N>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (ScopeId, N, V)>,
        N: Into<BindingName>,
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
    pub fn iter_scoped(&self, scope: ScopeId) -> impl Iterator<Item = (&str, &V)> + '_ {
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
    type Item = (&'a str, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self.bindings {
            GenericBindings::Empty => None,
            GenericBindings::Single((_, n, v)) => {
                if self.index == 0 {
                    self.index += 1;
                    Some((n.as_str(), v))
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => {
                if self.index < vec.len() {
                    let result = &vec[self.index];
                    self.index += 1;
                    Some((result.1.as_str(), &result.2))
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

/// Iterator yielding full `(ScopeId, &str, &V)` triples.
///
/// Use this for scope-aware compose / merge / round-trip emission. The
/// legacy [`GenericBindingsIter`] discards the scope and is retained as a
/// compatibility shim.
pub struct GenericBindingsFullIter<'a, V: MettaValueTrait + Clone> {
    bindings: &'a GenericBindings<V>,
    index: usize,
}

impl<'a, V: MettaValueTrait + Clone> Iterator for GenericBindingsFullIter<'a, V> {
    type Item = (ScopeId, &'a str, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self.bindings {
            GenericBindings::Empty => None,
            GenericBindings::Single((s, n, v)) => {
                if self.index == 0 {
                    self.index += 1;
                    Some((*s, n.as_str(), v))
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => {
                if self.index < vec.len() {
                    let result = &vec[self.index];
                    self.index += 1;
                    Some((result.0, result.1.as_str(), &result.2))
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

// ============================================================================
// S0d.1 — UnifyMode dispatch tag
// ============================================================================

/// Distinguishes the two callers of bidirectional unification.
///
/// - [`UnifyMode::Match`] (default): pattern-match binds rule LHS vars to
///   values from the query expression. Var-var-distinct creates an ordinary
///   binding `lhs → rhs` (chain-terminus semantics). Preserves existing PLN
///   pattern-match behavior — every Match-mode call is byte-identical to
///   pre-S0d behavior.
///
/// - [`UnifyMode::Unify`]: the user-facing `(unify ...)` form. Var-var-distinct
///   creates an equivalence class via [`BindingsWithClasses::insert_equivalence`].
///   Honors HE's M-VAR-VAR-DISTINCT (spec §4.3.1) so terminal var-var pairs
///   resolve to the ORIGINAL lookup-key name rather than chaining through to
///   a fresh name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifyMode {
    Match,
    Unify,
}

// ============================================================================
// S0d.0 — Equivalence-class bindings (foundation data structure)
// ============================================================================
//
// These types coexist alongside `GenericBindings<V>` and are not yet consumed
// anywhere in the evaluator. They form the data layer for HE-bisimilar
// equivalence-class bindings (mirrors HE's `BindingsMap` —
// `hyperon-experimental/hyperon-atom/src/matcher.rs:184-280`).
//
// Storage is inline (vs HE's split id_by_name / value_by_id) to keep one
// allocation hot for the common ≤ 4 members / ≤ 8 classes case.
//
// Lifecycle: `BindingsWithClasses<V>` lazily allocates the class table; the
// common (~95% of pattern matches form no equivalences) case stays on the
// fast path through `classes.is_none()`.

/// Monotonic class id assigned by `ClassTable::next_id`.
pub type ClassId = u32;

/// Per-class data: list of member names + optional shared value.
///
/// `members` is a `SmallVec<[BindingName; 4]>` because HE benchmarks
/// (Direct.metta / DR.metta) show classes rarely grow beyond 4 members.
#[derive(Debug, Clone)]
pub struct ClassData<V: Clone> {
    pub members: SmallVec<[BindingName; 4]>,
    pub value: Option<V>,
}

/// Conflict reasons when a value-install or merge fails.
#[derive(Debug, Clone)]
pub enum MergeConflict<V: Clone> {
    /// Incompatible values — both sides have distinct concrete values.
    ///
    /// Returned when at least one side is a ground type (Long, Bool, etc.)
    /// or both are concrete-distinct atoms, so structural unification
    /// cannot succeed.
    Incompatible,
    /// Both sides have values that are structurally similar; the caller
    /// should push (a, b) onto its unification work stack and continue.
    ///
    /// Returned when both sides are `SExpr`. The caller (S0d.1+ unifier)
    /// is expected to recurse on the children.
    NeedsUnify(V, V),
}

/// Equivalence-class registry with union-find and per-class value storage.
///
/// Mirrors HE's `BindingsMap` (`hyperon-experimental/hyperon-atom/src/matcher.rs:184-280`)
/// but stores data inline rather than via HE's split id_by_name/value_by_id
/// to keep one allocation hot.
///
/// ## Invariants
///
/// 1. Every name in `name_to_class` maps to a class id that exists in
///    `classes` and in `parent` (though it may not be the root after
///    unions — callers must `find` first to canonicalize).
/// 2. Every class id `c` in `classes` has `parent[c] = c` initially and
///    `parent[c]` walks up to a root; `rank[c]` exists.
/// 3. `next_id` is strictly monotonic — no id is ever reused.
/// 4. `version` is bumped on every mutating operation.
/// 5. After `union(a, b) -> root`, every name that previously mapped to
///    either side still maps to a class id whose `find()` is `root`.
#[derive(Debug, Clone)]
pub struct ClassTable<V: Clone> {
    /// var name -> class id (every member has an entry)
    pub(crate) name_to_class: BTreeMap<BindingName, ClassId>,
    /// class id -> class members + optional value
    pub(crate) classes: BTreeMap<ClassId, ClassData<V>>,
    /// Union-find parent map; class id -> parent class id (root = self)
    pub(crate) parent: BTreeMap<ClassId, ClassId>,
    /// Union-by-rank tie-breaker
    pub(crate) rank: BTreeMap<ClassId, u8>,
    /// Monotonic id counter
    pub(crate) next_id: u32,
    /// Version counter; bumped on every mutation (used by JIT cross-tier sync in S0d.3)
    pub(crate) version: u64,
}

// Manual Default impl — V need not implement Default (e.g. MettaValue lacks it).
impl<V: Clone> Default for ClassTable<V> {
    fn default() -> Self {
        Self {
            name_to_class: BTreeMap::new(),
            classes: BTreeMap::new(),
            parent: BTreeMap::new(),
            rank: BTreeMap::new(),
            next_id: 0,
            version: 0,
        }
    }
}

impl<V: MettaValueTrait + Clone + PartialEq> ClassTable<V> {
    /// Find the canonical class id (with path-compression).
    ///
    /// Classic two-pass path compression: first walk to find the root,
    /// then relink every visited node directly to the root.
    fn find_compressed(&mut self, id: ClassId) -> ClassId {
        // Walk to root
        let mut current = id;
        loop {
            let parent = match self.parent.get(&current) {
                Some(&p) => p,
                None => return current, // unknown id — return as-is
            };
            if parent == current {
                break;
            }
            current = parent;
        }
        let root = current;

        // Relink each visited node directly to root.
        let mut node = id;
        while let Some(&parent) = self.parent.get(&node) {
            if parent == node {
                break;
            }
            self.parent.insert(node, root);
            node = parent;
        }
        root
    }

    /// Read-only find without path compression.
    ///
    /// Walks the parent chain — O(log n) average since union-by-rank keeps
    /// the tree balanced. Used in `&self` contexts where mutation is not
    /// possible.
    pub fn find(&self, id: ClassId) -> ClassId {
        let mut current = id;
        loop {
            let parent = match self.parent.get(&current) {
                Some(&p) => p,
                None => return current,
            };
            if parent == current {
                return current;
            }
            current = parent;
        }
    }

    /// Look up the canonical class id for a given variable name.
    ///
    /// Returns the *current root* of the class (post-find), or `None` if
    /// `name` is not in any class.
    pub fn class_of(&self, name: &str) -> Option<ClassId> {
        // BTreeMap doesn't allow lookup by &str directly without the same
        // key type; we scan linearly. For the ≤ 8 classes typical case
        // this is the same as HashMap and avoids a hash.
        for (k, v) in &self.name_to_class {
            if k.matches(name) {
                return Some(self.find(*v));
            }
        }
        None
    }

    /// Get the class value (if any). `id` is canonicalized via `find()`.
    pub fn class_value(&self, id: ClassId) -> Option<&V> {
        let root = self.find(id);
        self.classes.get(&root).and_then(|c| c.value.as_ref())
    }

    /// Get the members of a class. `id` is canonicalized via `find()`.
    pub fn class_members(&self, id: ClassId) -> Option<&[BindingName]> {
        let root = self.find(id);
        self.classes.get(&root).map(|c| c.members.as_slice())
    }

    /// Allocate a fresh class id; monotonic, never reused.
    fn next_id(&mut self) -> ClassId {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("ClassTable::next_id overflow (u32::MAX classes)");
        id
    }

    /// Create a new class containing two members. Both must be currently
    /// unclassified. Returns the new class id.
    pub fn insert_equivalence_pair(&mut self, a: BindingName, b: BindingName) -> ClassId {
        debug_assert!(
            self.class_of(a.as_str()).is_none(),
            "insert_equivalence_pair: `a` is already in a class"
        );
        debug_assert!(
            self.class_of(b.as_str()).is_none(),
            "insert_equivalence_pair: `b` is already in a class"
        );
        let id = self.next_id();
        let mut members: SmallVec<[BindingName; 4]> = SmallVec::new();
        members.push(a.clone());
        members.push(b.clone());
        self.classes.insert(
            id,
            ClassData {
                members,
                value: None,
            },
        );
        self.parent.insert(id, id);
        self.rank.insert(id, 0);
        self.name_to_class.insert(a, id);
        self.name_to_class.insert(b, id);
        self.bump_version();
        id
    }

    /// Add a member to an existing class. `id` is canonicalized via
    /// `find_compressed`. The member must be currently unclassified.
    pub fn add_member_to_class(&mut self, id: ClassId, name: BindingName) {
        let root = self.find_compressed(id);
        debug_assert!(
            self.class_of(name.as_str()).is_none(),
            "add_member_to_class: name is already in a class"
        );
        if let Some(class) = self.classes.get_mut(&root) {
            class.members.push(name.clone());
        }
        self.name_to_class.insert(name, root);
        self.bump_version();
    }

    /// Set the value of a class. If the class already has a value, returns
    /// `MergeConflict::Incompatible` (or `NeedsUnify` if both sides are
    /// structurally compatible SExprs).
    ///
    /// If the new value equals the existing one, returns `Ok(())` (idempotent).
    pub fn set_class_value(&mut self, id: ClassId, value: V) -> Result<(), MergeConflict<V>> {
        let root = self.find_compressed(id);
        let class = self
            .classes
            .get_mut(&root)
            .expect("set_class_value: invalid class id");
        match class.value.take() {
            None => {
                class.value = Some(value);
                self.bump_version();
                Ok(())
            }
            Some(existing) => {
                if existing == value {
                    // Idempotent — restore existing.
                    class.value = Some(existing);
                    Ok(())
                } else if existing.is_sexpr() && value.is_sexpr() {
                    // Both structurally compatible — restore existing,
                    // surface the pair so the caller can recurse.
                    class.value = Some(existing.clone());
                    Err(MergeConflict::NeedsUnify(existing, value))
                } else {
                    // Concrete-distinct ground types or mixed-kind values.
                    class.value = Some(existing);
                    Err(MergeConflict::Incompatible)
                }
            }
        }
    }

    /// Union two classes. Returns the new canonical root id.
    ///
    /// Uses union-by-rank. On value conflict, returns
    /// `MergeConflict::Incompatible` if values are concrete-distinct, or
    /// `MergeConflict::NeedsUnify(a, b)` if both sides are SExprs.
    pub fn union(&mut self, a: ClassId, b: ClassId) -> Result<ClassId, MergeConflict<V>> {
        let ra = self.find_compressed(a);
        let rb = self.find_compressed(b);
        if ra == rb {
            // Already unified.
            return Ok(ra);
        }

        // Reconcile values first — if incompatible, abort without mutating
        // the union-find structure (preserves invariants).
        let value = match (
            self.classes.get(&ra).and_then(|c| c.value.clone()),
            self.classes.get(&rb).and_then(|c| c.value.clone()),
        ) {
            (None, None) => None,
            (Some(v), None) | (None, Some(v)) => Some(v),
            (Some(va), Some(vb)) => {
                if va == vb {
                    Some(va)
                } else if va.is_sexpr() && vb.is_sexpr() {
                    return Err(MergeConflict::NeedsUnify(va, vb));
                } else {
                    return Err(MergeConflict::Incompatible);
                }
            }
        };

        // Union-by-rank.
        let rank_a = self.rank.get(&ra).copied().unwrap_or(0);
        let rank_b = self.rank.get(&rb).copied().unwrap_or(0);
        let (winner, loser) = match rank_a.cmp(&rank_b) {
            std::cmp::Ordering::Less => (rb, ra),
            std::cmp::Ordering::Greater => (ra, rb),
            std::cmp::Ordering::Equal => {
                // Tie — choose ra as winner and bump its rank.
                self.rank.insert(ra, rank_a + 1);
                (ra, rb)
            }
        };

        // Reparent the loser's tree to the winner.
        self.parent.insert(loser, winner);

        // Move loser's members onto the winner's class.
        let loser_members = self
            .classes
            .remove(&loser)
            .map(|c| c.members)
            .unwrap_or_default();
        if let Some(winner_class) = self.classes.get_mut(&winner) {
            for m in &loser_members {
                // Repoint name_to_class to the winner root.
                self.name_to_class.insert(m.clone(), winner);
                winner_class.members.push(m.clone());
            }
            winner_class.value = value;
        }
        // We can drop the loser's rank entry — irrelevant once it's not a root.
        self.rank.remove(&loser);

        self.bump_version();
        Ok(winner)
    }

    /// Number of *live* classes (post-find collapsed). After unions, the
    /// loser entries have been removed from `classes`, so the size is
    /// accurate.
    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// Is the table empty?
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// Bump version counter (for cross-tier sync in S0d.3).
    pub(crate) fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }
}

// ============================================================================
// BindingsWithClasses — entries + optional class table
// ============================================================================

/// Wrapper that pairs ordinary bindings with an optional equivalence-class
/// registry. The class table is lazily allocated (None = no equivalences
/// formed yet, the common case ~95% of pattern matches).
///
/// Downstream consumers (S0d.1+) should bail out to the entries-only fast
/// path via [`is_empty_classes`](Self::is_empty_classes).
#[derive(Debug, Clone)]
pub struct BindingsWithClasses<V: MettaValueTrait + Clone> {
    pub entries: GenericBindings<V>,
    pub classes: Option<Arc<ClassTable<V>>>,
}

// Manual Default impl — V need not implement Default.
impl<V: MettaValueTrait + Clone> Default for BindingsWithClasses<V> {
    fn default() -> Self {
        Self {
            entries: GenericBindings::new(),
            classes: None,
        }
    }
}

impl<V: MettaValueTrait + Clone + PartialEq + Send + Sync + Unpin + 'static>
    BindingsWithClasses<V>
{
    /// Create empty bindings (no entries, no classes).
    pub fn new() -> Self {
        Self {
            entries: GenericBindings::new(),
            classes: None,
        }
    }

    /// Wrap an existing `GenericBindings<V>` with no class table.
    /// Used by S0d.1+ unify callers to bootstrap before class formation.
    pub fn from_entries(entries: GenericBindings<V>) -> Self {
        Self {
            entries,
            classes: None,
        }
    }

    /// Consume the wrapper and return only the entries map. Useful when
    /// downstream consumers don't need class-aware lookup (e.g. legacy
    /// `apply_bindings(&Bindings)` callers).
    pub fn into_entries(self) -> GenericBindings<V> {
        self.entries
    }

    /// True iff both entries and classes are empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.is_empty_classes()
    }

    /// Fast-path predicate: true iff no equivalence classes are formed.
    ///
    /// Downstream consumers (S0d.1+) should branch on this to bail out of
    /// class-aware logic.
    #[inline]
    pub fn is_empty_classes(&self) -> bool {
        match &self.classes {
            None => true,
            Some(table) => table.is_empty(),
        }
    }

    /// Number of equivalence classes.
    pub fn class_count(&self) -> usize {
        self.classes.as_ref().map_or(0, |t| t.class_count())
    }

    /// Get-or-init the class table for mutation. Triggers `Arc::make_mut` on
    /// shared tables to enforce clone-on-write semantics.
    pub fn classes_mut(&mut self) -> &mut ClassTable<V> {
        if self.classes.is_none() {
            self.classes = Some(Arc::new(ClassTable::default()));
        }
        // Safe: just initialized above if it was None.
        let arc = self.classes.as_mut().expect("classes just initialized");
        Arc::make_mut(arc)
    }

    /// Establish equivalence between two variable names.
    ///
    /// - If neither is currently classified, creates a new pair-class.
    /// - If exactly one is classified, adds the other to that class.
    /// - If both are classified, unions the two classes (reconciling values).
    pub fn insert_equivalence(
        &mut self,
        a: BindingName,
        b: BindingName,
    ) -> Result<(), MergeConflict<V>> {
        let table = self.classes_mut();
        let class_a = table.class_of(a.as_str());
        let class_b = table.class_of(b.as_str());
        match (class_a, class_b) {
            (None, None) => {
                table.insert_equivalence_pair(a, b);
                Ok(())
            }
            (Some(id), None) => {
                table.add_member_to_class(id, b);
                Ok(())
            }
            (None, Some(id)) => {
                table.add_member_to_class(id, a);
                Ok(())
            }
            (Some(ida), Some(idb)) => {
                if ida == idb {
                    Ok(())
                } else {
                    table.union(ida, idb).map(|_| ())
                }
            }
        }
    }

    /// Install a value for `name`.
    ///
    /// - If `name` is in a value-less class, promotes the class to value-bearing
    ///   (value-propagation across all class members).
    /// - If `name` is in a value-bearing class, returns `MergeConflict` if the
    ///   value disagrees with the class value.
    /// - Otherwise inserts as an ordinary (non-class) entry.
    pub fn insert_value(&mut self, name: BindingName, value: V) -> Result<(), MergeConflict<V>> {
        if let Some(table) = self.classes.as_ref() {
            if let Some(id) = table.class_of(name.as_str()) {
                let table_mut = self.classes_mut();
                return table_mut.set_class_value(id, value);
            }
        }
        self.entries.insert(name, value);
        Ok(())
    }
}

/// Concrete `MettaValue` impl: `resolve` synthesizes an Atom value for
/// value-less class members using the global factory.
///
/// This is in a concrete impl block (rather than generic) because the
/// generic `MettaValueTrait` bound does not provide a `from_atom_str`
/// constructor, and the only consumer in MeTTaTron is `MettaValue`.
impl BindingsWithClasses<super::MettaValue> {
    /// Class-aware value lookup.
    ///
    /// Resolution order:
    /// 1. Ordinary entries (`self.entries.get(name)`).
    /// 2. Class membership — if `name` is in a class:
    ///    - Value-bearing class: return the class value.
    ///    - Value-less class: return the *original* `name` as an Atom value
    ///      (preserves distinct identity in `apply_bindings`).
    /// 3. Otherwise `None` (unbound).
    pub fn resolve(&self, name: &str) -> Option<super::MettaValue> {
        use super::metta_value_trait::MettaValueFactory;
        if let Some(v) = self.entries.get(name) {
            return Some(v.clone());
        }
        let table = self.classes.as_ref()?;
        let id = table.class_of(name)?;
        if let Some(v) = table.class_value(id) {
            Some(v.clone())
        } else {
            // Value-less class: return ORIGINAL lookup name as Atom, preserving
            // distinct identity (matches HE's apply_bindings semantics —
            // unresolved vars keep their original surface name).
            Some(super::gc_allocator::global_factory().atom(name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::gc_allocator::global_allocator;
    use crate::backend::models::MettaValue;

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
    fn test_binding_name_survives_source_string_drop() {
        let key = {
            let dynamic_name = String::from("$__fr_temp");
            BindingName::from(dynamic_name.as_str())
        };
        assert_eq!(key.as_str(), "$__fr_temp");
        assert!(matches!(key, BindingName::Ephemeral(_)));

        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        {
            let dynamic_name = String::from("$__fr_local");
            bindings.insert(dynamic_name.as_str(), MettaValue::Long(7));
        }
        assert_eq!(bindings.get("$__fr_local"), Some(&MettaValue::Long(7)));
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
            bindings.insert(
                alloc.alloc_str(&format!("$v{}", i)),
                MettaValue::Long(i as i64),
            );
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

#[cfg(test)]
mod class_table_tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValue, MettaValueFactory};

    fn make_atom(s: &str) -> MettaValue {
        global_factory().atom(s)
    }
    fn make_long(n: i64) -> MettaValue {
        global_factory().long(n)
    }

    #[test]
    fn test_empty_class_table() {
        let t: ClassTable<MettaValue> = ClassTable::default();
        assert!(t.is_empty());
        assert_eq!(t.class_count(), 0);
        assert_eq!(t.class_of("$x"), None);
    }

    #[test]
    fn test_pair_creation() {
        let mut t: ClassTable<MettaValue> = ClassTable::default();
        let id = t.insert_equivalence_pair("$x".into(), "$y".into());
        assert_eq!(t.class_of("$x"), Some(id));
        assert_eq!(t.class_of("$y"), Some(id));
        let members = t.class_members(id).expect("class members");
        assert_eq!(members.len(), 2);
    }

    #[test]
    fn test_value_set_and_get() {
        let mut t: ClassTable<MettaValue> = ClassTable::default();
        let id = t.insert_equivalence_pair("$x".into(), "$y".into());
        t.set_class_value(id, make_long(5)).expect("set value");
        assert_eq!(t.class_value(id), Some(&make_long(5)));
    }

    #[test]
    fn test_value_conflict() {
        let mut t: ClassTable<MettaValue> = ClassTable::default();
        let id = t.insert_equivalence_pair("$x".into(), "$y".into());
        t.set_class_value(id, make_long(5)).expect("first set");
        let err = t
            .set_class_value(id, make_long(7))
            .expect_err("conflict expected");
        assert!(matches!(err, MergeConflict::Incompatible));
    }

    #[test]
    fn test_union_two_classes() {
        let mut t: ClassTable<MettaValue> = ClassTable::default();
        let a = t.insert_equivalence_pair("$x".into(), "$y".into());
        let b = t.insert_equivalence_pair("$z".into(), "$w".into());
        let root = t.union(a, b).expect("union ok");
        // After union, all 4 names point to the same root.
        assert_eq!(t.find(a), root);
        assert_eq!(t.find(b), root);
        assert_eq!(t.class_of("$x").map(|c| t.find(c)), Some(root));
        assert_eq!(t.class_of("$w").map(|c| t.find(c)), Some(root));
    }

    #[test]
    fn test_union_with_value_propagation() {
        let mut t: ClassTable<MettaValue> = ClassTable::default();
        let a = t.insert_equivalence_pair("$x".into(), "$y".into());
        let b = t.insert_equivalence_pair("$z".into(), "$w".into());
        t.set_class_value(a, make_long(5)).expect("set value");
        let root = t.union(a, b).expect("union ok");
        // Merged class should carry the value.
        assert_eq!(t.class_value(root), Some(&make_long(5)));
    }

    #[test]
    fn test_union_value_conflict() {
        let mut t: ClassTable<MettaValue> = ClassTable::default();
        let a = t.insert_equivalence_pair("$x".into(), "$y".into());
        let b = t.insert_equivalence_pair("$z".into(), "$w".into());
        t.set_class_value(a, make_long(5)).expect("set a");
        t.set_class_value(b, make_long(7)).expect("set b");
        assert!(matches!(t.union(a, b), Err(MergeConflict::Incompatible)));
    }

    #[test]
    fn test_bindings_with_classes_resolve_original_var() {
        let mut b: BindingsWithClasses<MettaValue> = BindingsWithClasses::new();
        b.insert_equivalence("$x".into(), "$y".into())
            .expect("insert eq");
        // Value-less class: resolve returns ORIGINAL lookup-key.
        // resolve("$x") returns Some(atom("$x")) — NOT atom("$y").
        let rx = b.resolve("$x").expect("class lookup");
        let ry = b.resolve("$y").expect("class lookup");
        assert_eq!(rx.as_atom(), Some("$x"));
        assert_eq!(ry.as_atom(), Some("$y"));
        // Sanity: make_atom helper used; suppress dead-code warning.
        let _ = make_atom("$dummy");
    }

    #[test]
    fn test_bindings_with_classes_value_propagation() {
        let mut b: BindingsWithClasses<MettaValue> = BindingsWithClasses::new();
        b.insert_equivalence("$x".into(), "$y".into())
            .expect("insert eq");
        b.insert_value("$x".into(), make_long(5))
            .expect("install value");
        // Both class members resolve to the value.
        assert_eq!(b.resolve("$x"), Some(make_long(5)));
        assert_eq!(b.resolve("$y"), Some(make_long(5)));
    }
}
