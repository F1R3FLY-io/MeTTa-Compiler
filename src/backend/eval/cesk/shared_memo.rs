//! Shared cross-thread cycle-detection substrate for #309/#266 (coordination-only).
//!
//! The architectural fix replaces the cross-thread-unsafe thread-local cycle
//! detection with a SHARED, content-keyed COORDINATION store that holds ONLY
//! `u64` derivation-ids — NEVER a `MettaValue`. Because it holds no `Addr`-bearing
//! value, it references zero GC-managed memory: it contributes nothing to the
//! index-GC root set and is invisible to the collector — **GC-safe by
//! construction, no rooting, ever** (the index-GC is mark-from-roots-only;
//! `project_roots_to_addrs` marks only the supplied roots, the hash-cons `retain`
//! follows the mark — so a structure that holds no `Addr` can never be the sole
//! reachability path to a value, hence needs no root). See
//! `docs/design/SHARED_MEMO_STORE_309.md`.
//!
//! What it fixes: under parallel fanout, a recursive thunk/subgoal that re-enters
//! a content hash currently IN-FLIGHT on the SAME logical derivation (or an
//! ANCESTOR of it) is a genuine cross-thread CYCLE and must be cut to the fixpoint
//! EMPTY; a re-entry of a hash a NON-ANCESTOR sibling is concurrently deriving is
//! a shared dependency and must NOT be cut (the M4 over-cut). The discriminator is
//! the derivation LINEAGE: `is_ancestor(owner, me)`.
//!
//! Two parts:
//!  - **Lineage**: a `derivation_id` per (possibly fanned-out) derivation; a worker
//!    enters a fresh child id at dispatch (transported like the active-eval seed).
//!    [`is_ancestor`] walks the parent chain (a forest by construction, so the walk
//!    terminates).
//!  - **Coordination store**: `content-hash -> in-flight owner id`. The cut decision
//!    ([`SharedMemoStore::lookup_or_cut`]) cuts iff `owner == me || is_ancestor(owner,
//!    me)`, otherwise re-derives locally. No values, no await, no parking — a shared
//!    dependency re-derives (bounded-width: the per-worker thread-local memo still
//!    caps recursion depth), a perf cost, never a correctness one.

