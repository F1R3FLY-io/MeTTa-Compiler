//! Index-addressed, segmented value arena — Increment 2 of the clean-room GC
//! migration (plan: `would-it-be-possible-resilient-gosling.md`).
//!
//! This is the *substrate* the non-moving collector reclaims: a growable set of
//! fixed-capacity **segments**, each owning a contiguous slot array plus an
//! atomic **mark bitmap**. A value is named by an [`Addr`] — a packed
//! `(segment_index, offset)` index, **not** a machine pointer — which is the
//! property that eliminates the `&'static` raw-pointer value model (#3) and
//! pointer-identity ABA (#4): segments are stable/boxed so `&node` references
//! never move, and reclamation is non-moving.
//!
//! ## Why segments
//! - **Stable addresses.** Each segment preallocates its slot storage to its
//!   capacity, so a slot reference is never invalidated by reallocation — that
//!   is what makes a *monotonic, append-only* segment table sound to read
//!   lock-free (see the plan's Finding 3).
//! - **Whole-segment release.** A segment with zero live slots after a sweep is
//!   released wholesale (its storage dropped), returning memory at segment
//!   granularity — the analogue of the slab's `munmap` (production will back
//!   segments with `mmap`/`madvise`; this module uses `Vec` storage and is the
//!   algorithmic core those back).
//! - **Slot reuse without ABA.** Partially-live segments contribute their
//!   unmarked slots to a free list **rebuilt from scratch by the single-threaded
//!   quiescent sweep** and consumed only during the next mutation phase — there
//!   is no concurrent free-vs-CAS, so reuse is ABA-free by construction.
//!
//! ## Status
//! Increment 2 is in progress: this module is complete and unit-tested in
//! isolation, but is not yet wired into [`crate::backend::models::MettaValue`]'s
//! decode or the `Store` trait (those are the subsequent steps of Inc 2). It is
//! therefore `#![allow(dead_code)]` for now, mirroring the inert-scaffolding
//! convention used by `trampoline/binding_store.rs`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

/// Number of low bits of an [`Addr`] used for the intra-segment slot offset.
/// 18 bits ⇒ up to 262_144 slots per segment (~6 MiB at 24 B/node, within the
/// plan's ≤8 MiB per-CCD-L3-resident segment target).
pub const OFFSET_BITS: u32 = 18;
/// Number of high bits used for the segment index (32 − [`OFFSET_BITS`] = 14 ⇒
/// up to 16_384 segments; 4.3 billion total slots in a 32-bit `Addr`). The
/// plan reserves ≥42 bits in the eventual `MettaValue` NaN-box payload, so this
/// widens without an ABI change when the value model is wired.
pub const SEGMENT_BITS: u32 = 32 - OFFSET_BITS;
/// Default per-segment slot capacity (the maximum addressable offset).
pub const DEFAULT_SEGMENT_CAPACITY: usize = 1 << OFFSET_BITS;
/// Maximum number of segments addressable by a 32-bit `Addr`.
pub const MAX_SEGMENTS: usize = 1 << SEGMENT_BITS;

const OFFSET_MASK: u32 = (1 << OFFSET_BITS) - 1;

/// A packed arena address: `(segment_index << OFFSET_BITS) | offset`.
///
/// This is an *index*, not a pointer. `Addr(u32::MAX)` is never produced by
/// allocation (segment `2^14−1`, offset `2^18−1` would require a full arena),
/// but callers should not rely on a niche; use [`Addr::is_null`] sentinels via
/// `Option<Addr>` where a "no address" is needed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Addr(u32);

impl Addr {
    /// Pack a `(segment, offset)` pair. Panics in debug if either field
    /// overflows its allotted bit width.
    #[inline]
    pub fn new(segment: u32, offset: u32) -> Self {
        debug_assert!(
            (segment as usize) < MAX_SEGMENTS,
            "segment index {segment} exceeds {MAX_SEGMENTS}"
        );
        debug_assert!(
            offset <= OFFSET_MASK,
            "offset {offset} exceeds {OFFSET_MASK}"
        );
        Addr((segment << OFFSET_BITS) | offset)
    }

    /// The raw packed 32-bit index (what the `MettaValue` payload will carry).
    #[inline]
    pub fn raw(self) -> u32 {
        self.0
    }

    /// Reconstruct from a raw packed index.
    #[inline]
    pub fn from_raw(raw: u32) -> Self {
        Addr(raw)
    }

    /// The segment index this address lives in.
    #[inline]
    pub fn segment(self) -> usize {
        (self.0 >> OFFSET_BITS) as usize
    }

    /// The intra-segment slot offset.
    #[inline]
    pub fn offset(self) -> usize {
        (self.0 & OFFSET_MASK) as usize
    }
}

