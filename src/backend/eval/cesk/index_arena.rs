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

use std::cell::UnsafeCell;
use std::hint;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

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
/// `nodes` is allocated **once** to `capacity` `MaybeUninit` cells in [`new`] and
/// never reallocated while the segment is alive — that stability is what makes a
/// published slot reference sound to read lock-free (the plan's Finding 3). Each
/// cell is an [`UnsafeCell`] so a node can be bump-written through a shared
/// `&Segment` (the B2 `&self` allocation path) without a `&mut`.
///
/// Two cursors discipline concurrent bump allocation (B2.2 two-cursor protocol):
/// `bump` *claims* a unique offset; `len` *publishes* the contiguous written
/// prefix. A reader (the marker) reads a slot only for `off < len.load(Acquire)`,
/// which — paired with the publisher's `Release` — guarantees the node bytes are
/// fully written and visible (never uninit, never torn). See the type-level
/// SAFETY block on [`IndexArena`].
struct Segment<N: Copy> {
    /// Slot storage, `capacity` cells allocated once. A cell is initialized
    /// (via `MaybeUninit::write`) exactly once, when its offset is claimed by
    /// `bump`, before that offset is published into `len`.
    nodes: Box<[UnsafeCell<MaybeUninit<N>>]>,
    /// PUBLISH cursor: the count of fully-written, published slots. `0..len` is a
    /// contiguous written prefix — the invariant `is_fully_dead`/`sweep`/
    /// `live_node_count` rely on. Published with `Release`, read with `Acquire`.
    len: AtomicUsize,
    /// CLAIM cursor: high-water of *reserved* (claimed) slots, `len <= bump`.
    /// `fetch_add(1, Relaxed)` hands each caller a unique offset.
    bump: AtomicUsize,
    /// Per-slot mark bits, `ceil(capacity/64)` words. `AtomicU64` so a future
    /// parallel/concurrent mark sets bits without a lock. **Ordering stays
    /// `Relaxed` in B2** (B3 flips mark ordering to Release/Acquire).
    marks: Box<[AtomicU64]>,
    /// Per-segment slot capacity.
    capacity: usize,
    /// `true` once the segment has been released (its `nodes` storage dropped).
    /// `AtomicBool` (read `Relaxed`) future-proofs the D-phase concurrent reader;
    /// in B2 it is only flipped at quiescence under `&mut self`.
    released: AtomicBool,
}

impl<N: Copy> Segment<N> {
    fn new(capacity: usize) -> Self {
        debug_assert!(capacity > 0 && capacity <= DEFAULT_SEGMENT_CAPACITY);
        // `UnsafeCell`/`MaybeUninit`/`AtomicU64` are not `Clone`, so the storage
        // is built from an iterator (one cell per slot) rather than `vec![..; n]`.
        let nodes: Box<[UnsafeCell<MaybeUninit<N>>]> = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        let words = capacity.div_ceil(64);
        let marks: Box<[AtomicU64]> = (0..words).map(|_| AtomicU64::new(0)).collect();
        Segment {
            nodes,
            len: AtomicUsize::new(0),
            bump: AtomicUsize::new(0),
            marks,
            capacity,
            released: AtomicBool::new(false),
        }
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.bump.load(Ordering::Relaxed) >= self.capacity
    }

    /// Set the mark bit for `offset`. Returns `true` if previously unmarked.
    ///
    /// `AcqRel` (B3): the Release half publishes the marking thread's prior writes
    /// (in allocate-black, the node bytes the mutator wrote before blackening) to a
    /// concurrent sweep / second marker that `Acquire`-loads this word and observes
    /// the bit; the Acquire half orders a parallel marker's subsequent worklist
    /// reads (and keeps the `prev & bit` idempotence correct across markers racing
    /// to gray a shared node). Byte-identical at the current single-threaded runtime
    /// — Relaxed→stronger only ADDS happens-before; with one thread it is identical.
    #[inline]
    fn set_mark(&self, offset: usize) -> bool {
        let bit = 1u64 << (offset & 63);
        let prev = self.marks[offset >> 6].fetch_or(bit, Ordering::AcqRel);
        (prev & bit) == 0
    }

    /// `Acquire` (B3): observing the bit set happens-after the `set_mark` (AcqRel)
    /// that set it, so the reader also observes that marker's published writes.
    #[inline]
    fn is_marked(&self, offset: usize) -> bool {
        let bit = 1u64 << (offset & 63);
        (self.marks[offset >> 6].load(Ordering::Acquire) & bit) != 0
    }

    /// `Release` (B3): cycle N's zeroing is ordered before cycle N+1's first
    /// `set_mark`/`is_marked` (Acquire) observes the word, so a stale set bit from
    /// the previous cycle never leaks into the next mark phase.
    #[inline]
    fn clear_marks(&self) {
        for w in self.marks.iter() {
            w.store(0, Ordering::Release);
        }
    }

