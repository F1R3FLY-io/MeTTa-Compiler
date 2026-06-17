//! Shared cross-thread cycle-detection substrate for #309/#266 (coordination-only).
//!
//! The architectural fix replaces the cross-thread-unsafe thread-local cycle
//! detection with a SHARED, content-keyed COORDINATION store that holds ONLY
//! `u64` derivation-ids — NEVER a `MettaValue`. Because it holds no `Addr`-bearing
//! value, it references zero GC-managed memory: it contributes nothing to the
//! index-GC root set and is invisible to the collector — **GC-safe by
//! construction, no rooting, ever** (the index-GC is mark-from-roots-only, so a
//! structure that holds no `Addr` can never be the sole reachability path to a
//! value, hence needs no root). See `docs/design/SHARED_MEMO_STORE_309.md`.
//!
//! What it fixes: under parallel fanout, a recursive thunk/subgoal that re-enters
//! a content hash currently IN-FLIGHT under an ANCESTOR derivation is a genuine
//! cross-thread CYCLE and must be cut to the fixpoint EMPTY; a re-entry of a hash
//! a NON-ANCESTOR sibling is concurrently deriving is a shared dependency and must
//! NOT be cut (the M4 over-cut). The discriminator is the derivation LINEAGE:
//! `is_proper_ancestor(owner, me)`.
//!
//! Per content hash the store keeps the SET of currently in-flight owner ids — one
//! per concurrently-in-flight lineage. The MULTI-owner set is load-bearing: when a
//! non-ancestor sibling re-derives a shared dependency, it REGISTERS itself, so its
//! OWN descendants cut against it; with a single owner those descendants would be
//! cut by neither the store (the original owner is their uncle, not ancestor) nor
//! the thread-local (each fanout level is a fresh worker) — and the re-derivation
//! fanout would run away. The protocol ([`SharedMemoStore::lookup_or_cut`]):
//!   - a PROPER ANCESTOR is in the set  -> Cut (genuine cross-thread fixpoint cycle)
//!   - I am already in the set          -> LocalEval (same-thread re-entry; the
//!                                         thread-local Blackhole owns that timing)
//!   - otherwise                        -> register me, Claimed (derive; my
//!                                         descendants cut against me; complete
//!                                         removes me)
//! No values, no await, no parking — bounded-width re-derivation, never a runaway.

use dashmap::DashMap;
use smallvec::SmallVec;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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

/// Is `ancestor` a PROPER ancestor of `descendant` (NOT reflexive — `a == d`
/// returns `false`)? `O(depth)`; depth is bounded by the fanout depth, with a
/// defensive cap. Used by the cut decision: a PROPER ancestor of the requester is
/// in-flight on the same hash ⇒ a genuine cross-thread cycle ⇒ cut.
pub fn is_proper_ancestor(ancestor: u64, descendant: u64) -> bool {
    if ancestor == descendant || ancestor == 0 {
        // 0 is the root of every lineage; an id-0-owned in-flight hash is handled
        // by re-derivation (bounded by the thread-local), not a cross-thread cut.
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
    // Defensive cap reached (lineage is acyclic and shallow); not-an-ancestor.
    false
}

// =========================================================================
// The coordination store: `content-hash -> SET of in-flight owner ids`. Holds NO
// `MettaValue` — only `u64`s — so it is GC-safe by construction (module doc).
// Wired into the thunk/subgoal channels behind `worker_ever_spawned()`.
// =========================================================================

/// One coordination cell for a content hash: the set of currently in-flight owner
/// derivation-ids (one per concurrently-in-flight lineage). Bounded by concurrent
/// fanout width. No value, no lifecycle beyond register/release.
pub struct SharedMemoEntry {
    owners: Mutex<SmallVec<[u64; 4]>>,
}

impl SharedMemoEntry {
    fn new() -> Self {
        SharedMemoEntry {
            owners: Mutex::new(SmallVec::new()),
        }
    }
}

/// Which content-hash namespace an operation targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoChannel {
    Thunk,
    Subgoal,
}

/// Outcome of [`SharedMemoStore::lookup_or_cut`]:
/// - `Claimed` — I registered as an owner; derive locally, then
///   [`SharedMemoStore::complete`] with the SAME id (`current_derivation_id`).
/// - `Cut` — a proper ancestor is deriving this hash (a genuine cross-thread
///   fixpoint cycle); contribute the fixpoint EMPTY, like the thread-local Blackhole.
/// - `LocalEval` — a same-thread re-entry (I'm already an owner; the thread-local
///   Blackhole handles it). Derive locally WITHOUT touching the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoLookup {
    Claimed,
    Cut,
    LocalEval,
}

