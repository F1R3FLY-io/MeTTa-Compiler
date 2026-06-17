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

use crate::backend::models::MettaValue;
use dashmap::DashMap;
use smallvec::SmallVec;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

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

// =========================================================================
// Step 3: the shared content-keyed memo store (DORMANT — built + tested here;
// wired into the eval channels in a later step). See
// docs/design/SHARED_MEMO_STORE_309.md §3-4. Concurrency: lock-free `DashMap`
// for the maps; per-entry `AtomicU8` state + `OnceLock` result; a per-entry
// `Mutex`+`Condvar` only for the rare await; a single-locked wait-for graph for
// deadlock detection. (Loom verification of the primitives is a later sub-step;
// this sub-step is single-threaded-correct + unit-tested.)
// =========================================================================

/// Memo entry lifecycle, stored as an `AtomicU8`.
mod memo_state {
    pub const EMPTY: u8 = 0; // no entry / never claimed
    pub const INFLIGHT: u8 = 1; // some derivation owns it and is computing
    pub const DONE: u8 = 2; // result published (read it)
    pub const ERROR: u8 = 3; // torn-read / panic — awaiters must re-derive
}

/// One shared memo cell for a content hash. The `state` is the source of truth;
/// `results` is published exactly once (via `OnceLock`) on the EMPTY→…→DONE edge.
pub struct SharedMemoEntry {
    state: AtomicU8,
    /// Derivation-id of the INFLIGHT owner (`0` = none). Read by the cut decision
    /// (`owner == me` ⇒ same-derivation cycle; ancestor ⇒ cross-thread cycle).
    owner: AtomicU64,
    results: OnceLock<SmallVec<[MettaValue; 2]>>,
    /// `space_mutation_epoch` captured when INFLIGHT was claimed — a DONE entry
    /// stale w.r.t. a sibling `add-atom` is rejected by the reader (Step 4).
    space_epoch: AtomicU64,
    /// Park/notify substrate for awaiters; the predicate is `state ∈ {DONE,ERROR}`
    /// and `_wait_lock` only fences the wait/notify against a lost wakeup.
    wait_lock: Mutex<()>,
    cv: Condvar,
}

impl SharedMemoEntry {
    fn new() -> Self {
        SharedMemoEntry {
            state: AtomicU8::new(memo_state::EMPTY),
            owner: AtomicU64::new(0),
            results: OnceLock::new(),
            space_epoch: AtomicU64::new(0),
            wait_lock: Mutex::new(()),
            cv: Condvar::new(),
        }
    }