use dashmap::DashMap;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Monotonic source of fresh derivation ids. `0` is reserved for "top-level / no
/// parent". Ids stay below [`INFLIGHT_BIT`] in any realistic run (they pack into a
/// memo slot's low 63 bits).
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
/// in practice), with a defensive cap. Consulted by the store's cut decision:
/// owner is an ancestor of the requester ⇒ a genuine cross-thread cycle ⇒ cut;
/// otherwise a shared in-flight dependency ⇒ local-eval, never cut.
pub fn is_ancestor(ancestor: u64, descendant: u64) -> bool {
    if ancestor == descendant {
        return true;
    }
    if ancestor == 0 {
        // The top-level (0) is the root of every lineage, but an id-0-owned
        // in-flight hash is handled by LOCAL-EVAL (a worker re-derives it under its
        // own thread-local Blackhole, which is bounded), not by a cross-thread cut.
        // Treat 0 as not-a-cutting-ancestor so we never cut against the root.
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
    // shallow); treat as not-an-ancestor (local-eval is always safe).
    false
}

// =========================================================================
// The coordination store: `content-hash -> in-flight owner id`. Holds NO
// `MettaValue` — only `u64`s — so it is GC-safe by construction (see the module
// doc). Wired into the thunk/subgoal channels behind `worker_ever_spawned()`.
// =========================================================================

/// High bit of a memo slot: set ⇒ the hash is IN-FLIGHT, low 63 bits = owner id.
/// Clear (slot == 0) ⇒ EMPTY. Packing `(in-flight, owner)` into ONE atomic word
/// makes the pair read/written atomically — no torn read between an in-flight flag
/// and a separate owner field. Derivation ids stay below this in any real run.
const INFLIGHT_BIT: u64 = 1 << 63;

/// One coordination cell for a content hash: a single atomic word holding either
/// EMPTY (`0`) or IN-FLIGHT (`INFLIGHT_BIT | owner_id`). No value, no lifecycle
/// beyond claim/release.
pub struct SharedMemoEntry {
    slot: AtomicU64,
}

impl SharedMemoEntry {
    fn new() -> Self {
        SharedMemoEntry {
            slot: AtomicU64::new(0),
        }
    }

    /// The in-flight owner id, or `None` if EMPTY. A single atomic load — the
    /// `(in-flight, owner)` pair is consistent.
    #[inline]
    pub fn inflight_owner(&self) -> Option<u64> {
        let w = self.slot.load(Ordering::Acquire);
        if w & INFLIGHT_BIT != 0 {
            Some(w & !INFLIGHT_BIT)
        } else {
            None
        }
    }

    /// CAS EMPTY → IN-FLIGHT(owner = `my_id`) in one atomic step. Returns `true`
    /// iff THIS caller won the claim (and must therefore derive, then [`complete`]).
    /// `my_id` must fit in 63 bits (derivation ids do).
    #[inline]
    pub fn try_claim(&self, my_id: u64) -> bool {
        debug_assert!(my_id < INFLIGHT_BIT, "derivation id overflows the memo slot");
        self.slot
            .compare_exchange(
                0,
                INFLIGHT_BIT | my_id,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Release the claim (IN-FLIGHT → EMPTY) when the owner finishes (or unwinds).
    /// Idempotent. The entry stays in the map (re-claimable); the map is cleared at
    /// `!` boundaries.
    #[inline]
    pub fn complete(&self) {
        self.slot.store(0, Ordering::Release);
    }
}

/// Which content-hash namespace an operation targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoChannel {
    Thunk,
    Subgoal,
}

/// Outcome of [`SharedMemoStore::lookup_or_cut`]:
/// - `Claimed` — I won the claim; derive locally, then [`SharedMemoStore::complete`].
/// - `Cut` — a genuine cycle (self or ancestor); contribute the fixpoint EMPTY,
///   exactly as the thread-local `Blackhole` would.
/// - `LocalEval` — a shared dependency (a non-ancestor sibling owns it); derive
///   locally WITHOUT touching the store. Always safe, never blocks, never over-cuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoLookup {
    Claimed,
    Cut,
    LocalEval,
}

/// The process-wide coordination store: in-flight owner per content hash, per
/// channel. Holds only `u64`s.
pub struct SharedMemoStore {
    thunks: DashMap<u64, Arc<SharedMemoEntry>>,
    subgoals: DashMap<u64, Arc<SharedMemoEntry>>,
}

impl SharedMemoStore {
    fn new() -> Self {
        SharedMemoStore {
            thunks: DashMap::new(),
            subgoals: DashMap::new(),
        }
    }

    #[inline]
    fn map(&self, channel: MemoChannel) -> &DashMap<u64, Arc<SharedMemoEntry>> {
        match channel {
            MemoChannel::Thunk => &self.thunks,
            MemoChannel::Subgoal => &self.subgoals,
        }
    }

    /// The (lazily created) entry for `hash` in `channel`.
    pub fn entry(&self, channel: MemoChannel, hash: u64) -> Arc<SharedMemoEntry> {
        self.map(channel)
            .entry(hash)
            .or_insert_with(|| Arc::new(SharedMemoEntry::new()))
            .clone()
    }

    /// Drop all entries (a top-level `!` boundary; the thread-local tables are
    /// cleared there too). Cheap when empty.
    pub fn clear(&self) {
        self.thunks.clear();
        self.subgoals.clear();
    }

    /// Number of live entries in a channel (diagnostics / tests).
    pub fn len(&self, channel: MemoChannel) -> usize {
        self.map(channel).len()
    }

    /// The coordination-only cross-thread cut protocol. `my_id` is the caller's
    /// derivation id ([`current_derivation_id`]). Holds no value: a re-entry of an
    /// in-flight hash is CUT iff its owner is the caller itself or a genuine
    /// ancestor (a real cross-thread fixpoint cycle); a non-ancestor (shared
    /// dependency) is `LocalEval` — never cut, never blocked.
    pub fn lookup_or_cut(&self, channel: MemoChannel, hash: u64, my_id: u64) -> MemoLookup {
        let e = self.entry(channel, hash);
        loop {
            match e.inflight_owner() {
                None => {
                    if e.try_claim(my_id) {
                        return MemoLookup::Claimed;
                    }
                    // Lost the claim race — re-observe (now in-flight under another
                    // owner, or briefly EMPTY again).
                    continue;
                }
                Some(owner) => {
                    if owner == my_id || is_ancestor(owner, my_id) {
                        return MemoLookup::Cut;
                    }
                    return MemoLookup::LocalEval;
                }
            }
        }
    }

    /// Release the in-flight claim for a hash this caller `Claimed` (on completion
    /// or unwind). Idempotent.
    pub fn complete(&self, channel: MemoChannel, hash: u64) {
        self.entry(channel, hash).complete();
    }
}

/// The process-wide store handle (lazily initialized). `clear` is called at `!`
/// boundaries.
pub fn store() -> &'static SharedMemoStore {
    static STORE: OnceLock<SharedMemoStore> = OnceLock::new();
    STORE.get_or_init(SharedMemoStore::new)
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

    #[test]
    fn entry_claim_complete_reclaim() {
        let e = SharedMemoEntry::new();
        assert_eq!(e.inflight_owner(), None, "starts EMPTY");
        assert!(e.try_claim(7), "first claim wins");
        assert_eq!(e.inflight_owner(), Some(7), "in-flight, owner 7");
        assert!(!e.try_claim(9), "second claim loses (already in-flight)");
        e.complete();
        assert_eq!(e.inflight_owner(), None, "EMPTY after complete");
        assert!(e.try_claim(9), "re-claimable after complete");
        assert_eq!(e.inflight_owner(), Some(9));
    }

    #[test]
    fn store_entry_disjoint_channels_and_clear() {
        let s = SharedMemoStore::new();
        let e1 = s.entry(MemoChannel::Thunk, 100);
        let e2 = s.entry(MemoChannel::Thunk, 100);
        assert!(Arc::ptr_eq(&e1, &e2), "same hash => same entry");
        assert_eq!(s.len(MemoChannel::Thunk), 1);
        let _ = s.entry(MemoChannel::Subgoal, 100);
        assert_eq!(s.len(MemoChannel::Subgoal), 1, "channels are disjoint");
        s.clear();
        assert_eq!(s.len(MemoChannel::Thunk), 0);
        assert_eq!(s.len(MemoChannel::Subgoal), 0);
    }

    #[test]
    fn protocol_empty_claims_then_self_cycle_cuts() {
        let s = SharedMemoStore::new();
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Thunk, 2, 10),
            MemoLookup::Claimed
        );
        // The SAME derivation re-enters its own in-flight hash => cut.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 2, 10), MemoLookup::Cut);
    }

    #[test]
    fn protocol_cross_thread_cycle_cuts() {
        let s = SharedMemoStore::new();
        let a = DerivationScope::enter_child(0);
        let aid = a.id();
        let b = DerivationScope::enter_child(aid); // B is a child of A
        let bid = b.id();
        // A claims hash 4 (in-flight, owner = A).
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Thunk, 4, aid),
            MemoLookup::Claimed
        );
        // B re-enters A's in-flight hash. A is a genuine ANCESTOR of B =>
        // CUT (a real cross-thread fixpoint cycle). No wait-for graph needed.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 4, bid), MemoLookup::Cut);
        drop(b);
        drop(a);
    }

    #[test]
    fn protocol_shared_dependency_local_evals() {
        let s = SharedMemoStore::new();
        // Two SIBLINGS (both children of the root 0), neither an ancestor of the
        // other.
        let s1 = DerivationScope::enter_child(0);
        let s1id = s1.id();
        let s2 = DerivationScope::enter_child(0);
        let s2id = s2.id();
        // S1 claims hash 5 (in-flight, owner = S1).
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Thunk, 5, s1id),
            MemoLookup::Claimed
        );
        // S2 needs the same hash. S1 is NOT an ancestor of S2 => a shared
        // dependency => LocalEval, NEVER cut (the M4 over-cut this prevents).
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Thunk, 5, s2id),
            MemoLookup::LocalEval
        );
        drop(s2);
        drop(s1);
    }

    #[test]
    fn protocol_complete_releases_for_reclaim() {
        let s = SharedMemoStore::new();
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Thunk, 6, 10),
            MemoLookup::Claimed
        );
        s.complete(MemoChannel::Thunk, 6);
        // After the owner completes, the hash is EMPTY again => a later derivation
        // claims it fresh (no stale cut against a finished owner).
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Thunk, 6, 20),
            MemoLookup::Claimed
        );
    }
}