/// The process-wide coordination store: in-flight owner SET per content hash, per
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

    /// The coordination-only cross-thread cut protocol. `me` is the caller's
    /// derivation id ([`current_derivation_id`]). Cut iff a PROPER ANCESTOR of `me`
    /// is already deriving this hash (a genuine cross-thread cycle); `LocalEval` if
    /// `me` is already an owner (a same-thread re-entry the thread-local handles);
    /// otherwise REGISTER `me` and `Claimed` (derive — my descendants cut against me,
    /// bounding the re-derivation fanout).
    ///
    /// The cut fires on the FIRST proper ancestor — NOT after an unroll like the
    /// thread-local thunk table (`Absent→Suspended→Blackhole`, which cuts on the
    /// 3rd occurrence). Cross-thread there is NO shared value memo, so each unroll
    /// RE-DERIVES the entire sub-fanout; admitting even one extra unroll explodes
    /// the work geometrically per recursion level. (Empirically refuted: a
    /// 2nd-ancestor threshold ran a worker to 93 GB virtual / OOM-kill at its 24 GB
    /// cgroup cap, while still dropping the same conclusions — the residual drops
    /// are NOT this cut's timing but the subgoal over-cut, fixed on the subgoal
    /// channel.) Cutting immediately is both correct (the ancestor cycle is genuine
    /// by lineage) and the only bounded choice.
    pub fn lookup_or_cut(&self, channel: MemoChannel, hash: u64, me: u64) -> MemoLookup {
        let e = self.entry(channel, hash);
        let mut owners = e.owners.lock().expect("memo owners lock");
        let mut me_present = false;
        for &o in owners.iter() {
            if o == me {
                me_present = true;
            } else if is_proper_ancestor(o, me) {
                return MemoLookup::Cut;
            }
        }
        if me_present {
            return MemoLookup::LocalEval;
        }
        owners.push(me);
        MemoLookup::Claimed
    }

    /// Release `me`'s registration for a hash it `Claimed` (on completion or
    /// unwind). Idempotent.
    pub fn complete(&self, channel: MemoChannel, hash: u64, me: u64) {
        let e = self.entry(channel, hash);
        let mut owners = e.owners.lock().expect("memo owners lock");
        if let Some(p) = owners.iter().position(|&o| o == me) {
            owners.swap_remove(p);
        }
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
    fn proper_ancestor_chain() {
        // root(scope a) -> child(b) -> grandchild(c).
        let a = DerivationScope::enter_child(0);
        let aid = a.id();
        let b = DerivationScope::enter_child(aid);
        let bid = b.id();
        let c = DerivationScope::enter_child(bid);
        let cid = c.id();

        assert!(!is_proper_ancestor(aid, aid), "NOT reflexive");
        assert!(is_proper_ancestor(aid, bid), "parent");
        assert!(is_proper_ancestor(aid, cid), "grandparent");
        assert!(is_proper_ancestor(bid, cid), "parent");
        assert!(!is_proper_ancestor(cid, aid), "descendant is not an ancestor");
        assert!(!is_proper_ancestor(bid, aid), "sibling/uncle direction");
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
    fn protocol_claim_self_reentry_defers_complete_reclaims() {
        let s = SharedMemoStore::new();
        // First derivation (id 10) claims hash 2.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 2, 10), MemoLookup::Claimed);
        // The SAME id re-enters => defer to the thread-local (LocalEval), NOT cut.
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Thunk, 2, 10),
            MemoLookup::LocalEval
        );
        // After it completes, the hash is owner-free => a later derivation claims fresh.
        s.complete(MemoChannel::Thunk, 2, 10);
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 2, 20), MemoLookup::Claimed);
    }

    #[test]
    fn protocol_cross_thread_cycle_cuts() {
        // A genuine cross-thread cycle cuts on the FIRST proper ancestor (cross-
        // thread re-derivation has no shared memo, so unrolling explodes — a
        // 2nd-ancestor threshold was empirically refuted by a 93 GB OOM runaway).
        let s = SharedMemoStore::new();
        let a = DerivationScope::enter_child(0);
        let aid = a.id();
        let b = DerivationScope::enter_child(aid); // child of A
        let bid = b.id();
        // A claims hash 4.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 4, aid), MemoLookup::Claimed);
        // B re-enters: A is a PROPER ANCESTOR in flight => CUT immediately.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 4, bid), MemoLookup::Cut);
        drop(b);
        drop(a);
    }

    #[test]
    fn protocol_subgoal_channel_cuts_independently() {
        // The Subgoal channel is disjoint and cuts by the same first-ancestor rule.
        let s = SharedMemoStore::new();
        let a = DerivationScope::enter_child(0);
        let aid = a.id();
        let b = DerivationScope::enter_child(aid); // child of A
        let bid = b.id();
        assert_eq!(
            s.lookup_or_cut(MemoChannel::Subgoal, 7, aid),
            MemoLookup::Claimed
        );
        assert_eq!(s.lookup_or_cut(MemoChannel::Subgoal, 7, bid), MemoLookup::Cut);
        drop(b);
        drop(a);
    }

    #[test]
    fn protocol_shared_dependency_registers_and_its_descendant_cuts() {
        // The load-bearing multi-owner case: a non-ancestor sibling re-deriving a
        // shared dependency registers itself, so ITS descendant cuts (no runaway) —
        // against S2 directly, independent of the original owner S1.
        let s = SharedMemoStore::new();
        let s1 = DerivationScope::enter_child(0);
        let s1id = s1.id();
        let s2 = DerivationScope::enter_child(0); // sibling of S1
        let s2id = s2.id();
        let s2child = DerivationScope::enter_child(s2id); // child of S2
        let s2cid = s2child.id();
        // S1 claims hash 5.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 5, s1id), MemoLookup::Claimed);
        // S2 (a non-ancestor sibling) needs hash 5: NOT cut — it REGISTERS and derives.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 5, s2id), MemoLookup::Claimed);
        // S2's child re-enters hash 5: S2 is a PROPER ANCESTOR of it => CUT. This is
        // the cut that, with a single owner (S1 only), would NOT have fired -> runaway.
        assert_eq!(s.lookup_or_cut(MemoChannel::Thunk, 5, s2cid), MemoLookup::Cut);
        drop(s2child);
        drop(s2);
        drop(s1);
    }
}