/// A node whose outgoing edges (child handles) the collector can enumerate.
///
/// Implemented by the concrete value-node type stored in the arena. The
/// transitive [`IndexArena::mark_from_roots`] uses it to reach every live node
/// from the root set; leaves (atoms, numbers, interned ids) push nothing. When
/// the value model is wired (a later step of Inc 2), the arena's `Node` —
/// mirroring `MettaValueInner` with children as [`Addr`]s — implements this.
pub trait ArenaNode: Copy {
    /// Append this node's child addresses to `out` (order irrelevant).
    fn child_addrs(&self, out: &mut Vec<Addr>);
}

/// One segment: a fixed-capacity slot array plus its mark bitmap.
///
/// `nodes` is preallocated to `capacity` so the storage never reallocates while
/// the segment is alive (stable addresses). A released segment drops `nodes`
/// (freeing the memory) and is flagged so the arena can detect a stale access
/// in debug builds.
struct Segment<N: Copy> {
    /// Slot storage; `len` slots are in use (`0..len` is the bump high-water).
    nodes: Vec<N>,
    /// Number of slots ever bump-allocated in this segment (the high-water).
    len: usize,
    /// Per-slot mark bits, `ceil(capacity/64)` words. `AtomicU64` so a future
    /// parallel/concurrent mark can set bits without a lock.
    marks: Vec<AtomicU64>,
    /// Per-segment slot capacity.
    capacity: usize,
    /// `true` once the segment has been released (its `nodes` dropped).
    released: bool,
}

impl<N: Copy> Segment<N> {
    fn new(capacity: usize) -> Self {
        debug_assert!(capacity > 0 && capacity <= DEFAULT_SEGMENT_CAPACITY);
        let words = capacity.div_ceil(64);
        let mut marks = Vec::with_capacity(words);
        for _ in 0..words {
            marks.push(AtomicU64::new(0));
        }
        Segment {
            nodes: Vec::with_capacity(capacity),
            len: 0,
            marks,
            capacity,
            released: false,
        }
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.len >= self.capacity
    }

    /// Set the mark bit for `offset`. Returns `true` if it was previously
    /// unmarked (i.e. this call performed the marking), enabling the caller to
    /// push newly-grayed nodes onto a worklist exactly once.
    #[inline]
    fn set_mark(&self, offset: usize) -> bool {
        let bit = 1u64 << (offset & 63);
        let prev = self.marks[offset >> 6].fetch_or(bit, Ordering::Relaxed);
        (prev & bit) == 0
    }

    #[inline]
    fn is_marked(&self, offset: usize) -> bool {
        let bit = 1u64 << (offset & 63);
        (self.marks[offset >> 6].load(Ordering::Relaxed) & bit) != 0
    }

    #[inline]
    fn clear_marks(&self) {
        for w in &self.marks {
            w.store(0, Ordering::Relaxed);
        }
    }

    /// `true` if no slot in `0..len` is marked.
    ///
    /// B1.b word-parallel fast path: OR together the complete mark words covering
    /// `0..len` (one `AtomicU64::load` per 64 slots instead of per slot), masking
    /// the final partial word to the valid low `len & 63` bits. Equivalent to the
    /// per-slot scan — a segment is fully dead iff no in-range mark bit is set
    /// (offsets `>= len` are never set by `set_mark`, so the mask only guards the
    /// padding bits of the last word).
    fn is_fully_dead(&self) -> bool {
        let len = self.len;
        let full_words = len >> 6;
        for wi in 0..full_words {
            if self.marks[wi].load(Ordering::Relaxed) != 0 {
                return false;
            }
        }
        let rem = len & 63;
        if rem != 0 {
            let mask = (1u64 << rem) - 1;
            if (self.marks[full_words].load(Ordering::Relaxed) & mask) != 0 {
                return false;
            }
        }
        true
    }

    /// Drop the slot storage, returning the byte estimate freed.
    fn release(&mut self) -> usize {
        let freed = self.nodes.capacity() * std::mem::size_of::<N>();
        self.nodes = Vec::new();
        self.len = 0;
        self.released = true;
        freed
    }
}

/// Outcome of a [`IndexArena::sweep`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepStats {
    /// Live slots retained (marked).
    pub live: usize,
    /// Slots reclaimed onto the free list (partially-live segments).
    pub reclaimed_to_free_list: usize,
    /// Whole segments released (fully dead).
    pub segments_released: usize,
    /// Estimated bytes returned by segment release.
    pub bytes_released: usize,
}

