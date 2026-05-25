//! WAM trail-based binding store (clause-scoped, mutable, trail-backed).
//!
//! See `docs/wam/trail-binding-model.md` for the full design.
//!
//! This is **Increment 1 (inert scaffolding)**: the store, its union-find +
//! trail operations, and the `GenericBindings` interface (`snapshot_scoped` /
//! `seed_from`) — fully unit-tested but NOT yet wired into evaluation. Later
//! increments read/write it at the cache-hit, rule-match, and fork sites so
//! that variable bindings become clause-GLOBAL (held in this store, not in
//! per-result value sidecars), which makes the values-only caches
//! binding-neutral and eliminates the flaky "binding dropped on a cache hit"
//! class (PLN `Direct.metta` tests 2/3).
//!
//! Representation mirrors the bytecode VM's trail (`bytecode/vm/mod.rs` +
//! `vm/types.rs`): a union-find over scoped variable cells keyed by
//! `(ScopeId, name)`, plus a trail recording each mutation so a choice point
//! can `mark()` the trail height and `undo_to(mark)` on backtrack.

// Increment 1 is inert: the store is unit-tested but not yet called from the
// evaluator, so its public API is dead code until Increment 2 wires it in.
#![allow(dead_code)]

use std::collections::HashMap;

use crate::backend::models::generic_bindings::ScopeId;
use crate::backend::models::{GenericBindings, MettaValue};

/// Index into [`BindingStore::cells`].
pub type CellId = usize;

/// A union-find cell: either an (possibly aliased) unbound variable or a bound
/// value. `MettaValue` is `Copy` (an 8-byte GC handle), so cells are cheap to
/// store and to snapshot onto the trail.
#[derive(Clone, Copy, Debug)]
enum Cell {
    /// Unbound variable. `reference == self` ⇒ a union-find root; otherwise an
    /// alias pointing at another cell (the variable was unified with another
    /// variable, not yet with a value).
    Var { reference: CellId },
    /// Bound to a (possibly partially-ground) term.
    Bound { value: MettaValue },
}

/// Undo-log entry. On backtrack the trail is popped and each entry inverted,
/// restoring the cell to its pre-mutation state. Mirrors
/// `vm::types::TrailEntry`.
#[derive(Clone, Copy, Debug)]
enum TrailEntry {
    /// The cell was a fresh root `Var { reference: self }`; undo resets it.
    NewBinding { cell: CellId },
    /// The cell held `old`; undo restores it.
    Rebinding { cell: CellId, old: Cell },
}

/// Clause-scoped binding store. One instance is installed per trampoline
/// activation (thread-local); nested activations get a fresh store.
#[derive(Default, Debug)]
pub struct BindingStore {
    /// Intern `(scope, name)` → cell. Names are interned as owned strings;
    /// the common case is a handful of variables per clause.
    cell_of: HashMap<(ScopeId, String), CellId>,
    /// Cell table indexed by `CellId`.
    cells: Vec<Cell>,
    /// Undo log; `mark()`/`undo_to()` snapshot and rewind its height.
    trail: Vec<TrailEntry>,
}

