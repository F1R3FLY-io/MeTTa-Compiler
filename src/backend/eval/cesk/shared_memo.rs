//! Shared cross-thread memoization substrate for #309/#266.
//!
//! **Step 1 (this file, initial): derivation lineage.** The architectural fix
//! (see `docs/post-mortems/PARALLEL_FANOUT_TABLING_309_266.md` §7) replaces the
//! cross-thread-unsafe thread-local thunk/subgoal cycle detection with a shared
//! content-keyed memo store whose cut decision needs a discriminator the reverted
//! M4 seed lacked: **is the owner of an in-flight thunk an ANCESTOR of the
//! requester** (a genuine cross-thread cycle → cut) **or a sibling/cousin** (a
//! shared in-flight dependency → await / local-eval, never cut)?
//!
//! A `derivation_id` names one logical (possibly fanned-out) derivation. When a
//! worker is dispatched, it enters a FRESH id whose parent is the dispatching
//! thread's current id (captured at the fanout boundary, transported exactly like
//! the active-eval seed). [`is_ancestor`] walks the parent chain. The lineage is a
//! forest by construction (each id has exactly one parent, assigned once at
//! creation, never cyclic), so the walk terminates.
//!
//! This step is pure plumbing: the ids are assigned and the lineage is recorded,
//! but no lookup consults [`is_ancestor`] yet (the shared store wires it in a
//! later step), so behavior — including FANOUT=0 byte-identity — is unchanged.

use dashmap::DashMap;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Monotonic source of fresh derivation ids. `0` is reserved for "top-level / no
/// parent".
static NEXT_DERIVATION_ID: AtomicU64 = AtomicU64::new(1);

/// `id -> parent id`, for ACTIVE derivations only — an entry is inserted when a
/// [`DerivationScope`] enters and removed when it drops, so the map holds only
/// live lineage and does not grow unboundedly. Lazily initialized (no cost until
/// the first fanout).
fn parent_map() -> &'static DashMap<u64, u64> {
    static MAP: OnceLock<DashMap<u64, u64>> = OnceLock::new();
    MAP.get_or_init(DashMap::new)
}

thread_local! {
    /// This thread's current derivation id (`0` = top-level / not in a fanned-out
    /// worker).
    static CURRENT_DERIVATION_ID: Cell<u64> = const { Cell::new(0) };
}

/// The dispatching thread's current derivation id — captured at a fanout boundary
/// (before the spawn loop) and transported to each worker as its parent.
#[inline]
pub fn current_derivation_id() -> u64 {
    CURRENT_DERIVATION_ID.with(|c| c.get())
}

/// RAII guard installing a FRESH derivation id (a child of `parent`) for a
/// fanned-out worker; on drop it removes the lineage entry and restores the
/// previous id. Mirrors `SeedActiveScope`/`WorkerCaptureScope`. `!Send` — it
/// manipulates thread-locals and must not cross threads.
pub struct DerivationScope {
    id: u64,
    prev: u64,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl DerivationScope {
    /// Enter a fresh child derivation of `parent` (the dispatching thread's id,
    /// captured by [`current_derivation_id`] before dispatch). Returns the guard;
    /// the fresh id is this worker's [`current_derivation_id`] for the guard's
    /// lifetime.
    pub fn enter_child(parent: u64) -> Self {
        let id = NEXT_DERIVATION_ID.fetch_add(1, Ordering::Relaxed);
        parent_map().insert(id, parent);
        let prev = CURRENT_DERIVATION_ID.with(|c| {
            let p = c.get();
            c.set(id);
            p
        });
        DerivationScope {
            id,
            prev,
            _not_send: std::marker::PhantomData,
        }
    }

    /// This scope's fresh derivation id.
    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for DerivationScope {
    fn drop(&mut self) {
        parent_map().remove(&self.id);
        CURRENT_DERIVATION_ID.with(|c| c.set(self.prev));
    }
}

/// Is `ancestor` on the parent chain of `descendant` (reflexive — equality
/// counts)? `O(depth)`; depth is bounded by the fanout depth (`MAX_PARALLEL_DEPTH`
/// in practice), with a defensive cap. Consulted by the shared store's cut
/// decision (a later step): owner is an ancestor of the requester ⇒ a genuine
/// cross-thread cycle ⇒ cut; otherwise a shared in-flight dependency ⇒
/// await / local-eval, never cut.
pub fn is_ancestor(ancestor: u64, descendant: u64) -> bool {
    if ancestor == descendant {
        return true;
    }
    if ancestor == 0 {
        // The top-level (0) is an ancestor of every derivation; but id 0 is never
        // an in-flight thunk owner (the top-level thread owns thunks under its own
        // nonzero scope once it fans out), so treat 0 conservatively as "not a
        // cutting ancestor" to avoid cutting against the root.
        return false;
    }
    let map = parent_map();
    let mut cur = descendant;
    for _ in 0..1024 {
        match map.get(&cur) {
            Some(p) => {
                let parent = *p;
                if parent == ancestor {
                    return true;
                }
                if parent == 0 {
                    return false;
                }
                cur = parent;
            }
            None => return false,
        }
    }
    // Defensive cap reached (should be unreachable — lineage is acyclic and
    // shallow); treat as not-an-ancestor (await/local-eval is always safe).
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflexive_and_chain() {
        // Build a small lineage: root(scope a) -> child(b) -> grandchild(c).
        let a = DerivationScope::enter_child(0);
        let aid = a.id();
        let b = DerivationScope::enter_child(aid);
        let bid = b.id();
        let c = DerivationScope::enter_child(bid);
        let cid = c.id();

        assert!(is_ancestor(aid, aid), "reflexive");
        assert!(is_ancestor(aid, bid), "parent");
        assert!(is_ancestor(aid, cid), "grandparent");
        assert!(is_ancestor(bid, cid), "parent");
        assert!(!is_ancestor(cid, aid), "not a descendant-as-ancestor");
        assert!(!is_ancestor(bid, aid), "sibling/uncle direction");
    }

    #[test]
    fn dropped_scope_is_forgotten() {
        let id;
        {
            let s = DerivationScope::enter_child(0);
            id = s.id();
            assert!(parent_map().contains_key(&id));
        }
        assert!(!parent_map().contains_key(&id), "entry removed on drop");
    }
}