/// A segmented, index-addressed, non-moving value arena.
///
/// Allocation pops a reusable slot from the free list (rebuilt by the previous
/// sweep) or bump-allocates in the current segment, opening a new segment when
/// it fills. Reclamation is the mark/sweep cycle: callers [`mark`](Self::mark)
/// the live set, then [`sweep`](Self::sweep) rebuilds the free list from
/// unmarked slots and releases fully-dead segments.
pub struct IndexArena<N: Copy> {
    segments: Vec<Segment<N>>,
    /// Current bump-target segment.
    cur_seg: usize,
    /// Per-segment slot capacity for new segments.
    segment_capacity: usize,
    /// Free slots reclaimed by the last sweep, consumed by allocation.
    /// Rebuilt-from-scratch each sweep (never persisted across cycles), so a
    /// slot in a released segment is never handed out.
    free_list: Vec<Addr>,
    /// Total live + free slots ever bump-allocated (diagnostics).
    alloc_count: u64,
}

impl<N: Copy> Default for IndexArena<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N: Copy> IndexArena<N> {
    /// A new arena with the default (production) segment capacity.
    pub fn new() -> Self {
        Self::with_segment_capacity(DEFAULT_SEGMENT_CAPACITY)
    }

    /// A new arena whose segments hold `capacity` slots each. Small capacities
    /// are used by tests to exercise multi-segment behavior cheaply.
    pub fn with_segment_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0 && capacity <= DEFAULT_SEGMENT_CAPACITY,
            "segment capacity {capacity} out of range 1..={DEFAULT_SEGMENT_CAPACITY}"
        );
        let mut arena = IndexArena {
            segments: Vec::new(),
            cur_seg: 0,
            segment_capacity: capacity,
            free_list: Vec::new(),
            alloc_count: 0,
        };
        arena.open_segment();
        arena
    }

    /// Open a fresh segment and make it the bump target. Returns its index.
    fn open_segment(&mut self) -> usize {
        assert!(
            self.segments.len() < MAX_SEGMENTS,
            "arena exhausted: {MAX_SEGMENTS} segments"
        );
        let idx = self.segments.len();
        self.segments.push(Segment::new(self.segment_capacity));
        self.cur_seg = idx;
        idx
    }

    /// Allocate `node`, returning its address. Prefers a reused free slot, else
    /// bump-allocates in the current segment (opening a new one if full).
    pub fn alloc(&mut self, node: N) -> Addr {
        self.alloc_count += 1;
        if let Some(addr) = self.free_list.pop() {
            let seg = &mut self.segments[addr.segment()];
            debug_assert!(!seg.released);
            seg.nodes[addr.offset()] = node;
            return addr;
        }
        // Bump in the current segment, advancing past full/released segments.
        loop {
            let seg = &mut self.segments[self.cur_seg];
            if !seg.released && !seg.is_full() {
                let off = seg.len;
                debug_assert_eq!(seg.nodes.len(), off);
                seg.nodes.push(node);
                seg.len += 1;
                return Addr::new(self.cur_seg as u32, off as u32);
            }
            self.open_segment();
        }
    }

    /// Borrow the node at `addr`.
    #[inline]
    pub fn get(&self, addr: Addr) -> &N {
        let seg = &self.segments[addr.segment()];
        debug_assert!(
            !seg.released,
            "get on a released segment {}",
            addr.segment()
        );
        &seg.nodes[addr.offset()]
    }

    /// Mutably borrow the node at `addr`.
    #[inline]
    pub fn get_mut(&mut self, addr: Addr) -> &mut N {
        let seg = &mut self.segments[addr.segment()];
        debug_assert!(!seg.released);
        &mut seg.nodes[addr.offset()]
    }

    /// Mark `addr` live. Returns `true` if it was newly marked (was unmarked).
    #[inline]
    pub fn mark(&self, addr: Addr) -> bool {
        self.segments[addr.segment()].set_mark(addr.offset())
    }

    /// Whether `addr` is currently marked.
    #[inline]
    pub fn is_marked(&self, addr: Addr) -> bool {
        self.segments[addr.segment()].is_marked(addr.offset())
    }

    /// Number of segments currently allocated (including released ones, which
    /// retain their index slot so addresses stay stable).
    #[inline]
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Total allocations performed (diagnostics).
    #[inline]
    pub fn alloc_count(&self) -> u64 {
        self.alloc_count
    }

    /// Number of slots currently on the free list.
    #[inline]
    pub fn free_slots(&self) -> usize {
        self.free_list.len()
    }

    /// Bytes of node-slot storage currently committed (Σ over non-released
    /// segments of `nodes.capacity() * size_of::<N>()`). This is the
    /// node-slab footprint actually backed by allocated memory; it is monotone
    /// between sweeps (segments only grow) and drops when a fully-dead segment
    /// is released. Side-arena (children/strings/spans) bytes are NOT counted
    /// here — the wrapping `IndexHeap` adds those — but the node footprint is a
    /// faithful, cheap watermark signal for the index GC trigger (Inc 6).
    #[inline]
    pub fn committed_node_bytes(&self) -> usize {
        let per = std::mem::size_of::<N>();
        let mut total = 0usize;
        for seg in &self.segments {
            if !seg.released {
                total += seg.nodes.capacity() * per;
            }
        }
        total
    }

    /// Number of node slots ever bump-allocated across all non-released segments
    /// (the sum of per-segment high-water marks). A cheap proxy for "occupied
    /// slots"; it does not subtract free-list slots (those still occupy
    /// committed storage). Used to recompute the GC watermark after a sweep.
    #[inline]
    pub fn live_node_count(&self) -> usize {
        let mut total = 0usize;
        for seg in &self.segments {
            if !seg.released {
                total += seg.len;
            }
        }
        total
    }

    /// Per-node byte size (`size_of::<N>()`), so a wrapper can convert a node
    /// count into a byte estimate without knowing `N`.
    #[inline]
    pub fn node_size_bytes(&self) -> usize {
        std::mem::size_of::<N>()
    }

    /// Reclaim unreachable slots: rebuild the free list from unmarked slots of
    /// partially-live segments, release fully-dead segments, then clear all
    /// mark bits for the next cycle. Returns sweep statistics.
    ///
    /// Must be called at quiescence (no concurrent allocation/mutation) — this
    /// is what keeps free-list reuse ABA-free.
    pub fn sweep(&mut self) -> SweepStats {
        self.sweep_with(|_| {})
    }

    /// Like [`sweep`](Self::sweep) but invokes `on_release(segment_index)` for
    /// each fully-dead segment as it is released, so a wrapper that owns parallel
    /// per-segment side-arenas (e.g. `IndexHeap`) can co-release them in lockstep.
    pub fn sweep_with<F: FnMut(usize)>(&mut self, mut on_release: F) -> SweepStats {
        let mut stats = SweepStats::default();
        // Rebuild the free list from scratch (never persist across cycles).
        self.free_list.clear();

        let seg_count = self.segments.len();
        for si in 0..seg_count {
            if self.segments[si].released {
                continue;
            }
            let is_current = si == self.cur_seg;
            let fully_dead = self.segments[si].is_fully_dead();

            if fully_dead && !is_current {
                // Whole-segment release (never the current bump target, so the
                // arena always has a live segment to allocate into).
                on_release(si);
                stats.bytes_released += self.segments[si].release();
                stats.segments_released += 1;
                continue;
            }

            // Partially-live (or the current segment): reclaim unmarked slots.
            // B1.b word-parallel fast path: per complete mark word, skip the
            // per-bit loop when the word is all-live (`u64::MAX` -> 64 live) or
            // all-dead (`0` -> 64 contiguous free slots); only mixed words and the
            // final partial word fall back to the per-bit test. Push order stays
            // increasing-`off`, so the free list (and its LIFO reuse order) is
            // byte-identical to the per-slot scan.
            let len = self.segments[si].len;
            let full_words = len >> 6;
            let rem = len & 63;
            for wi in 0..full_words {
                let word = self.segments[si].marks[wi].load(Ordering::Relaxed);
                let base = wi << 6;
                if word == u64::MAX {
                    stats.live += 64;
                } else if word == 0 {
                    for off in base..base + 64 {
                        self.free_list.push(Addr::new(si as u32, off as u32));
                    }
                    stats.reclaimed_to_free_list += 64;
                } else {
                    for b in 0..64usize {
                        if (word & (1u64 << b)) != 0 {
                            stats.live += 1;
                        } else {
                            self.free_list.push(Addr::new(si as u32, (base + b) as u32));
                            stats.reclaimed_to_free_list += 1;
                        }
                    }
                }
            }
            if rem != 0 {
                let word = self.segments[si].marks[full_words].load(Ordering::Relaxed);
                let base = full_words << 6;
                for b in 0..rem {
                    if (word & (1u64 << b)) != 0 {
                        stats.live += 1;
                    } else {
                        self.free_list.push(Addr::new(si as u32, (base + b) as u32));
                        stats.reclaimed_to_free_list += 1;
                    }
                }
            }
            self.segments[si].clear_marks();
        }

        // If the current segment was released-eligible but kept, or all
        // non-current segments died, ensure cur_seg points at a usable segment.
        if self.segments[self.cur_seg].released {
            self.open_segment();
        }
        stats
    }

    /// Transitively mark from `roots`, sourcing each node's child addresses from
    /// `child_fn` rather than [`ArenaNode::child_addrs`]. This lets a wrapper that
    /// owns side-arenas (e.g. `IndexHeap`) supply children for variable-length
    /// nodes (SExpr/Conjunction) whose children live outside the node. Stack-safe
    /// explicit worklist; idempotent marking dedups shared substructure/cycles.
    pub fn mark_from_roots_with<F: FnMut(Addr, &mut Vec<Addr>)>(
        &self,
        roots: &[Addr],
        mut child_fn: F,
    ) -> usize {
        let mut marked = 0usize;
        let mut worklist: Vec<Addr> = Vec::with_capacity(roots.len().max(16));
        for &r in roots {
            if self.mark(r) {
                marked += 1;
                worklist.push(r);
            }
        }
        let mut kids: Vec<Addr> = Vec::new();
        while let Some(addr) = worklist.pop() {
            kids.clear();
            child_fn(addr, &mut kids);
            for &k in &kids {
                if self.mark(k) {
                    marked += 1;
                    worklist.push(k);
                }
            }
        }
        marked
    }

    /// Ensure the current segment can bump-allocate a node slot, opening a fresh
    /// segment if the current one is full or released. Returns the segment index
    /// that will receive the next [`bump_in`]. Variable-length allocation calls
    /// this first so a node and its side-arena data co-locate in one segment.
    pub fn ensure_bump_room(&mut self) -> usize {
        loop {
            let seg = &self.segments[self.cur_seg];
            if !seg.released && !seg.is_full() {
                return self.cur_seg;
            }
            self.open_segment();
        }
    }

    /// Bump-allocate `node` into `seg` (which must be the current, non-full,
    /// non-released segment, as just returned by [`ensure_bump_room`]). Bypasses
    /// the free list so the caller controls co-location with side-arena data.
    pub fn bump_in(&mut self, seg: usize, node: N) -> Addr {
        assert_eq!(
            seg, self.cur_seg,
            "bump_in target must be the current segment"
        );
        let s = &mut self.segments[seg];
        debug_assert!(!s.released && !s.is_full());
        let off = s.len;
        debug_assert_eq!(s.nodes.len(), off);
        s.nodes.push(node);
        s.len += 1;
        self.alloc_count += 1;
        Addr::new(seg as u32, off as u32)
    }
}