    #[inline]
    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }
    #[inline]
    pub fn owner(&self) -> u64 {
        self.owner.load(Ordering::Acquire)
    }
    #[inline]
    pub fn space_epoch(&self) -> u64 {
        self.space_epoch.load(Ordering::Acquire)
    }

    /// CAS EMPTY→INFLIGHT, recording the owner + space epoch. Returns `true` iff
    /// THIS caller won the claim (and must therefore derive + publish).
    pub fn try_claim(&self, my_id: u64, space_epoch: u64) -> bool {
        if self
            .state
            .compare_exchange(
                memo_state::EMPTY,
                memo_state::INFLIGHT,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.owner.store(my_id, Ordering::Release);
            self.space_epoch.store(space_epoch, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Publish the result and transition INFLIGHT→DONE, waking awaiters. The
    /// `state` store (Release) happens before taking `wait_lock`, so a waiter that
    /// is between its state-check and `cv.wait` is fenced by the lock and cannot
    /// miss the wakeup.
    pub fn publish_done(&self, results: SmallVec<[MettaValue; 2]>) {
        let _ = self.results.set(results);
        self.state.store(memo_state::DONE, Ordering::Release);
        let _g = self.wait_lock.lock().expect("memo entry wait_lock");
        self.cv.notify_all();
    }

    /// Mark ERROR (torn read / owner panic) and wake awaiters to re-derive.
    pub fn publish_error(&self) {
        self.state.store(memo_state::ERROR, Ordering::Release);
        let _g = self.wait_lock.lock().expect("memo entry wait_lock");
        self.cv.notify_all();
    }

    /// Read the published result iff DONE.
    pub fn read_done(&self) -> Option<SmallVec<[MettaValue; 2]>> {
        if self.state() == memo_state::DONE {
            self.results.get().cloned()
        } else {
            None
        }
    }

    /// Park until the entry reaches DONE or ERROR; returns the terminal state.
    /// Holds `wait_lock` across the state-check+wait so a concurrent publish
    /// cannot slip a notify in between (no lost wakeup).
    pub fn await_terminal(&self) -> u8 {
        let mut g = self.wait_lock.lock().expect("memo entry wait_lock");
        loop {
            let s = self.state();
            if s == memo_state::DONE || s == memo_state::ERROR {
                return s;
            }
            g = self.cv.wait(g).expect("memo entry cv wait");
        }
    }
}

/// Wait-for graph for cross-thread deadlock detection. Node = derivation id;
/// edge `a → b` = "a is parked awaiting b's in-flight entry". The graph is kept
/// ACYCLIC by construction: [`try_add_edge`](WaitForGraph::try_add_edge) inserts
/// `a → b` only when it would NOT close a cycle, so no set of parked awaiters can
/// deadlock.
pub struct WaitForGraph {
    adj: Mutex<HashMap<u64, SmallVec<[u64; 4]>>>,
}

impl WaitForGraph {
    fn new() -> Self {
        WaitForGraph {
            adj: Mutex::new(HashMap::new()),
        }
    }

    /// Atomically test-and-insert: add `from → to` iff `from` is NOT already
    /// reachable from `to` (which would make `from → to` close a cycle). Returns
    /// `true` iff the edge was added — i.e. it is safe for `from` to PARK awaiting
    /// `to`. `false` means awaiting would deadlock; the caller must cut/local-eval.
    pub fn try_add_edge(&self, from: u64, to: u64) -> bool {
        let mut adj = self.adj.lock().expect("waitfor graph lock");
        if Self::reachable(&adj, to, from) {
            return false;
        }
        adj.entry(from).or_default().push(to);
        true
    }

    /// Remove `from → to` (called when the awaiter wakes).
    pub fn remove_edge(&self, from: u64, to: u64) {
        let mut adj = self.adj.lock().expect("waitfor graph lock");
        if let Some(v) = adj.get_mut(&from) {
            if let Some(p) = v.iter().position(|&x| x == to) {
                v.swap_remove(p);
            }
            if v.is_empty() {
                adj.remove(&from);
            }
        }
    }

    /// Is `target` reachable from `start` via await edges? (DFS over the
    /// adjacency held under the caller's lock.)
    fn reachable(adj: &HashMap<u64, SmallVec<[u64; 4]>>, start: u64, target: u64) -> bool {
        if start == target {
            return true;
        }
        let mut stack: SmallVec<[u64; 16]> = SmallVec::new();
        stack.push(start);
        let mut seen: HashSet<u64> = HashSet::new();
        while let Some(n) = stack.pop() {
            if !seen.insert(n) {
                continue;
            }
            if let Some(neighbors) = adj.get(&n) {
                for &m in neighbors {
                    if m == target {
                        return true;
                    }
                    stack.push(m);
                }
            }
        }
        false
    }
}

/// Which content-hash namespace an operation targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoChannel {
    Thunk,
    Subgoal,
}

/// Outcome of [`SharedMemoStore::lookup_or_claim`] — the Step-4 protocol decision
/// (matches `tla/SharedMemoAwaitNoDeadlock.tla`). The caller acts on it:
/// `Read`/`AwaitedRead` → use the bag; `Claimed` → derive then `publish_*`;
/// `Cut` → contribute the fixpoint EMPTY; `LocalEval` → derive on this thread
/// WITHOUT touching the store (the always-safe, never-blocking, never-over-cut
/// fallback for a would-deadlock shared dependency, a stale/errored entry).
pub enum MemoLookup {
    Read(SmallVec<[MettaValue; 2]>),
    AwaitedRead(SmallVec<[MettaValue; 2]>),
    Claimed,
    Cut,
    LocalEval,
}

/// The process-wide shared memo store: cross-thread memoization for the thunk and
/// subgoal channels plus the wait-for graph that keeps awaits deadlock-free.
pub struct SharedMemoStore {
    thunks: DashMap<u64, Arc<SharedMemoEntry>>,
    subgoals: DashMap<u64, Arc<SharedMemoEntry>>,
    waitfor: WaitForGraph,
}

impl SharedMemoStore {
    fn new() -> Self {
        SharedMemoStore {
            thunks: DashMap::new(),
            subgoals: DashMap::new(),
            waitfor: WaitForGraph::new(),
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

    /// The wait-for graph (deadlock detection).
    #[inline]
    pub fn waitfor(&self) -> &WaitForGraph {
        &self.waitfor
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

    /// The Step-4 cross-thread memo protocol (formally verified deadlock-free +
    /// over-cut-free by `tla/SharedMemoAwaitNoDeadlock.tla`). `my_id` is the
    /// caller's derivation id ([`current_derivation_id`]); `space_epoch` the
    /// current `space_mutation_epoch`. See [`MemoLookup`] for how to act on each
    /// outcome.
    pub fn lookup_or_claim(
        &self,
        channel: MemoChannel,
        hash: u64,
        my_id: u64,
        space_epoch: u64,
    ) -> MemoLookup {
        let e = self.entry(channel, hash);
        loop {
            match e.state() {
                memo_state::DONE => {
                    if e.space_epoch() == space_epoch {
                        if let Some(r) = e.read_done() {
                            return MemoLookup::Read(r);
                        }
                    }
                    // Stale w.r.t. a sibling add-atom (or transiently unreadable):
                    // re-derive locally rather than trust a stale bag.
                    return MemoLookup::LocalEval;
                }
                memo_state::ERROR => return MemoLookup::LocalEval,
                memo_state::EMPTY => {
                    if e.try_claim(my_id, space_epoch) {
                        return MemoLookup::Claimed;
                    }
                    // Lost the claim race — re-observe (now INFLIGHT/DONE/ERROR).
                    continue;
                }
                memo_state::INFLIGHT => {
                    let owner = e.owner();
                    if owner == my_id {
                        // Same-derivation re-entry — the genuine self-cycle the
                        // thread-local Blackhole used to catch.
                        return MemoLookup::Cut;
                    }
                    if self.waitfor.try_add_edge(my_id, owner) {
                        // Awaiting cannot deadlock (the edge did not close a cycle):
                        // park until the owner finishes, then read.
                        let terminal = e.await_terminal();
                        self.waitfor.remove_edge(my_id, owner);
                        if terminal == memo_state::DONE && e.space_epoch() == space_epoch {
                            if let Some(r) = e.read_done() {
                                return MemoLookup::AwaitedRead(r);
                            }
                        }
                        // Owner errored or the result went stale — derive locally.
                        return MemoLookup::LocalEval;
                    }
                    // Awaiting WOULD deadlock. Cut iff the owner is a genuine
                    // ancestor (a real cross-thread fixpoint cycle); otherwise it
                    // is a shared in-flight dependency the blocked parent owns —
                    // never cut it, evaluate it locally.
                    if is_ancestor(owner, my_id) {
                        return MemoLookup::Cut;
                    }
                    return MemoLookup::LocalEval;
                }
                _ => return MemoLookup::LocalEval, // unreachable state byte
            }
        }
    }

    /// Publish a derived result for a hash this caller `Claimed` (INFLIGHT→DONE,
    /// waking awaiters).
    pub fn publish_done(
        &self,
        channel: MemoChannel,
        hash: u64,
        results: SmallVec<[MettaValue; 2]>,
    ) {
        self.entry(channel, hash).publish_done(results);
    }

    /// Mark a claimed hash ERROR (torn read / owner unwound), waking awaiters to
    /// re-derive locally.
    pub fn publish_error(&self, channel: MemoChannel, hash: u64) {
        self.entry(channel, hash).publish_error();
    }

    // GC-safety — UNRESOLVED design decision (store is dormant, so nothing is
    // broken yet). VERIFIED that the index-GC is MARK-FROM-ROOTS-ONLY:
    // `project_roots_to_addrs` (index_heap.rs:2092) marks ONLY the supplied root
    // slice, and the sweep's hash-cons `retain` (index_heap.rs:1276) FOLLOWS the
    // mark (it does not pin). A `MettaValue` is a bare `Copy` `Addr` handle with no
    // refcount, so a bag held ONLY in this store would NOT be retained — unmarked
    // => swept => silent ABA (the node slot has no generation). Therefore, BEFORE
    // this store may hold bags on the live eval path, GC-safety MUST be resolved:
    // either (a) make it a named anchor in `collect_global_anchors` (like
    // `collect_thunk_roots`) + join the post-sweep ABA clear, or (b) redesign it to
    // hold NO bags past machine-reachability (coordination-only: track in-flight
    // owners for the lineage cut, store no values). See
    // docs/post-mortems/PARALLEL_FANOUT_TABLING_309_266.md §8.
}

/// The process-wide store handle (lazily initialized). Entries are content-keyed
/// and space-epoch-stamped, so a stale entry is rejected at read time; `clear` is
/// called at `!` boundaries.
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
    fn entry_claim_publish_read() {
        let e = SharedMemoEntry::new();
        assert_eq!(e.state(), memo_state::EMPTY);
        assert!(e.try_claim(7, 0), "first claim wins");
        assert_eq!(e.state(), memo_state::INFLIGHT);
        assert_eq!(e.owner(), 7);
        assert!(!e.try_claim(9, 0), "second claim loses (already INFLIGHT)");
        assert!(e.read_done().is_none(), "not done yet");
        e.publish_done(SmallVec::new());
        assert_eq!(e.state(), memo_state::DONE);
        assert_eq!(e.read_done().map(|v| v.len()), Some(0));
    }

    #[test]
    fn waitfor_rejects_cycle_admits_dag() {
        let g = WaitForGraph::new();
        assert!(g.try_add_edge(1, 2), "1->2 ok");
        assert!(g.try_add_edge(2, 3), "2->3 ok (chain)");
        assert!(!g.try_add_edge(3, 1), "3->1 would close 1->2->3->1 — rejected");
        assert!(g.try_add_edge(1, 4), "1->4 ok (no cycle)");
        g.remove_edge(2, 3);
        assert!(g.try_add_edge(3, 1), "3->1 ok after 2->3 removed");
    }

    #[test]
    fn waitfor_self_edge_is_a_cycle() {
        let g = WaitForGraph::new();
        assert!(!g.try_add_edge(5, 5), "self-await is a cycle");
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
    fn await_terminal_wakes_on_publish() {
        let e = Arc::new(SharedMemoEntry::new());
        assert!(e.try_claim(1, 0));
        let e2 = Arc::clone(&e);
        let h = std::thread::spawn(move || e2.await_terminal());
        std::thread::yield_now();
        e.publish_done(SmallVec::new());
        assert_eq!(h.join().expect("await thread"), memo_state::DONE);
    }

    #[test]
    fn protocol_claim_then_read() {
        let s = SharedMemoStore::new();
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 1, 10, 0),
            MemoLookup::Claimed
        ));
        s.publish_done(MemoChannel::Thunk, 1, SmallVec::new());
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 1, 20, 0),
            MemoLookup::Read(_)
        ));
    }

    #[test]
    fn protocol_same_derivation_cycle_cuts() {
        let s = SharedMemoStore::new();
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 2, 10, 0),
            MemoLookup::Claimed
        ));
        // The SAME derivation re-enters its own in-flight entry => cut.
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 2, 10, 0),
            MemoLookup::Cut
        ));
    }

    #[test]
    fn protocol_stale_done_local_evals() {
        let s = SharedMemoStore::new();
        // Claimed + published at space-epoch 5.
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 3, 10, 5),
            MemoLookup::Claimed
        ));
        s.publish_done(MemoChannel::Thunk, 3, SmallVec::new());
        // A caller at a NEWER epoch sees the stale DONE => local-eval (re-derive).
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 3, 20, 7),
            MemoLookup::LocalEval
        ));
    }

    #[test]
    fn protocol_cross_thread_cycle_cuts() {
        let s = SharedMemoStore::new();
        let a = DerivationScope::enter_child(0);
        let aid = a.id();
        let b = DerivationScope::enter_child(aid); // B is a child of A
        let bid = b.id();
        // A claims hash 4 (in-flight, owner = A).
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 4, aid, 0),
            MemoLookup::Claimed
        ));
        // A is parked awaiting B (the collapse-merge block): edge A->B.
        assert!(s.waitfor().try_add_edge(aid, bid));
        // B re-enters A's in-flight thunk. Awaiting B->A would close A<->B; A is a
        // genuine ancestor of B => CUT (a real cross-thread fixpoint cycle).
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 4, bid, 0),
            MemoLookup::Cut
        ));
        drop(b);
        drop(a);
    }

    #[test]
    fn protocol_shared_dependency_awaits_then_reads() {
        let s = Arc::new(SharedMemoStore::new());
        // Owner derivation 100 claims hash 6.
        assert!(matches!(
            s.lookup_or_claim(MemoChannel::Thunk, 6, 100, 0),
            MemoLookup::Claimed
        ));
        // A different, NON-ancestor derivation (200) needs the same hash. With no
        // wait-for cycle it must AWAIT (a shared dependency), never cut.
        let s2 = Arc::clone(&s);
        let h = std::thread::spawn(move || s2.lookup_or_claim(MemoChannel::Thunk, 6, 200, 0));
        std::thread::yield_now();
        s.publish_done(MemoChannel::Thunk, 6, SmallVec::new());
        // The shared dependency must be SERVED, never cut — either it parked then
        // read (AwaitedRead) or the publish landed first and it read immediately
        // (Read), depending on the race; both are correct, neither is Cut.
        assert!(matches!(
            h.join().expect("await thread"),
            MemoLookup::AwaitedRead(_) | MemoLookup::Read(_)
        ));
    }

}