    /// `true` if no slot in `0..len` is marked (B1.b word-parallel form preserved).
    ///
    /// Reads the PUBLISH cursor with `Acquire` (paired with the publisher's
    /// `Release`, every published slot is observed) AND each mark word with
    /// `Acquire` (B3 — paired with `set_mark`'s AcqRel, the sweep, as the mark
    /// CONSUMER, observes every bit any marker set, so a live slot is never
    /// reclaimed). OR together the complete mark words covering `0..len` (one load
    /// per 64 slots), masking the final partial word to the valid low `len & 63`
    /// bits (offsets `>= len` are never set by `set_mark`, so the mask only guards
    /// padding bits).
    fn is_fully_dead(&self) -> bool {
        let len = self.len.load(Ordering::Acquire);
        let full_words = len >> 6;
        for wi in 0..full_words {
            if self.marks[wi].load(Ordering::Acquire) != 0 {
                return false;
            }
        }
        let rem = len & 63;
        if rem != 0 {
            let mask = (1u64 << rem) - 1;
            if (self.marks[full_words].load(Ordering::Acquire) & mask) != 0 {
                return false;
            }
        }
        true
    }

    /// Borrow the published node at `offset`.
    ///
    /// # Safety
    /// The caller must guarantee `offset < self.len.load(Acquire)` *as observed by
    /// the calling thread*, i.e. `offset` lies in the published prefix. Publication
    /// (`len.compare_exchange(.., Release)`) happens-after the slot's
    /// `MaybeUninit::write`, so an `Acquire`-load of `len` that observes
    /// `len > offset` also observes the fully-written node bytes. Under that
    /// premise the cell is initialized and not concurrently mutated (a published
    /// slot is rewritten only by free-list reuse, which runs only at quiescence —
    /// see the `IndexArena` SAFETY block), so the `&N` is a valid shared borrow.
    #[inline]
    unsafe fn node_at(&self, offset: usize) -> &N {
        // `UnsafeCell::get()` yields `*mut MaybeUninit<N>`; the reference produced
        // does NOT borrow `&self` (it derives from the raw pointer), which is what
        // lets `IndexArena::get`/`segment` return `&N` without aliasing conflicts.
        let cell = self.nodes[offset].get();
        (*cell).assume_init_ref()
    }

    /// Claim a unique slot offset by bumping the CLAIM cursor. Returns `None` once
    /// the segment is full. `Relaxed` is sufficient: the offset is made safe to
    /// read only by the subsequent `publish` (`Release`); the claim itself only
    /// needs atomic uniqueness, which `fetch_add` provides on any ordering.
    #[inline]
    fn bump_one(&self) -> Option<usize> {
        let off = self.bump.fetch_add(1, Ordering::Relaxed);
        if off >= self.capacity {
            None
        } else {
            Some(off)
        }
    }

    /// Write `node` into the (uniquely claimed, exclusive) slot `off`.
    ///
    /// # Safety
    /// `off` must have been returned by a prior `bump_one` on `self` and not yet
    /// written — i.e. the caller holds the unique claim to `off` and `off` is not
    /// yet published. Under that premise the write is exclusive (no other thread
    /// can hold the same claim) and races no reader (`off >= len` until publish).
    #[inline]
    unsafe fn write_claimed(&self, off: usize, node: N) {
        (*self.nodes[off].get()).write(node);
    }

    /// Publish slot `off` into the contiguous written prefix.
    ///
    /// Spins until `len == off`, then advances `len` to `off + 1` with `Release`
    /// ordering (so the prior `write_claimed` happens-before any `Acquire`-load of
    /// `len` that observes the new value). Keeps `[0, len)` a contiguous written
    /// prefix even when claims complete out of order. Under the B2 single-bumper
    /// scope the CAS always succeeds first try (`len == off` already), degenerating
    /// to a `store(off+1, Release)`; the CAS form is implemented so the invariant
    /// survives a future relaxation (concurrent bumpers within one segment).
    #[inline]
    fn publish(&self, off: usize) {
        while self
            .len
            .compare_exchange_weak(off, off + 1, Ordering::Release, Ordering::Relaxed)
            .is_err()
        {
            hint::spin_loop();
        }
    }