impl<N: ArenaNode> IndexArena<N> {
    /// Transitively mark every node reachable from `roots`, returning the count
    /// of nodes *newly* marked this call.
    ///
    /// The traversal is an explicit worklist — **no Rust recursion** — so it is
    /// stack-safe regardless of graph depth, satisfying the project's
    /// stack-safety mandate for the collector. Marking is idempotent (the mark
    /// bitmap dedups), so shared substructure is visited once and cycles (should
    /// the graph ever contain a mutable `State` back-edge) terminate.
    ///
    /// This is the full-heap transitive mark the non-moving collector runs at a
    /// quiescent safepoint; pair it with [`sweep`](Self::sweep) to reclaim.
    pub fn mark_from_roots(&self, roots: &[Addr]) -> usize {
        let mut marked = 0usize;
        let mut worklist: Vec<Addr> = Vec::with_capacity(roots.len().max(16));
        for &r in roots {
            if self.mark(r) {
                marked += 1;
                worklist.push(r);
            }
        }
        let mut kids: Vec<Addr> = Vec::new();
        while let Some(addr) = worklist.pop() {
            kids.clear();
            self.get(addr).child_addrs(&mut kids);
            for &k in &kids {
                if self.mark(k) {
                    marked += 1;
                    worklist.push(k);
                }
            }
        }
        marked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_pack_unpack_roundtrip() {
        for &(seg, off) in &[(0u32, 0u32), (1, 5), (3, 17), (16383, 262143), (42, 100)] {
            let a = Addr::new(seg, off);
            assert_eq!(a.segment(), seg as usize, "segment for ({seg},{off})");
            assert_eq!(a.offset(), off as usize, "offset for ({seg},{off})");
            assert_eq!(
                Addr::from_raw(a.raw()),
                a,
                "raw roundtrip for ({seg},{off})"
            );
        }
    }

    #[test]
    fn alloc_get_within_one_segment() {
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(8);
        let a = arena.alloc(10);
        let b = arena.alloc(20);
        let c = arena.alloc(30);
        assert_eq!(*arena.get(a), 10);
        assert_eq!(*arena.get(b), 20);
        assert_eq!(*arena.get(c), 30);
        // All in segment 0 with ascending offsets.
        assert_eq!((a.segment(), a.offset()), (0, 0));
        assert_eq!((b.segment(), b.offset()), (0, 1));
        assert_eq!((c.segment(), c.offset()), (0, 2));
        assert_eq!(arena.alloc_count(), 3);
    }

    #[test]
    fn alloc_spans_segments_with_stable_addresses() {
        // Capacity 4 forces a new segment on the 5th allocation.
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(4);
        let addrs: Vec<Addr> = (0..10u64).map(|v| arena.alloc(v * 100)).collect();
        // First 4 in segment 0, next 4 in segment 1, last 2 in segment 2.
        assert_eq!(addrs[0].segment(), 0);
        assert_eq!(addrs[3].segment(), 0);
        assert_eq!(addrs[4].segment(), 1);
        assert_eq!(addrs[7].segment(), 1);
        assert_eq!(addrs[8].segment(), 2);
        assert_eq!(arena.segment_count(), 3);
        // Every earlier address still reads its original value (no realloc moved it).
        for (i, &a) in addrs.iter().enumerate() {
            assert_eq!(*arena.get(a), i as u64 * 100, "stable read for alloc #{i}");
        }
    }

    #[test]
    fn mark_bits_set_test_clear() {
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(128);
        let a = arena.alloc(1);
        let b = arena.alloc(2);
        assert!(!arena.is_marked(a));
        assert!(arena.mark(a), "first mark returns was-unmarked=true");
        assert!(!arena.mark(a), "second mark returns false (idempotent)");
        assert!(arena.is_marked(a));
        assert!(!arena.is_marked(b), "marking a must not mark b");
    }

    #[test]
    fn sweep_reclaims_unmarked_and_reuses_slots() {
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(64);
        let live = arena.alloc(111);
        let dead1 = arena.alloc(222);
        let dead2 = arena.alloc(333);

        // Mark only `live`, then sweep.
        arena.mark(live);
        let stats = arena.sweep();
        assert_eq!(stats.live, 1);
        assert_eq!(stats.reclaimed_to_free_list, 2, "dead1 + dead2 freed");
        assert_eq!(arena.free_slots(), 2);

        // Live value survives unchanged; marks were cleared.
        assert_eq!(*arena.get(live), 111);
        assert!(!arena.is_marked(live), "sweep clears mark bits");

        // The next two allocations reuse the freed slots (no segment growth).
        let r1 = arena.alloc(444);
        let r2 = arena.alloc(555);
        assert_eq!(arena.free_slots(), 0, "free list drained by reuse");
        assert_eq!(arena.segment_count(), 1, "reuse, not growth");
        // Reused addresses are exactly the freed slots (dead2/dead1 LIFO order).
        assert!(
            [dead1, dead2].contains(&r1) && [dead1, dead2].contains(&r2),
            "reused the reclaimed slots"
        );
        assert_eq!(*arena.get(r1), 444);
        assert_eq!(*arena.get(r2), 555);
    }

    #[test]
    fn sweep_releases_fully_dead_segments() {
        // capacity 4: fill segment 0 fully, then allocate into segment 1.
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(4);
        let seg0: Vec<Addr> = (0..4u64).map(|v| arena.alloc(v)).collect();
        let live_in_seg1 = arena.alloc(999); // segment 1, current bump target
        assert_eq!(arena.segment_count(), 2);
        assert!(seg0.iter().all(|a| a.segment() == 0));
        assert_eq!(live_in_seg1.segment(), 1);

        // Mark only the segment-1 value; segment 0 is fully dead.
        arena.mark(live_in_seg1);
        let stats = arena.sweep();
        assert_eq!(
            stats.segments_released, 1,
            "segment 0 fully dead → released"
        );
        assert!(stats.bytes_released > 0);
        assert_eq!(stats.live, 1);
        // Segment 0's dead slots were NOT added to the free list (released wholesale).
        assert_eq!(arena.free_slots(), 0);
        // The surviving value is intact.
        assert_eq!(*arena.get(live_in_seg1), 999);
    }

    #[test]
    fn current_segment_never_released_even_if_dead() {
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(8);
        let a = arena.alloc(1);
        // Nothing marked → `a` is dead, but it lives in the current segment,
        // which must remain usable for allocation rather than be released.
        let stats = arena.sweep();
        assert_eq!(stats.segments_released, 0);
        assert_eq!(
            stats.reclaimed_to_free_list, 1,
            "the dead slot is freed for reuse"
        );
        let _ = a;
        // Arena is still usable.
        let b = arena.alloc(2);
        assert_eq!(*arena.get(b), 2);
    }

    // A concrete value-node for exercising the transitive collector core.
    #[derive(Clone, Copy)]
    enum TestNode {
        Leaf(u64),
        One(Addr),
        Pair(Addr, Addr),
    }

    impl ArenaNode for TestNode {
        fn child_addrs(&self, out: &mut Vec<Addr>) {
            match self {
                TestNode::Leaf(_) => {}
                TestNode::One(a) => out.push(*a),
                TestNode::Pair(a, b) => {
                    out.push(*a);
                    out.push(*b);
                }
            }
        }
    }

    #[test]
    fn transitive_mark_then_sweep_reclaims_only_unreachable() {
        let mut arena: IndexArena<TestNode> = IndexArena::with_segment_capacity(64);
        let l1 = arena.alloc(TestNode::Leaf(1));
        let l2 = arena.alloc(TestNode::Leaf(2));
        let pair = arena.alloc(TestNode::Pair(l1, l2)); // reachable: pair → {l1, l2}
        let orphan = arena.alloc(TestNode::Leaf(99)); // unreachable leaf
        let chain_leaf = arena.alloc(TestNode::Leaf(3));
        let chain = arena.alloc(TestNode::One(chain_leaf)); // unreachable: chain → chain_leaf

        let newly = arena.mark_from_roots(&[pair]);
        assert_eq!(newly, 3, "pair + l1 + l2 reachable from the single root");
        assert!(arena.is_marked(pair) && arena.is_marked(l1) && arena.is_marked(l2));
        assert!(!arena.is_marked(orphan));
        assert!(!arena.is_marked(chain) && !arena.is_marked(chain_leaf));

        let stats = arena.sweep();
        assert_eq!(stats.live, 3);
        assert_eq!(
            stats.reclaimed_to_free_list, 3,
            "orphan + chain + chain_leaf freed"
        );

        // Reachable nodes survive intact with their child handles unchanged.
        match arena.get(pair) {
            TestNode::Pair(a, b) => {
                assert_eq!(*a, l1);
                assert_eq!(*b, l2);
            }
            _ => panic!("reachable Pair node was corrupted by sweep"),
        }
        assert!(matches!(arena.get(l1), TestNode::Leaf(1)));
    }

    #[test]
    fn transitive_mark_visits_shared_substructure_once() {
        let mut arena: IndexArena<TestNode> = IndexArena::with_segment_capacity(64);
        let shared = arena.alloc(TestNode::Leaf(7));
        let p1 = arena.alloc(TestNode::One(shared)); // p1 → shared
        let root = arena.alloc(TestNode::Pair(shared, p1)); // root → {shared, p1}; shared reached twice

        let newly = arena.mark_from_roots(&[root]);
        assert_eq!(
            newly, 3,
            "root + shared + p1 — `shared` marked exactly once despite two paths"
        );
    }

    #[test]
    fn transitive_mark_terminates_on_cycle() {
        let mut arena: IndexArena<TestNode> = IndexArena::with_segment_capacity(64);
        let a = arena.alloc(TestNode::Leaf(0)); // placeholder
        let b = arena.alloc(TestNode::One(a)); // b → a
                                               // Close a cycle a → b → a by patching `a` to point back at `b`.
        *arena.get_mut(a) = TestNode::One(b);

        let newly = arena.mark_from_roots(&[a]);
        assert_eq!(newly, 2, "a + b; the cycle terminates via mark-bit dedup");
        assert!(arena.is_marked(a) && arena.is_marked(b));
    }

    #[test]
    fn mark_from_roots_with_sources_children_from_closure() {
        use std::collections::HashMap;
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(64);
        // The child graph lives entirely OUTSIDE the node (as SExpr children
        // will, in the side-arena): nodes are opaque values; edges are external.
        let a = arena.alloc(1);
        let b = arena.alloc(2);
        let c = arena.alloc(3);
        let orphan = arena.alloc(4);
        let mut edges: HashMap<u32, Vec<Addr>> = HashMap::new();
        edges.insert(a.raw(), vec![b, c]); // a → {b, c}

        let newly = arena.mark_from_roots_with(&[a], |addr, out| {
            if let Some(kids) = edges.get(&addr.raw()) {
                out.extend_from_slice(kids);
            }
        });
        assert_eq!(newly, 3, "a + b + c reached via the external child source");
        assert!(arena.is_marked(a) && arena.is_marked(b) && arena.is_marked(c));
        assert!(!arena.is_marked(orphan));
    }

    #[test]
    fn ensure_bump_room_and_bump_in_co_locate() {
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(2);
        let s0 = arena.ensure_bump_room();
        let a = arena.bump_in(s0, 10);
        let s0b = arena.ensure_bump_room();
        assert_eq!(s0b, s0, "segment 0 still has room");
        let b = arena.bump_in(s0b, 20);
        assert_eq!(a.segment(), 0);
        assert_eq!(b.segment(), 0);
        // Segment 0 (capacity 2) is now full → ensure_bump_room opens segment 1.
        let s1 = arena.ensure_bump_room();
        assert_eq!(s1, 1, "full segment 0 → fresh segment 1");
        let c = arena.bump_in(s1, 30);
        assert_eq!(c.segment(), 1);
        assert_eq!(*arena.get(a), 10);
        assert_eq!(*arena.get(c), 30);
    }

    #[test]
    fn sweep_with_reports_released_segment_indices() {
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(2);
        let _d0 = arena.alloc(1);
        let _d1 = arena.alloc(2); // segment 0 now full (2 dead nodes)
        let live = arena.alloc(3); // segment 1 (current), live
        arena.mark(live);

        let mut released = Vec::new();
        let stats = arena.sweep_with(|si| released.push(si));
        assert_eq!(
            released,
            vec![0],
            "segment 0 fully dead → released and reported"
        );
        assert_eq!(stats.segments_released, 1);
        assert_eq!(stats.live, 1);
    }

    // ---- B1.b: word-parallel sweep / is_fully_dead equivalence tests ----
    // Each pins a branch of the word-parallel fast path against the spec
    // (same free list, same SweepStats as the per-slot scan).

    #[test]
    fn sweep_word_parallel_all_live_and_all_dead_words() {
        // 128 slots = 2 mark words. Word 0 (offsets 0..64) all-live; word 1
        // (64..128) all-dead. Exercises the `word == u64::MAX` and `word == 0`
        // fast paths in the reclaim loop (single/current segment → not released).
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(128);
        let addrs: Vec<Addr> = (0..128u64).map(|v| arena.alloc(v)).collect();
        for a in &addrs[0..64] {
            arena.mark(*a);
        }
        let stats = arena.sweep();
        assert_eq!(stats.live, 64, "word 0 all-live → 64 live");
        assert_eq!(stats.reclaimed_to_free_list, 64, "word 1 all-dead → 64 freed");
        assert_eq!(stats.segments_released, 0, "current segment never released");
        assert_eq!(arena.free_slots(), 64);
        // Live slots survive with original values; marks cleared.
        for (i, a) in addrs[0..64].iter().enumerate() {
            assert_eq!(*arena.get(*a), i as u64);
            assert!(!arena.is_marked(*a), "sweep clears marks");
        }
        // The 64 freed slots are reused without segment growth.
        for v in 0..64u64 {
            let _ = arena.alloc(1000 + v);
        }
        assert_eq!(arena.free_slots(), 0, "free list drained by reuse");
        assert_eq!(arena.segment_count(), 1, "reuse, not growth");
    }

    #[test]
    fn sweep_word_parallel_mixed_and_partial_words() {
        // 100 slots: one complete mixed word (offsets 0..64) + a mixed partial
        // final word (64..100, 36 bits). Even offsets marked. Exercises the
        // per-bit fallback for both a complete mixed word and the partial word.
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(128);
        let addrs: Vec<Addr> = (0..100u64).map(|v| arena.alloc(v)).collect();
        let mut expected_live = 0usize;
        for (off, a) in addrs.iter().enumerate() {
            if off % 2 == 0 {
                arena.mark(*a);
                expected_live += 1;
            }
        }
        let expected_free = 100 - expected_live;
        let stats = arena.sweep();
        assert_eq!(stats.live, expected_live, "even offsets live (50)");
        assert_eq!(
            stats.reclaimed_to_free_list, expected_free,
            "odd offsets freed (50)"
        );
        assert_eq!(arena.free_slots(), expected_free);
        for (off, a) in addrs.iter().enumerate() {
            if off % 2 == 0 {
                assert_eq!(*arena.get(*a), off as u64, "live even slot unchanged");
            }
        }
    }

    #[test]
    fn sweep_word_parallel_releases_fully_dead_multiword_segment() {
        // Segment 0 (128 slots = 2 full words) fully dead and non-current →
        // released via the word-parallel `is_fully_dead` (all words zero, rem==0).
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(128);
        let _seg0: Vec<Addr> = (0..128u64).map(|v| arena.alloc(v)).collect();
        let live1 = arena.alloc(9999); // opens + makes segment 1 current
        assert_eq!(live1.segment(), 1);
        assert_eq!(arena.segment_count(), 2);
        arena.mark(live1); // all of segment 0 stays unmarked
        let stats = arena.sweep();
        assert_eq!(stats.segments_released, 1, "fully-dead seg 0 released");
        assert_eq!(stats.live, 1, "only the seg-1 slot survives");
        assert_eq!(stats.reclaimed_to_free_list, 0, "released, not reclaimed");
    }

    #[test]
    fn sweep_word_parallel_partial_word_mark_prevents_release() {
        // A FULL non-current segment of capacity 100 has a partial final mark word
        // (36 bits). A single mark at offset 99 (in that partial word) must keep
        // `is_fully_dead` false (the mask preserves the bit) → segment NOT released
        // → its other 99 slots reclaimed via the reclaim loop's partial-word path.
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(100);
        let seg0: Vec<Addr> = (0..100u64).map(|v| arena.alloc(v)).collect();
        let live1 = arena.alloc(7777); // opens segment 1 (current)
        assert_eq!(live1.segment(), 1);
        arena.mark(live1);
        arena.mark(seg0[99]); // offset 99 ∈ partial word [64,100)
        let stats = arena.sweep();
        assert_eq!(stats.segments_released, 0, "seg 0 has a live slot → not released");
        assert_eq!(stats.live, 2, "seg0[99] + live1");
        assert_eq!(stats.reclaimed_to_free_list, 99, "seg 0's other 99 slots freed");
        assert_eq!(*arena.get(seg0[99]), 99, "the live partial-word slot is intact");
    }
}