impl BindingStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the store (reused when an activation finishes and the thread-local
    /// is recycled). Cheaper than re-allocating the maps/vecs.
    pub fn clear(&mut self) {
        self.cell_of.clear();
        self.cells.clear();
        self.trail.clear();
    }

    /// Intern `(scope, name)`, creating a fresh unbound root cell if absent.
    pub fn cell_for(&mut self, scope: ScopeId, name: &str) -> CellId {
        if let Some(&id) = self.cell_of.get(&(scope, name.to_string())) {
            return id;
        }
        let id = self.cells.len();
        self.cells.push(Cell::Var { reference: id }); // self-ref ⇒ unbound root
        self.cell_of.insert((scope, name.to_string()), id);
        id
    }

    /// Union-find find with path compression. Returns the representative cell:
    /// the unbound root (a `Var` self-ref) or a `Bound` cell. Iterative — no
    /// recursion (stack-safety mandate).
    fn find(&mut self, start: CellId) -> CellId {
        // Walk to the representative.
        let mut root = start;
        loop {
            match self.cells[root] {
                Cell::Var { reference } if reference != root => root = reference,
                _ => break, // unbound root (self-ref) or Bound
            }
        }
        // Path-compress: point every cell on the path directly at `root`.
        let mut cur = start;
        while let Cell::Var { reference } = self.cells[cur] {
            if reference == cur || reference == root {
                break;
            }
            self.cells[cur] = Cell::Var { reference: root };
            cur = reference;
        }
        root
    }

    /// Resolve `(scope_chain, name)` to a bound value, if any. Tries each scope
    /// in `scope_chain` order (typically `[dispatch_scope, ROOT_SCOPE]`); the
    /// first scope with an interned cell whose representative is `Bound` wins.
    /// Does NOT create cells (read-only lookup uses a transient resolve).
    pub fn lookup(&mut self, scope_chain: &[ScopeId], name: &str) -> Option<MettaValue> {
        for &scope in scope_chain {
            if let Some(&id) = self.cell_of.get(&(scope, name.to_string())) {
                let rep = self.find(id);
                if let Cell::Bound { value } = self.cells[rep] {
                    return Some(value);
                }
            }
        }
        None
    }

    /// Bind `(scope, name)`'s representative cell to `value`, recording the
    /// prior state on the trail. If the representative is already bound, this
    /// rebinds it (trailed) — callers that require unification semantics should
    /// check `lookup` first.
    pub fn bind(&mut self, scope: ScopeId, name: &str, value: MettaValue) {
        let id = self.cell_for(scope, name);
        let rep = self.find(id);
        match self.cells[rep] {
            Cell::Var { reference } if reference == rep => {
                // Fresh root → record NewBinding (undo resets to self-ref).
                self.trail.push(TrailEntry::NewBinding { cell: rep });
            }
            other => {
                // Aliased var or already-bound → record the old cell verbatim.
                self.trail.push(TrailEntry::Rebinding {
                    cell: rep,
                    old: other,
                });
            }
        }
        self.cells[rep] = Cell::Bound { value };
    }

    /// Alias the variable `(scope_a, name_a)` to `(scope_b, name_b)` (var-var
    /// unification): point a's representative at b's representative, trailed.
    /// If a's representative is already bound, this is a no-op alias attempt and
    /// the caller should instead `bind` b to a's value — but for Increment 1
    /// this records the union faithfully for the unbound case.
    pub fn union(&mut self, scope_a: ScopeId, name_a: &str, scope_b: ScopeId, name_b: &str) {
        let a = self.cell_for(scope_a, name_a);
        let b = self.cell_for(scope_b, name_b);
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        // Only alias an unbound root; if ra is bound, propagate its value to b.
        match self.cells[ra] {
            Cell::Var { reference } if reference == ra => {
                self.trail.push(TrailEntry::NewBinding { cell: ra });
                self.cells[ra] = Cell::Var { reference: rb };
            }
            Cell::Bound { value } => {
                // ra bound → bind rb to the same value (trailed).
                match self.cells[rb] {
                    Cell::Var { reference } if reference == rb => {
                        self.trail.push(TrailEntry::NewBinding { cell: rb });
                    }
                    other => self.trail.push(TrailEntry::Rebinding { cell: rb, old: other }),
                }
                self.cells[rb] = Cell::Bound { value };
            }
            other => {
                self.trail.push(TrailEntry::Rebinding { cell: ra, old: other });
                self.cells[ra] = Cell::Var { reference: rb };
            }
        }
    }

    /// Snapshot the current trail height (a WAM choice-point mark).
    #[inline]
    pub fn mark(&self) -> usize {
        self.trail.len()
    }

    /// Rewind the trail to `mark`, inverting each popped entry. Restores all
    /// bindings made since the mark (backtracking / branch death).
    pub fn undo_to(&mut self, mark: usize) {
        while self.trail.len() > mark {
            match self.trail.pop().expect("trail height checked") {
                TrailEntry::NewBinding { cell } => {
                    self.cells[cell] = Cell::Var { reference: cell };
                }
                TrailEntry::Rebinding { cell, old } => {
                    self.cells[cell] = old;
                }
            }
        }
    }

    /// Materialize the bound variables at `scope` into a `GenericBindings`
    /// (the sidecar wire format used at activation/output boundaries). Only
    /// cells interned at `scope` whose representative is `Bound` are emitted.
    pub fn snapshot_scoped(&mut self, scope: ScopeId) -> GenericBindings<MettaValue> {
        // Collect names first to avoid borrow conflict with `find`.
        let names: Vec<String> = self
            .cell_of
            .keys()
            .filter(|(s, _)| *s == scope)
            .map(|(_, n)| n.clone())
            .collect();
        let mut out = GenericBindings::new();
        for name in names {
            if let Some(value) = self.lookup(&[scope], &name) {
                out.insert_scoped(scope, crate::backend::models::BindingName::new(&name), value);
            }
        }
        out
    }

    /// Seed the store from an existing `GenericBindings` (the activation's
    /// inbound carrying). Each scoped entry becomes a bound cell.
    pub fn seed_from(&mut self, bindings: &GenericBindings<MettaValue>) {
        for (scope, name, value) in bindings.iter_full() {
            self.bind(scope, name, *value);
        }
    }

    /// Number of interned cells (for tests / diagnostics).
    #[inline]
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::generic_bindings::ROOT_SCOPE;
    use crate::backend::models::global_factory;
    use crate::backend::models::MettaValueFactory;

    fn atom(s: &str) -> MettaValue {
        global_factory().atom(s)
    }

    #[test]
    fn bind_then_lookup() {
        let mut store = BindingStore::new();
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$who"), None);
        store.bind(ROOT_SCOPE, "$who", atom("a"));
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$who"), Some(atom("a")));
        // A different name is independent.
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$x"), None);
    }

    #[test]
    fn scope_chain_resolution() {
        let mut store = BindingStore::new();
        let dispatch = crate::backend::models::generic_bindings::allocate_scope_id();
        // $who is bound only at ROOT_SCOPE; lookup via [dispatch, ROOT] finds it.
        store.bind(ROOT_SCOPE, "$who", atom("a"));
        assert_eq!(store.lookup(&[dispatch, ROOT_SCOPE], "$who"), Some(atom("a")));
        // A binding at the dispatch scope shadows nothing (different name).
        store.bind(dispatch, "$rule", atom("b"));
        assert_eq!(store.lookup(&[dispatch, ROOT_SCOPE], "$rule"), Some(atom("b")));
    }

    #[test]
    fn union_propagates_value() {
        let mut store = BindingStore::new();
        let dispatch = crate::backend::models::generic_bindings::allocate_scope_id();
        // Unify rule-local $x with query $who, then bind $who=a; $x resolves to a.
        store.union(dispatch, "$x", ROOT_SCOPE, "$who");
        store.bind(ROOT_SCOPE, "$who", atom("a"));
        assert_eq!(store.lookup(&[dispatch], "$x"), Some(atom("a")));
    }

    #[test]
    fn mark_undo_roundtrip() {
        let mut store = BindingStore::new();
        store.bind(ROOT_SCOPE, "$keep", atom("k"));
        let m = store.mark();
        store.bind(ROOT_SCOPE, "$who", atom("a"));
        store.bind(ROOT_SCOPE, "$y", atom("b"));
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$who"), Some(atom("a")));
        store.undo_to(m);
        // Bindings made after the mark are gone; the earlier one survives.
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$who"), None);
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$y"), None);
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$keep"), Some(atom("k")));
    }

    #[test]
    fn undo_restores_union() {
        let mut store = BindingStore::new();
        let dispatch = crate::backend::models::generic_bindings::allocate_scope_id();
        let m = store.mark();
        store.union(dispatch, "$x", ROOT_SCOPE, "$who");
        store.bind(ROOT_SCOPE, "$who", atom("a"));
        assert_eq!(store.lookup(&[dispatch], "$x"), Some(atom("a")));
        store.undo_to(m);
        assert_eq!(store.lookup(&[dispatch], "$x"), None);
        assert_eq!(store.lookup(&[ROOT_SCOPE], "$who"), None);
    }

    #[test]
    fn snapshot_and_seed_roundtrip() {
        let mut store = BindingStore::new();
        store.bind(ROOT_SCOPE, "$who", atom("a"));
        store.bind(ROOT_SCOPE, "$y", atom("b"));
        let snap = store.snapshot_scoped(ROOT_SCOPE);
        assert_eq!(snap.get("$who"), Some(&atom("a")));
        assert_eq!(snap.get("$y"), Some(&atom("b")));
        // Seeding a fresh store from the snapshot reproduces the bindings.
        let mut store2 = BindingStore::new();
        store2.seed_from(&snap);
        assert_eq!(store2.lookup(&[ROOT_SCOPE], "$who"), Some(atom("a")));
        assert_eq!(store2.lookup(&[ROOT_SCOPE], "$y"), Some(atom("b")));
    }
}