    /// Drop the slot storage, returning the byte estimate freed. `&mut self`:
    /// release runs only inside `sweep_with` at quiescence (exclusive access), so
    /// the storage teardown cannot race a reader. The freed-byte estimate uses
    /// `capacity` (storage is allocated once to `capacity`, never grown).
    fn release(&mut self) -> usize {
        let freed = self.capacity * std::mem::size_of::<N>();
        // Drop the once-allocated cell storage.
        self.nodes = Box::new([]);
        self.len.store(0, Ordering::Relaxed);
        self.bump.store(0, Ordering::Relaxed);
        self.released.store(true, Ordering::Relaxed);
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
    /// Never-realloc segment directory: `MAX_SEGMENTS` cells, allocated once.
    /// Cell `i` is initialized (its `Box<Segment>` written) exactly once, under
    /// `dir_lock`, before `seg_count` is advanced past `i`. A reader dereferences
    /// cell `i` only for `i < seg_count.load(Acquire)`.
    segments: Box<[UnsafeCell<MaybeUninit<Box<Segment<N>>>>]>,
    /// Published directory length (count of initialized cells). Monotone;
    /// advanced with `Release` under `dir_lock`, read with `Acquire`.
    seg_count: AtomicUsize,
    /// Current bump-target segment index. Advanced with `Release` under
    /// `dir_lock`; read with `Acquire`.
    cur_seg: AtomicUsize,
    /// Serializes directory growth (`open_segment`): the rare slow path. A plain
    /// `Mutex<()>` — bump allocation does NOT take it (only the open of a fresh
    /// segment does), so the steady-state fast path is lock-free.
    dir_lock: Mutex<()>,
    /// Per-segment slot capacity for new segments.
    segment_capacity: usize,
    /// Free slots reclaimed by the last sweep, consumed by allocation **at
    /// quiescence only** (rebuilt-from-scratch each sweep, never persisted). Stays
    /// a plain `Vec<Addr>` accessed under `&mut self`: it is touched only by
    /// `sweep_with` (rebuild) and the quiescent `&mut self` `alloc` free-list path
    /// — never by a concurrent `&self` bump — so no atomics are needed.
    free_list: Vec<Addr>,
    /// Total slots ever bump-allocated (diagnostics). `AtomicU64` so a concurrent
    /// `&self` `bump_in`/`alloc` can increment it; read `Relaxed` (diagnostic only).
    alloc_count: AtomicU64,
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

    /// A new arena whose segments hold `capacity` slots each. Small capacities are
    /// used by tests to exercise multi-segment behavior cheaply.
    ///
    /// Allocates the full `MAX_SEGMENTS`-cell directory once (each cell is an
    /// uninitialized `MaybeUninit<Box<Segment>>` — `8 * MAX_SEGMENTS = 128 KiB` of
    /// pointer slots, see the risk note on directory cost), then opens segment 0.
    pub fn with_segment_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0 && capacity <= DEFAULT_SEGMENT_CAPACITY,
            "segment capacity {capacity} out of range 1..={DEFAULT_SEGMENT_CAPACITY}"
        );
        // `UnsafeCell`/`MaybeUninit` are not `Clone`, so the directory is built
        // from an iterator (one uninit cell per addressable segment index) rather
        // than `vec![..; MAX_SEGMENTS]`. Allocated once, never reallocated.
        let segments: Box<[UnsafeCell<MaybeUninit<Box<Segment<N>>>>]> = (0..MAX_SEGMENTS)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        let arena = IndexArena {
            segments,
            seg_count: AtomicUsize::new(0),
            cur_seg: AtomicUsize::new(0),
            dir_lock: Mutex::new(()),
            segment_capacity: capacity,
            free_list: Vec::new(),
            alloc_count: AtomicU64::new(0),
        };
        arena.open_segment(); // publishes segment 0; sets cur_seg = 0
        arena
    }

    /// Open a fresh segment and make it the bump target. Returns its index.
    ///
    /// `&self`: takes `dir_lock` (the rare slow path — once per `capacity`
    /// allocations), writes the next directory cell, then publishes it by
    /// advancing `seg_count` (`Release`) and retargets `cur_seg` (`Release`). The
    /// `Release` stores pair with the `Acquire` loads in `segment`/`current_seg`
    /// so a reader that observes `i < seg_count` also observes the initialized
    /// cell `i` (its `Box<Segment>` ptr and the segment's initial `len = 0`).
    fn open_segment(&self) -> usize {
        let _guard = self.dir_lock.lock().expect("index-arena dir_lock poisoned");
        let idx = self.seg_count.load(Ordering::Relaxed); // exclusive under the guard
        assert!(
            idx < MAX_SEGMENTS,
            "arena exhausted: {MAX_SEGMENTS} segments"
        );
        let seg = Box::new(Segment::new(self.segment_capacity));
        // SAFETY: cell `idx` is not yet published (`idx == seg_count`), so no
        // reader can observe it; under `dir_lock` we are the unique writer of this
        // cell. Initialize it before publishing `idx` into `seg_count`.
        unsafe {
            (*self.segments[idx].get()).write(seg);
        }
        self.seg_count.store(idx + 1, Ordering::Release); // publish the cell
        self.cur_seg.store(idx, Ordering::Release); // retarget bump
        idx
    }

    /// Borrow segment `i` of the published directory.
    ///
    /// # Safety
    /// The caller must guarantee `i < self.seg_count.load(Acquire)` as observed by
    /// the calling thread — i.e. cell `i` has been published by `open_segment`'s
    /// `Release` store to `seg_count`, which happens-after the cell's
    /// initialization. Under that premise the cell is initialized.
    ///
    /// The returned `&Segment<N>` derives from the raw pointer
    /// `UnsafeCell::get()` returns (`*mut MaybeUninit<Box<Segment>>`), NOT from a
    /// borrow of `&self.segments`. This is deliberate: the reference does not
    /// borrow `self`, so `sweep_with` can hold a `&Segment` (to read its marks)
    /// while *also* mutably borrowing `self.free_list` to push reclaimed slots —
    /// the borrow-checker resolution in §4.
    #[inline]
    unsafe fn segment(&self, i: usize) -> &Segment<N> {
        let cell = self.segments[i].get(); // *mut MaybeUninit<Box<Segment<N>>>
        (*cell).assume_init_ref() // &Box<Segment<N>> -> &Segment<N> via Deref
    }

    /// The current bump-target segment index (published).
    #[inline]
    fn current_seg(&self) -> usize {
        self.cur_seg.load(Ordering::Acquire)
    }

    /// Allocate `node`, returning its address. Prefers a reused free slot
    /// (quiescence-only, `&mut self`), else bump-allocates in the current segment
    /// (opening a new one if full). Byte-identical to the pre-B2 path: free-list
    /// LIFO reuse, then fresh bump.
    pub fn alloc(&mut self, node: N) -> Addr {
        if let Some(addr) = self.free_list.pop() {
            self.alloc_count.fetch_add(1, Ordering::Relaxed);
            // SAFETY: `addr` came from the last sweep's reclaim of a *published*
            // slot in a non-released segment; `&mut self` (quiescence) ⇒ no reader.
            unsafe {
                let seg = self.segment(addr.segment());
                debug_assert!(!seg.released.load(Ordering::Relaxed));
                // Overwrite the (already-initialized, already-published) slot in
                // place — exclusive under `&mut self`. The slot stays published
                // (len unchanged), so no publish step.
                (*seg.nodes[addr.offset()].get()).write(node);
            }
            return addr;
        }
        // Fresh bump (the path a concurrent producer also takes, but here under
        // `&mut self`). Delegates to the `&self` primitive for a single impl.
        self.alloc_bump(node)
    }

    /// Fresh-bump-only allocation — `&self`, lock-free-capable. Claims a unique
    /// slot via the current segment's atomic `bump` cursor, writes the node, and
    /// publishes it; opens a fresh segment when the current one is full. **Never
    /// touches the free list** (free-list reuse is `&mut self`/quiescence-only, so
    /// a concurrent claim can never alias a reused slot — the ABA/torn-node class
    /// B2 avoids; TLA+ `NoConcurrentFree`). This is the substrate B3 TLABs use.
    pub fn alloc_bump(&self, node: N) -> Addr {
        loop {
            let si = self.current_seg();
            // SAFETY: `si == cur_seg < seg_count` (cur_seg is only ever set to a
            // published index by `open_segment`), so the cell is initialized.
            let seg = unsafe { self.segment(si) };
            if !seg.released.load(Ordering::Relaxed) {
                if let Some(off) = seg.bump_one() {
                    // SAFETY: `off` uniquely claimed by this thread (no aliasing);
                    // not yet published, so it races no reader.
                    unsafe { seg.write_claimed(off, node) };
                    seg.publish(off);
                    self.alloc_count.fetch_add(1, Ordering::Relaxed);
                    return Addr::new(si as u32, off as u32);
                }
            }
            // Current segment full/released: open a fresh one and retry. Multiple
            // threads may race here; `open_segment` serializes under `dir_lock`,
            // and a loser simply re-reads the advanced `cur_seg` next iteration.
            self.open_segment();
        }
    }

    /// Borrow the node at `addr`. The slot must be published (it is, for any
    /// `Addr` ever returned by allocation — allocation publishes before returning).
    #[inline]
    pub fn get(&self, addr: Addr) -> &N {
        // SAFETY: `addr.segment()` was published (it indexes a segment that
        // produced `addr` via allocation, so `< seg_count`); `addr.offset()` was
        // published before `addr` was handed out (`alloc`/`bump_in` publish then
        // return), so `offset < len` for this thread. Not released (a live `Addr`
        // names a non-released segment — release only at quiescence after the
        // marker proved it unreachable).
        unsafe {
            let seg = self.segment(addr.segment());
            debug_assert!(
                !seg.released.load(Ordering::Relaxed),
                "get on a released segment {}",
                addr.segment()
            );
            debug_assert!(
                addr.offset() < seg.len.load(Ordering::Acquire),
                "get on an unpublished offset {}",
                addr.offset()
            );
            seg.node_at(addr.offset())
        }
    }

    /// Mutably borrow the node at `addr`. **Stays `&mut self`** — its only caller
    /// is the arena's own cycle test; `IndexHeap` never exposes a mutable node
    /// borrow (the value model is immutable post-publish). `&mut self` ⇒ exclusive
    /// ⇒ no concurrent reader, so the in-place rewrite is sound.
    #[inline]
    pub fn get_mut(&mut self, addr: Addr) -> &mut N {
        // SAFETY: `&mut self` is exclusive; `addr` names a published, non-released
        // slot (see `get`). The `&mut N` aliases nothing.
        unsafe {
            let cell = self.segment(addr.segment()).nodes[addr.offset()].get();
            (*cell).assume_init_mut()
        }
    }

    /// Mark `addr` live. Returns `true` if newly marked. `&self` (already was) —
    /// `set_mark` is an atomic `fetch_or`.
    #[inline]
    pub fn mark(&self, addr: Addr) -> bool {
        // SAFETY: `addr.segment() < seg_count` (it names a live, published slot).
        unsafe { self.segment(addr.segment()) }.set_mark(addr.offset())
    }

    /// Whether `addr` is currently marked.
    #[inline]
    pub fn is_marked(&self, addr: Addr) -> bool {
        // SAFETY: as `mark`.
        unsafe { self.segment(addr.segment()) }.is_marked(addr.offset())
    }

    /// Number of segments currently published (including released ones, which
    /// retain their directory cell so addresses stay stable).
    #[inline]
    pub fn segment_count(&self) -> usize {
        self.seg_count.load(Ordering::Acquire)
    }

    /// Total allocations performed (diagnostics).
    #[inline]
    pub fn alloc_count(&self) -> u64 {
        self.alloc_count.load(Ordering::Relaxed)
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
        let n = self.seg_count.load(Ordering::Acquire);
        for si in 0..n {
            // SAFETY: si < seg_count ⇒ published.
            let seg = unsafe { self.segment(si) };
            if !seg.released.load(Ordering::Relaxed) {
                total += seg.capacity * per;
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
        let n = self.seg_count.load(Ordering::Acquire);
        for si in 0..n {
            // SAFETY: si < seg_count ⇒ published.
            let seg = unsafe { self.segment(si) };
            if !seg.released.load(Ordering::Relaxed) {
                total += seg.len.load(Ordering::Acquire);
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

        let seg_count = self.seg_count.load(Ordering::Acquire);
        let cur = self.cur_seg.load(Ordering::Acquire);
        for si in 0..seg_count {
            // Read this segment through a RAW POINTER local, NOT the `segment()`
            // helper: `segment()` returns `&Segment` whose lifetime is threaded
            // through `&self`, which the borrow checker then treats as a live
            // shared borrow of `*self` for the whole loop body — conflicting with
            // `self.free_list.push` (E0502). A `*mut Segment` derived from
            // `UnsafeCell::get()` carries no lifetime (it launders through a raw
            // pointer, §4), so the per-use `&*seg_ptr` / `&mut *seg_ptr` borrows
            // are independent of `self` and coexist with `&mut self.free_list`.
            // SAFETY: si < seg_count ⇒ published; `&mut self` ⇒ quiescence, no
            // concurrent mutator, so reading marks/len and (for release) taking a
            // `&mut Segment` is exclusive. The cell is initialized (published).
            let seg_ptr: *mut Segment<N> = unsafe {
                // `assume_init_mut()` yields `&mut Box<Segment<N>>`; deref the Box
                // (`**`) to reach the `Segment`, then take a transient `&mut` and
                // cast to a raw pointer (the `&mut` is consumed by the cast, not
                // held — so it neither aliases nor outlives anything).
                &mut **(*self.segments[si].get()).assume_init_mut() as *mut Segment<N>
            };
            if unsafe { (*seg_ptr).released.load(Ordering::Relaxed) } {
                continue;
            }
            let is_current = si == cur;
            let fully_dead = unsafe { (*seg_ptr).is_fully_dead() };

            if fully_dead && !is_current {
                on_release(si);
                // Take a `&mut Segment` through the raw pointer to drop its storage.
                // SAFETY: `&mut self` is exclusive; no other reference to this
                // segment is live across this point.
                let seg_mut: &mut Segment<N> = unsafe { &mut *seg_ptr };
                stats.bytes_released += seg_mut.release();
                stats.segments_released += 1;
                continue;
            }

            // Partially-live (or current): reclaim unmarked slots. B1.b
            // word-parallel fast path preserved verbatim. Mark words are read with
            // `Acquire` (B3): the sweep is the mark CONSUMER, so it must observe
            // every bit any marker set (else it reclaims a live slot — UAF); each
            // load pairs with `set_mark`'s AcqRel.
            let seg: &Segment<N> = unsafe { &*seg_ptr };
            let len = seg.len.load(Ordering::Acquire);
            let full_words = len >> 6;
            let rem = len & 63;
            for wi in 0..full_words {
                let word = seg.marks[wi].load(Ordering::Acquire);
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
                let word = seg.marks[full_words].load(Ordering::Acquire);
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
            seg.clear_marks();
        }

        // If the current segment was released-eligible but kept, or all
        // non-current segments died, ensure cur_seg points at a usable segment.
        // SAFETY: cur < seg_count ⇒ published.
        let cur_released = unsafe { self.segment(cur) }
            .released
            .load(Ordering::Relaxed);
        if cur_released {
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

    /// Ensure the current segment can bump-allocate, opening a fresh segment if
    /// full/released. Returns the segment index for the next `bump_in`. `&self`.
    pub fn ensure_bump_room(&self) -> usize {
        loop {
            let si = self.current_seg();
            // SAFETY: si == cur_seg < seg_count ⇒ published.
            let seg = unsafe { self.segment(si) };
            if !seg.released.load(Ordering::Relaxed) && !seg.is_full() {
                return si;
            }
            self.open_segment();
        }
    }

    /// Bump-allocate `node` into `seg` (must be the current, non-full, non-released
    /// segment from `ensure_bump_room`). `&self`: claims a unique offset, writes,
    /// publishes. Bypasses the free list (caller controls side-arena co-location).
    pub fn bump_in(&self, seg: usize, node: N) -> Addr {
        assert_eq!(
            seg,
            self.current_seg(),
            "bump_in target must be the current segment"
        );
        // SAFETY: seg == cur_seg < seg_count ⇒ published.
        let s = unsafe { self.segment(seg) };
        debug_assert!(!s.released.load(Ordering::Relaxed));
        let off = s
            .bump_one()
            .expect("bump_in called on a full segment (ensure_bump_room contract)");
        // SAFETY: `off` uniquely claimed; not yet published ⇒ no reader race.
        unsafe { s.write_claimed(off, node) };
        s.publish(off);
        self.alloc_count.fetch_add(1, Ordering::Relaxed);
        Addr::new(seg as u32, off as u32)
    }
}

// SAFETY: `IndexArena<N>` contains `UnsafeCell`s (in the segment directory and in
// each segment's slot storage), which makes it `!Sync`/`!Send` by default. The
// following manual impls are sound because every `&self` access to interior-
// mutable state obeys the B2 concurrency protocol:
//
//   (i)   UNIQUE CLAIM. A slot is *written* only after `bump.fetch_add(1, Relaxed)`
//         hands the writing thread a unique offset. No two threads ever obtain the
//         same offset, so the `MaybeUninit::write` is exclusive — no writer aliases
//         another writer.
//
//   (ii)  PUBLISHED READS ONLY, RELEASE/ACQUIRE-ORDERED. A reader (`get`/`node_at`/
//         the marker) dereferences slot `off` only for `off < len.load(Acquire)`.
//         The writer publishes with `len.CAS(off→off+1, Release)` *after* its
//         `write`, so an `Acquire`-load observing `len > off` happens-after the
//         write ⇒ the reader sees fully-initialized, non-torn bytes. A slot is
//         never read before it is published ⇒ no uninit/torn read, no read/write
//         race.
//
//   (iii) DIRECTORY PUBLICATION. A reader dereferences directory cell `i` only for
//         `i < seg_count.load(Acquire)`. `open_segment` initializes cell `i` then
//         does `seg_count.store(i+1, Release)` (under `dir_lock`, the unique
//         writer of cell `i`), so observing `i < seg_count` happens-after the
//         cell's initialization ⇒ the `Box<Segment>` ptr and the segment's initial
//         `len = 0` are visible before the reader indexes it.
//
//   (iv)  FREE-LIST REUSE AT QUIESCENCE. The only in-place rewrite of an *already-
//         published* slot is free-list reuse (`alloc`'s pop path) and the only
//         producer of free slots is `sweep_with`. Both are `&mut self` and run
//         only at a quiescent safepoint (the collector gate is closed whenever any
//         worker exists). `&mut self` is statically exclusive of every `&self`
//         reader/bumper, so reuse never races a concurrent access (ABA-free by
//         construction; TLA+ `NoConcurrentFree`).
//
// Hence no data race on any field. `Send` additionally requires `N: Send` (the
// arena owns `N` values it may hand to another thread); `Sync` additionally
// requires `N: Send + Sync` (a shared `&IndexArena` lets multiple threads obtain
// `&N`, and moving an `N` out — none of the API does, but the bound is the
// conventional, conservative one). `free_list: Vec<Addr>` and the atomics/`Mutex`
// are all `Send`/`Sync` for `Addr: Send + Sync`.
unsafe impl<N: Copy + Send> Send for IndexArena<N> {}
unsafe impl<N: Copy + Send + Sync> Sync for IndexArena<N> {}

// SAFETY: `Segment<N>`'s interior mutability (the `UnsafeCell` slot cells) is
// disciplined by the same claim/publish protocol as `IndexArena` (see above);
// `marks`/`len`/`bump`/`released` are atomics. Sound for the same N bounds.
unsafe impl<N: Copy + Send> Send for Segment<N> {}
unsafe impl<N: Copy + Send + Sync> Sync for Segment<N> {}

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
    fn alloc_bump_fresh_only_skips_free_list() {
        // `alloc_bump` (&self) must NEVER consume the free list — it claims fresh
        // bump space only. Sweep frees a slot; a following `alloc_bump` bumps past
        // it (free_slots stays nonzero), whereas `alloc` (&mut) would reuse it.
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(64);
        let a = arena.alloc(1);
        let b = arena.alloc(2);
        // Mark `a` (the LIVE slot); `b` is unmarked and becomes the freed slot.
        arena.mark(a);
        let stats = arena.sweep();
        assert_eq!(
            stats.reclaimed_to_free_list, 1,
            "the unmarked slot (b) is freed"
        );
        assert_eq!(arena.free_slots(), 1);
        // `&self` fresh bump: does not pop the free list (it bumps past it).
        let c = arena.alloc_bump(3);
        assert_eq!(
            arena.free_slots(),
            1,
            "alloc_bump leaves the free list intact"
        );
        assert_ne!(c, b, "alloc_bump did not reuse the freed slot");
        assert_ne!(c, a, "alloc_bump did not clobber the live slot");
        assert_eq!(*arena.get(c), 3);
        // And `alloc` (&mut) still reuses the freed slot, proving the two paths
        // differ as designed. The reused slot is `b` (the freed one), NOT `a`
        // (which stayed live/marked and is never on the free list).
        let d = arena.alloc(4);
        assert_eq!(arena.free_slots(), 0, "alloc reuses the freed slot");
        assert_eq!(d, b, "alloc reused the freed slot (b) LIFO");
        assert_ne!(d, a, "the live slot a was never freed, so never reused");
        assert_eq!(*arena.get(a), 1, "the live slot a is intact");
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
        assert_eq!(
            stats.reclaimed_to_free_list, 64,
            "word 1 all-dead → 64 freed"
        );
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
        assert_eq!(
            stats.segments_released, 0,
            "seg 0 has a live slot → not released"
        );
        assert_eq!(stats.live, 2, "seg0[99] + live1");
        assert_eq!(
            stats.reclaimed_to_free_list, 99,
            "seg 0's other 99 slots freed"
        );
        assert_eq!(
            *arena.get(seg0[99]),
            99,
            "the live partial-word slot is intact"
        );
    }
}

// ============================================================================
// loom model — the bump/publish/read protocol proof (B2)
// ============================================================================
//
// Builds only under `--cfg loom`. loom exhaustively explores every
// Acquire/Release interleaving of: two writer threads each claim a unique slot
// (`bump.fetch_add`), write it, and publish it contiguously
// (`len.CAS(off→off+1, Release)`); a reader thread loads `len` (Acquire) and
// reads every published slot. The model asserts the three B2 invariants:
//   (i)   no two writers ever obtain the same offset (unique claim);
//   (ii)  every slot the reader reads is fully written (no uninit / torn read);
//   (iii) `len` is always a contiguous written prefix `[0, len)`.
//
// RUN (capped, FOREGROUND — an uncapped loom run can blow memory exploring the
// state space). VERIFIED-GREEN command:
//   RUSTFLAGS="--cfg loom -C target-cpu=native" LOOM_MAX_PREEMPTIONS=2 \
//     systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0 -p CPUQuota=1200% \
//     cargo test --release --lib --features index-gc \
//     backend::eval::cesk::index_arena::loom_model -- --nocapture
// (`-C target-cpu=native` is RE-ADDED because setting RUSTFLAGS overrides
// .cargo/config.toml, which would otherwise drop the gxhash AES/SSE2 flags.)
// TWO requirements, both load-bearing (each empirically needed — without them the
// run fails, NOT a protocol bug):
//   * `--release`: loom runs each thread on a fixed-size `generator` coroutine
//     stack; the DEBUG-profile frame of this large crate overflows it ("coroutine
//     has overflowed its stack", failing on the first thread in 0.00s). Release's
//     smaller frames fit. loom's model checking is runtime (its instrumented
//     atomics/cells track accesses regardless of opt level), so release is a valid
//     check; assert! fires in release too.
//   * `LOOM_MAX_PREEMPTIONS=2`: bounds the schedule tree for tractability (this
//     model's meaningful interleavings — the two publish orders + the reader's
//     observation point — are all within 2 preemptions).
// The model itself uses the STRONG `compare_exchange` (not `_weak`) and
// `thread::yield_now()` (not `hint::spin_loop()`) — see `claim_write_publish`.
#[cfg(loom)]
mod loom_model {
    use super::*;
    use loom::cell::UnsafeCell;
    use loom::sync::atomic::{AtomicUsize, Ordering};
    use loom::sync::Arc;
    use loom::thread;

    /// Minimal mirror of `Segment`'s slot machinery (the protocol under test),
    /// over loom's instrumented `UnsafeCell`/atomics. `CAP` slots; two writers,
    /// one reader. Values are `usize` (a per-writer tagged payload) so the reader
    /// can assert it read a fully-written, sensible value (not torn/uninit).
    struct LoomSeg {
        slots: Vec<UnsafeCell<MaybeUninit<usize>>>,
        len: AtomicUsize,
        bump: AtomicUsize,
        cap: usize,
    }
    // SAFETY: same protocol as `Segment` (claim-unique / publish-Release /
    // read-published); loom verifies the absence of races.
    unsafe impl Sync for LoomSeg {}
    unsafe impl Send for LoomSeg {}

    impl LoomSeg {
        fn new(cap: usize) -> Self {
            LoomSeg {
                slots: (0..cap)
                    .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                    .collect(),
                len: AtomicUsize::new(0),
                bump: AtomicUsize::new(0),
                cap,
            }
        }
        fn bump_one(&self) -> Option<usize> {
            let off = self.bump.fetch_add(1, Ordering::Relaxed);
            if off >= self.cap {
                None
            } else {
                Some(off)
            }
        }
        // claim+write+publish for a single value; returns the claimed offset.
        fn claim_write_publish(&self, value: usize) -> Option<usize> {
            let off = self.bump_one()?;
            // exclusive: `off` uniquely claimed.
            self.slots[off].with_mut(|p| unsafe { (*p).write(value) });
            // Publish contiguously: spin until `len == off`. TWO loom-model-only
            // adaptations (production `Segment::publish` keeps `compare_exchange_weak`
            // + `hint::spin_loop()`, which are correct there):
            //   (1) STRONG `compare_exchange` (not `_weak`): loom models a weak CAS
            //       as able to fail SPURIOUSLY, so a weak CAS in a spin loop lets
            //       loom explore unboundedly many spurious failures within a SINGLE
            //       execution — the coroutine never exits the loop and overflows its
            //       stack. The strong CAS fails only when `len != off` (a real wait
            //       on the lower-offset writer), which is bounded. The protocol proof
            //       (contiguity / Release-Acquire happens-before / no-torn-read) is
            //       identical — weak only adds benign retries that re-establish the
            //       same invariant.
            //   (2) `thread::yield_now()` (not `hint::spin_loop()`): a waiter blocked
            //       on a lower offset must YIELD to loom's scheduler so that writer
            //       is run; a CPU `spin_loop` is not a loom scheduling point.
            while self
                .len
                .compare_exchange(off, off + 1, Ordering::Release, Ordering::Relaxed)
                .is_err()
            {
                thread::yield_now();
            }
            Some(off)
        }
    }

    #[test]
    fn loom_two_writers_one_reader_bump_publish() {
        loom::model(|| {
            const CAP: usize = 4;
            let seg = Arc::new(LoomSeg::new(CAP));

            // Two writers, each publishing one tagged value.
            let w1 = {
                let seg = seg.clone();
                thread::spawn(move || seg.claim_write_publish(0xA0)) // tag A
            };
            let w2 = {
                let seg = seg.clone();
                thread::spawn(move || seg.claim_write_publish(0xB0)) // tag B
            };

            // Reader: observe the published prefix and read every slot in it.
            let r = {
                let seg = seg.clone();
                thread::spawn(move || {
                    let n = seg.len.load(Ordering::Acquire);
                    // (iii) len is a contiguous prefix: 0..=2 (0, 1, or both done).
                    assert!(n <= CAP, "len {n} exceeded capacity {CAP}");
                    for off in 0..n {
                        // (ii) every published slot is fully written: the value is
                        // exactly one of the two tags, never uninit/torn. loom
                        // would surface a data race here if the read could observe
                        // an unpublished/half-written slot.
                        let v = seg.slots[off].with(|p| unsafe { (*p).assume_init() });
                        assert!(
                            v == 0xA0 || v == 0xB0,
                            "slot {off} read torn/uninit value {v:#x}"
                        );
                    }
                    n
                })
            };

            let o1 = w1.join().expect("w1");
            let o2 = w2.join().expect("w2");
            let _ = r.join().expect("r");

            // (i) unique claim: the two writers got DIFFERENT offsets.
            if let (Some(a), Some(b)) = (o1, o2) {
                assert_ne!(a, b, "two writers claimed the same offset {a}");
            }

            // Final state: both published ⇒ len == 2, prefix fully written.
            let final_len = seg.len.load(Ordering::Acquire);
            assert_eq!(final_len, 2, "both writers must have published");
            let mut seen = [false; 2];
            for off in 0..final_len {
                let v = seg.slots[off].with(|p| unsafe { (*p).assume_init() });
                match v {
                    0xA0 => seen[0] = true,
                    0xB0 => seen[1] = true,
                    other => panic!("unexpected published value {other:#x}"),
                }
            }
            assert!(
                seen[0] && seen[1],
                "both tags present in the contiguous prefix"
            );
        });
    }
}
