I have everything I need. Key final facts:

1. **`cesk/mod.rs:46-48`**: `pub mod index_arena; pub mod index_heap; pub mod index_node;` — non-cfg-gated (compile in both builds). Confirmed.
2. **No `[lints]` / `unexpected_cfgs` config** anywhere. Rust nightly 1.97 emits an `unexpected_cfgs` **warn** (not deny) for `--cfg loom`. But the gate is "lib 49 warnings BOTH builds" — a new warning would break it. So I MUST declare `loom` via `println!("cargo::rustc-check-cfg=cfg(loom)")` in `build.rs`. This is a load-bearing build.rs edit.
3. **All `IndexArena`/`IndexHeap` test constructions are inside `index_arena.rs` and `index_heap.rs` themselves** (the grep mangled output to `ln`/`lnln` but the count is 29, all in those two files plus the global init). No external test file constructs an arena. So the 128 KiB directory cost hits ~13 arena test fns + ~7 heap test fns + the global = ~21 small arenas in the test binary, transiently. Quantifiable.
4. **`get_mut` has no external caller** — only `index_arena.rs` test line 813 uses it. It can stay `&mut self`.
5. **rustc 1.97 nightly** — `Box::new_uninit_slice` / `MaybeUninit` slice helpers available but I'll use stable-safe construction to be portable.

I have all the grounding I need. Let me produce the complete design.

---

# Phase B2 — Concurrent-Capable Index-Arena Interior: Precise Line-Level Implementation Design

## 0. Grounding summary (verified against HEAD `b8ef27e`, tree clean)

| Fact | Source | Consequence for B2 |
|---|---|---|
| `index_arena.rs` is 976 lines; structs/methods exactly as the prompt states | `index_arena.rs:1-976` (full read) | Rewrite is total-file; line numbers below are the *current* anchors. |
| `IndexHeap.arena: IndexArena<Node>` behind `static OnceLock<RwLock<IndexHeap>>` | `index_heap.rs:75, 476-481` | Runtime stays serialized by `.write()` → byte-identical, as framed. |
| `IndexHeap` never calls `arena.get_mut`; `get_mut` used only by `index_arena.rs:813` test | crate-wide grep (no external hits) | **`get_mut` stays `&mut self`** — zero blast radius. |
| `IndexHeap::mark` already splits borrows `let arena=&self.arena; let sides=&self.sides;` | `index_heap.rs:380-395` | `mark`/`mark_from_roots_with` already `&self` — no signature change there. |
| `IndexHeap::sweep` already does `let sides=&mut self.sides; self.arena.sweep_with(|seg| …)` | `index_heap.rs:424-429` | `sweep_with` stays `&mut self`. The closure borrows `sides` mutably *disjoint* from `self.arena` — preserved. |
| `Node: Copy` (`#[derive(Clone, Copy)]`), `Drop`-free | `index_node.rs:65-66`, module docs lines 3-8 | `MaybeUninit<N>` needs no drop; `assume_init_ref` is the read primitive. `N: Copy` bound stays. |
| `[features] index-gc = []`; both modules non-cfg-gated | `Cargo.toml:280`, `mod.rs:46-48` | Must compile + pass nextest in BOTH builds. |
| No `loom` dep, no `[lints]`, no `check-cfg` | `Cargo.toml`, no `.toml` hits | Must add loom as a `[target.'cfg(loom)'.dependencies]` dep + `build.rs` `rustc-check-cfg` to keep "lib 49 warnings". |
| `.cargo/config.toml` sets `rustflags=["-C","target-cpu=native"]`; **env `RUSTFLAGS` overrides it** | `.cargo/config.toml:9,17,24` | The loom run MUST pass `RUSTFLAGS="--cfg loom -C target-cpu=native"` or gxhash's AES/SSE2 break. |
| Gate (HEAD-current, prompt): slab 4329/0, index 4172/0, conf 483/0 ~840 cyc, lib 49 both, oracle 0 | prompt + `phase-b-plan.md:212` (plan shows pre-B1.c 4324/4167) | Honor the prompt's HEAD numbers. |
| rustc 1.97.0-nightly | `rustc --version` | `cargo::rustc-check-cfg` syntax (double-colon) supported; loom `UnsafeCell`/atomics are drop-in. |

**Critical reframing I am honoring:** B2 makes the `IndexArena` *interior* `&self`/lock-free-capable and proves it with loom in isolation. The runtime stays serialized by `IndexHeap`'s `.write()`, so B2 is **byte-identical at runtime**. There is **no FANOUT>0 concurrent-ASAN gate for B2** (the plan-of-record at `phase-b-plan.md:219-226` lists one, but that is the *original combined* B2.1–B2.3 plan; the prompt's reframing supersedes it: at FANOUT>0 the IndexHeap lock still serializes arena access, so it cannot exercise concurrent arena allocs — that validation is deferred to D). B2's gate = byte-identical green wall + loom + ASAN at **FANOUT=0**.

---

## 1. New `Segment<N>`

### 1.1 Field layout (replaces `index_arena.rs:122-134`)

```rust
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
```

Import change at `index_arena.rs:36`:
```rust
use std::cell::UnsafeCell;
use std::hint;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
```

### 1.2 `Segment::new` (replaces `index_arena.rs:137-151`)

`UnsafeCell<MaybeUninit<N>>` is **not `Clone`**, so `vec![…; capacity]` is illegal; build the boxed slice from an iterator. Same for `marks` (`AtomicU64` is not `Clone`).

```rust
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
```

> Note: `Box<[T]>: FromIterator<T>` (via `collect`) is stable — it collects into a `Vec` then `into_boxed_slice`, preallocating exactly `capacity`. This matches the "preallocate" constraint.

### 1.3 `is_full` (replaces `index_arena.rs:153-156`)

Fullness is now about claims, not the old `len: usize`. A segment is full when every offset has been **claimed** (`bump >= capacity`):

```rust
    #[inline]
    fn is_full(&self) -> bool {
        self.bump.load(Ordering::Relaxed) >= self.capacity
    }
```

> Justification: in the single-bumper runtime, after the last successful `bump_one` returns `capacity-1`, the next `bump_one` does `fetch_add`→`capacity` and returns `None`; callers must treat "claim cursor exhausted" as full. Using `bump` (not `len`) avoids a window where `bump==capacity` but `len<capacity` mid-publish would wrongly report not-full and re-hand a claimed segment. In the serialized runtime `len==bump` always holds at observation points, so this is byte-identical; under concurrency it is the correct conservative test.

### 1.4 `set_mark` / `is_marked` / `clear_marks` (replace `index_arena.rs:161-179`) — Ordering UNCHANGED (Relaxed; B3 flips)

```rust
    /// Set the mark bit for `offset`. Returns `true` if previously unmarked.
    /// Ordering stays `Relaxed` in B2 (B3 introduces Release/Acquire mark
    /// ordering for the concurrent marker).
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
        for w in self.marks.iter() {
            w.store(0, Ordering::Relaxed);
        }
    }
```

> Only change vs current: `for w in &self.marks` → `for w in self.marks.iter()` (a `Box<[_]>` iterates the same; `&self.marks` would iterate `&Box<[_]>` which also derefs — either compiles, but `.iter()` is explicit). Bit logic identical to lines 162-178.

### 1.5 `is_fully_dead` (replaces `index_arena.rs:189-205`) — preserve B1.b word-parallel form, read `len.load(Acquire)`

```rust
    /// `true` if no slot in `0..len` is marked (B1.b word-parallel form preserved).
    ///
    /// Reads the PUBLISH cursor with `Acquire` so that, paired with the
    /// publisher's `Release`, every published slot's mark word is observed. OR
    /// together the complete mark words covering `0..len` (one load per 64 slots),
    /// masking the final partial word to the valid low `len & 63` bits (offsets
    /// `>= len` are never set by `set_mark`, so the mask only guards padding bits).
    fn is_fully_dead(&self) -> bool {
        let len = self.len.load(Ordering::Acquire);
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
```

> The only change from lines 190-204 is `let len = self.len.load(Ordering::Acquire);` (was `let len = self.len;`). The word-parallel structure is byte-identical. Since B2 calls this only from `sweep_with` at quiescence, `Acquire` is conservative-correct (and forward-compatible with a concurrent reader).

### 1.6 `node_at` — the new unsafe reader (NEW method)

```rust
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
```

### 1.7 The claim+write+publish path (NEW methods on `Segment`)

```rust
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
```

### 1.8 `release` — STAYS `&mut self` (replaces `index_arena.rs:208-214`)

**Decision: `release` stays `&mut self`.** Justification: `release` runs only inside `sweep_with`, which is `&mut self` and only at quiescence (the collector gate is closed whenever workers exist — `phase-b-plan.md:166-170`). With `&mut self` the method has exclusive access, so dropping `nodes` and zeroing the cursors needs no atomics-as-store and cannot race. The `AtomicBool released` is set with a plain `store(Relaxed)` (a `&mut`-exclusive store; the `Relaxed` is harmless and forward-compatible). Making `release` `&self` would buy nothing in B2 (it never runs concurrently) and would *weaken* the type-level guarantee that storage teardown is exclusive — exactly the property D will want to reason about carefully. So keep it `&mut self`.

```rust
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
```

> `freed` now uses `self.capacity * size_of::<N>()` instead of `self.nodes.capacity()` (a `Box<[T]>` has no spare-capacity notion; its length *is* the capacity). For a non-released segment `committed_node_bytes` likewise uses `capacity` (§2.10). This is byte-identical to today for the common case where the old `Vec` was filled to `capacity` before release — and **more** accurate for partially-filled current segments (which today report `nodes.capacity()`, i.e. the `Vec::with_capacity(capacity)` reservation = `capacity` anyway). So the watermark number is unchanged.

`Box::new([])` is the canonical empty boxed slice (zero-allocation; `[]: [UnsafeCell<MaybeUninit<N>>; 0]` coerces to `Box<[_]>`).

---

## 2. New `IndexArena<N>`

### 2.1 Field layout (replaces `index_arena.rs:237-249`)

```rust
/// A segmented, index-addressed, non-moving value arena.
///
/// **B2 interior:** the node-slot allocation path (`alloc`/`bump_in`/
/// `ensure_bump_room`/`open_segment`) is `&self` and lock-free-capable — fresh
/// bump claims a unique offset via an atomic cursor and publishes it with
/// `Release`, so a concurrent marker reading the published prefix sees only
/// fully-written nodes. The segment *directory* is a never-realloc
/// `Box<[UnsafeCell<MaybeUninit<Box<Segment>>>]>` of `MAX_SEGMENTS` cells
/// (allocated once), published by a monotone `seg_count`; readers index only
/// `< seg_count.load(Acquire)`. Free-list reuse and `sweep` stay `&mut self` at
/// quiescence (the only producer of free slots is `sweep`, gated closed while
/// workers exist) — that is what keeps reuse ABA-free. See the type-level SAFETY
/// block ([`unsafe impl Sync`]).
///
/// In the current runtime the wrapping `IndexHeap` still serializes every access
/// behind its `RwLock` write lock, so `bump.fetch_add` is called by one thread at
/// a time and `Addr` assignment is identical to the pre-B2 `len`-bump — i.e. B2
/// is byte-identical at runtime. The `&self` interior is the substrate B3 (TLABs)
/// and the D-phase parallel collector build on.
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
```

Add to the import block:
```rust
use std::sync::Mutex;
```

> **Why `dir_lock: Mutex<()>` (justified, not repurposed):** the directory is a 16384-cell array published by `seg_count`. The *fast* path (bump into the current segment) never touches it. The *slow* path (`open_segment`) must (a) pick the next index = `seg_count`, (b) write that cell, (c) `seg_count.store(idx+1, Release)`, (d) `cur_seg.store(idx, Release)`. If two threads opened concurrently they could pick the same index or publish out of order. A `Mutex<()>` around (a)-(d) is the minimal serialization; it is uncontended in the single-bumper runtime (taken once per 262144 allocations) and is the structure D will keep. I do **not** repurpose an existing lock — there is none at arena level (`IndexHeap` owns the `RwLock`, a layer up). `parking_lot` is not a dep; `std::sync::Mutex` is correct and `Send+Sync`.

> **Why `free_list` stays `Vec<Addr>` under `&mut self` (justified):** `phase-b-plan.md:164-170` and the TLA+ `NoConcurrentFree` invariant require that the *only* producer of free slots is `sweep` at quiescence, and concurrent `&self` alloc claims **only** fresh bump space (never pops the free list). So `free_list` is touched exclusively under `&mut self` (sweep rebuild + the quiescent free-list `alloc` path) and never observed by a concurrent `&self` reader/bumper. A plain `Vec` is therefore sound and needs no atomics — converting it to a concurrent structure would be wasted work that D's design has not yet specified.

### 2.2 The free-list / `&self`-alloc decision (the prompt's explicit DECIDE)

**Decision: keep a single `alloc` but make it `&mut self`, and add a separate `&self` fresh-bump method.** Concretely:

- **`alloc(&mut self, node) -> Addr`** — retains *exactly* today's semantics: free-list pop OR fresh bump. It is the quiescent / serialized path. `IndexHeap::alloc_fixed` (`index_heap.rs:138-142`) calls `self.arena.alloc(node)` and `IndexHeap` is always behind the write lock, so `alloc` having `&mut self` costs nothing at runtime and **preserves byte-identical free-list reuse** (the `sweep_reclaims_unmarked_and_reuses_slots` test at `index_arena.rs:664-693` depends on `alloc` reusing freed slots).
- **`alloc_bump(&self, node) -> Addr`** — NEW: fresh-bump-only, `&self`, lock-free-capable. It never touches the free list. This is the method a future concurrent producer (B3 TLAB / D worker) calls. In B2 it is exercised **only by the loom model** (and is available but unused by the serialized runtime).
- **`bump_in(&self, seg, node)`** and **`ensure_bump_room(&self)`** — go `&self` (the prompt requires this; `IndexHeap::alloc_sexpr`/`alloc_atom`/etc. at `index_heap.rs:159-248` call them, all under the write lock, so byte-identical).

This is the cleanest split: it satisfies "make `alloc`/`bump_in`/`open_segment` `&self`" *for the paths that must be concurrent* (`bump_in`, `ensure_bump_room`, `open_segment`, `alloc_bump`) while keeping the **free-list reuse** path (`alloc`) `&mut self` exactly as the TLA+ `NoConcurrentFree` premise requires. `alloc` *internally* calls the same `&self` `open_segment`/bump primitives, so there is one bump implementation.

> Rationale vs the alternative ("keep `alloc` `&self`, make free-list reuse a separate quiescent `&mut` method"): that alternative changes `IndexHeap::alloc_fixed`'s reuse semantics (it would have to choose between two methods), perturbing the free-list reuse the existing tests pin. Keeping `alloc` `&mut self` is the **smaller, byte-identical** change and is exactly what `phase-b-plan.md:165-167` prescribes ("Free-list reuse stays `&mut self`… concurrent alloc claims ONLY fresh bump space"). The `&self` capability the gate demands is delivered by `bump_in`/`ensure_bump_room`/`open_segment`/`alloc_bump`, which is what `IndexHeap`'s variable-length allocators already funnel through.

### 2.3 `new` / `with_segment_capacity` (replace `index_arena.rs:251-279`) — allocate the 16384-cell directory once

```rust
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
```

> `arena.open_segment()` is now `&self` (§2.4), so it can be called on the non-`mut` binding `arena`. (Today it required `&mut`; the binding was `let mut arena`.) The `let mut` is dropped — `arena` is moved out by value.

### 2.4 `open_segment(&self) -> usize` (replaces `index_arena.rs:282-291`)

```rust
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
        assert!(idx < MAX_SEGMENTS, "arena exhausted: {MAX_SEGMENTS} segments");
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
```

### 2.5 `segment(&self, i) -> &Segment<N>` — the unsafe raw-pointer reader (NEW)

```rust
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
```

> `(*cell).assume_init_ref()` yields `&Box<Segment<N>>`; the `-> &Segment<N>` return type triggers a `Deref` coercion to `&Segment<N>`. The lifetime of the returned `&Segment` is tied to the elided `&self` lifetime of `segment()`, **not** to a borrow of the `segments` field — because it is produced from a raw pointer. That is the crux that makes §4 compile.

### 2.6 `alloc(&mut self, node) -> Addr` (replaces `index_arena.rs:295-315`) — free-list path stays, `&mut self`

```rust
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
```

> **Determinism / byte-identity (the prompt's item 5):** In the serialized runtime, `IndexHeap` holds the `RwLock` write lock across the whole call, so exactly one thread is in `alloc`/`alloc_bump`/`bump_in` at a time. `bump.fetch_add(1, Relaxed)` therefore returns `0,1,2,…` in the same sequence the old `seg.len` bump produced (`off = seg.len; seg.len += 1`). The free-list `pop()` order is unchanged (still a `Vec` LIFO rebuilt identically by `sweep_with`). So **every `Addr` is assigned identically to pre-B2 → byte-identical**. The `compare_exchange_weak` in `publish` always succeeds first try because `len == off` already (single bumper), so it is equivalent to the old `seg.len += 1`. **Stated explicitly: with callers single-threaded, the two-cursor protocol degenerates to the pre-B2 monotone bump and produces identical addresses and identical observable behavior.**

### 2.7 `get(&self)` / `get_mut(&mut self)` (replace `index_arena.rs:318-335`)

```rust
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
```

> **`get_mut` survives as `&mut self`** — verified: no external caller (crate-wide grep found none in `index_heap.rs` or elsewhere; the only use is `index_arena.rs:813` in `transitive_mark_terminates_on_cycle`). It needs no signature change. The body changes from `&mut seg.nodes[off]` to the `assume_init_mut` form because the storage is now `MaybeUninit`.

### 2.8 `mark` / `is_marked` (replace `index_arena.rs:339-347`)

```rust
    /// Mark `addr` live. Returns `true` if newly marked. `&self` (already was) —
    /// `set_mark` is an atomic `fetch_or`.
    #[inline]
    pub fn mark(&self, addr: Addr) -> bool {
        // SAFETY: `addr.segment() < seg_count` (it names a live, published slot).
        unsafe { self.segment(addr.segment()) }.set_mark(addr.offset())
    }

    #[inline]
    pub fn is_marked(&self, addr: Addr) -> bool {
        // SAFETY: as `mark`.
        unsafe { self.segment(addr.segment()) }.is_marked(addr.offset())
    }
```

> `mark`/`is_marked`/`mark_from_roots`/`mark_from_roots_with` keep their existing `&self` signatures (`index_arena.rs:339,345,503,575`). Only the field-access changes to `self.segment(i)`. `IndexHeap::mark` (`index_heap.rs:383`) and `IndexHeap::sweep`'s retain closure (`index_heap.rs:422`) call `arena.is_marked`/`arena.mark` through `&self.arena` — unaffected.

### 2.9 `segment_count` / `alloc_count` / `free_slots` (replace `index_arena.rs:352-366`)

```rust
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

    /// Number of slots currently on the free list (quiescence-only field).
    #[inline]
    pub fn free_slots(&self) -> usize {
        self.free_list.len()
    }
```

> `IndexHeap::sync_sides` (`index_heap.rs:130`) loops `while self.sides.len() < self.arena.segment_count()` — `segment_count` returning `seg_count.load(Acquire)` gives the same value the old `self.segments.len()` did. `IndexHeap::alloc_count` (`index_heap.rs:436`) and `segment_count` (`index_heap.rs:441`) delegate — unaffected.

### 2.10 `committed_node_bytes` / `live_node_count` / `node_size_bytes` (replace `index_arena.rs:376-407`)

These iterated `for seg in &self.segments`. Now they must iterate the **published** directory `0..seg_count` via the raw-pointer `segment()`:

```rust
    /// Bytes of node-slot storage currently committed (Σ over non-released
    /// published segments of `capacity * size_of::<N>()`). Monotone between
    /// sweeps; drops when a fully-dead segment is released.
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

    /// Number of node slots ever bump-published across all non-released published
    /// segments (Σ of per-segment publish cursors). Used to recompute the GC
    /// watermark after a sweep.
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

    /// Per-node byte size (`size_of::<N>()`).
    #[inline]
    pub fn node_size_bytes(&self) -> usize {
        std::mem::size_of::<N>()
    }
```

> `committed_node_bytes` now uses `seg.capacity` instead of `seg.nodes.capacity()`. **Byte-identical:** the old `Segment::new` did `Vec::with_capacity(capacity)`, whose `.capacity()` is exactly `capacity`, and `release` set `nodes = Vec::new()` (capacity 0). The new layout: `capacity` for non-released, and `release` drops storage (we skip released anyway). So the sum is identical. `IndexHeap::committed_bytes`/`live_bytes` (`index_heap.rs:452-468`) delegate — unaffected.

### 2.11 `sweep_with(&mut self)` (replaces `index_arena.rs:422-496`) — rewrite access sites for the directory + raw-pointer reads

This is the borrow-checker-critical method. It (a) reads each segment's `marks`/`len`/`released` via the raw-pointer `segment()` (NOT a `self` borrow), and (b) pushes to `self.free_list` and calls `self.open_segment()`. Because `segment()` returns a `&Segment` derived from a raw pointer (not borrowing `self`), the `self.free_list.push` and `self.segment(si).release()` (which needs `&mut`) interleave correctly — but `release` is `&mut self` on `Segment`, so I take it through a `&mut`-typed raw deref. Full rewrite:

```rust
    pub fn sweep(&mut self) -> SweepStats {
        self.sweep_with(|_| {})
    }

    pub fn sweep_with<F: FnMut(usize)>(&mut self, mut on_release: F) -> SweepStats {
        let mut stats = SweepStats::default();
        // Rebuild the free list from scratch (never persist across cycles).
        self.free_list.clear();

        let seg_count = self.seg_count.load(Ordering::Acquire);
        let cur = self.cur_seg.load(Ordering::Acquire);
        for si in 0..seg_count {
            // Read this segment via the raw-pointer accessor: the resulting
            // `&Segment` does NOT borrow `self`, so the `self.free_list.push`
            // below (and `self.open_segment()` after the loop) do not conflict.
            // SAFETY: si < seg_count ⇒ published; `&mut self` ⇒ quiescence, no
            // concurrent mutator, so reading marks/len and (for release) taking a
            // `&mut Segment` is exclusive.
            let seg: &Segment<N> = unsafe { self.segment(si) };
            if seg.released.load(Ordering::Relaxed) {
                continue;
            }
            let is_current = si == cur;
            let fully_dead = seg.is_fully_dead();

            if fully_dead && !is_current {
                on_release(si);
                // Take a `&mut Segment` through the raw pointer to drop its storage.
                // SAFETY: `&mut self` is exclusive; no other reference to this
                // segment is live across this point (the `seg` shared borrow above
                // is not used after `is_fully_dead`).
                let seg_mut: &mut Segment<N> =
                    unsafe { (*self.segments[si].get()).assume_init_mut() };
                stats.bytes_released += seg_mut.release();
                stats.segments_released += 1;
                continue;
            }

            // Partially-live (or current): reclaim unmarked slots. B1.b
            // word-parallel fast path preserved verbatim — only the slot read
            // changes to `seg.marks[..]` and the length to `seg.len.load(Acquire)`.
            let len = seg.len.load(Ordering::Acquire);
            let full_words = len >> 6;
            let rem = len & 63;
            for wi in 0..full_words {
                let word = seg.marks[wi].load(Ordering::Relaxed);
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
                let word = seg.marks[full_words].load(Ordering::Relaxed);
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
        let cur_released =
            unsafe { self.segment(cur) }.released.load(Ordering::Relaxed);
        if cur_released {
            self.open_segment();
        }
        stats
    }
```

> **Note on the `&Segment` vs `&mut Segment` interleave in the release arm:** I do *not* hold `seg` (shared) across the `seg_mut` (exclusive) creation in a way the compiler rejects — `seg` is last used at `seg.is_fully_dead()`, and `seg_mut` is a *fresh* `assume_init_mut()` from the raw `UnsafeCell::get()`. Both are unsafe raw derefs that the borrow checker does not track as aliasing of `self` (they come from `self.segments[si].get()`, a `*mut`). The one subtlety: `self.segments[si].get()` borrows `self.segments` *immutably* (indexing `&self.segments` to call `.get()`), which coexists with the later `self.free_list.push` (disjoint field) and `self.open_segment()` (`&self`). Field-disjoint borrows of distinct struct fields are allowed; the only thing that would break is borrowing `&self.segments` mutably while `&self.free_list` is live, which never happens.

### 2.12 `mark_from_roots_with` / `bump_in` / `ensure_bump_room` (replace `index_arena.rs:503-560`)

`mark_from_roots_with` (`index_arena.rs:503-528`) and `mark_from_roots` (`index_arena.rs:575-596`) are already `&self` and only call `self.mark`/`self.get` — **no body change beyond what `mark`/`get` already absorb**. Keep verbatim.

`bump_in` and `ensure_bump_room` go `&self`:

```rust
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
```

> **Byte-identity for `bump_in`:** `IndexHeap::alloc_sexpr` etc. do `let seg = self.arena.ensure_bump_room(); self.arena.bump_in(seg, …)` (`index_heap.rs:161-162`, all under the write lock). `ensure_bump_room` returns `cur_seg`; `bump_in` claims `off = bump.fetch_add` which (single-threaded) equals the old `seg.len`. Identical `Addr`. The `assert_eq!(seg, self.current_seg())` preserves the old `assert_eq!(seg, self.cur_seg)` invariant (`index_arena.rs:548-551`). **Critically**, `IndexHeap` calls `ensure_bump_room` *twice* in some paths (`intern_children` then `alloc_sexpr` each call it — `index_heap.rs:161,233`) — both return the same `cur_seg` and neither bumps `bump` (only `bump_in` does), so co-location is preserved exactly.

---

## 3. `unsafe impl Send + Sync` (NEW, after the `impl` blocks)

```rust
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
```

**`Segment<N>` Send/Sync:** `Segment` is a private type, but it is reachable through `IndexArena`'s `UnsafeCell`s, so the auto-trait analysis for `IndexArena` does **not** depend on `Segment: Sync` (the `unsafe impl` on `IndexArena` covers it). However, `Box<Segment<N>>` sits inside `MaybeUninit` inside `UnsafeCell` — auto traits stop at `UnsafeCell` (`!Sync`) regardless. So **no separate `unsafe impl` for `Segment` is required** for the `IndexArena` impls to hold. I add them anyway for clarity and so a future direct use of `Segment` across threads (D-phase) is covered:

```rust
// SAFETY: `Segment<N>`'s interior mutability (the `UnsafeCell` slot cells) is
// disciplined by the same claim/publish protocol as `IndexArena` (see above);
// `marks`/`len`/`bump`/`released` are atomics. Sound for the same N bounds.
unsafe impl<N: Copy + Send> Send for Segment<N> {}
unsafe impl<N: Copy + Send + Sync> Sync for Segment<N> {}
```

> **Why this fixes the compile break the prompt flags:** `static GLOBAL_INDEX_HEAP: OnceLock<RwLock<IndexHeap>>` requires `IndexHeap: Sync` (a `static` must be `Sync`). `IndexHeap` contains `IndexArena<Node>`. Today `IndexArena` is auto-`Sync` (only `Vec`+`usize`). Introducing `UnsafeCell` makes it auto-`!Sync` → `RwLock<IndexHeap>: Sync` fails → the `static` breaks → **compile error in BOTH builds** (the module compiles unconditionally). The `unsafe impl Sync for IndexArena<N>` above restores `IndexArena<Node>: Sync` (since `Node: Send + Sync` — it's `Copy` + the handle `Arc`s are `Send+Sync`), hence `IndexHeap: Sync`, hence the `static` compiles. **This impl is mandatory for B2 to compile at all, in the default slab build too.** I will verify `Node: Send + Sync` holds — `Node` is `Copy` and stores `SpaceHandle`/`MemoHandle` only as `u64` ids (per `index_node.rs:20` "Non-`Copy` handles … represented by their" id), so `Node` is trivially `Send + Sync`.

---

## 4. Borrow-checker resolution (the prompt's item 4) — explicit

The hazard: `sweep_with(&mut self)`'s reclaim loop reads `seg.marks[wi]` *and* pushes to `self.free_list`. If `seg` were `&self.segments[si].marks[..]` (a real borrow of `self`), the `self.free_list.push` would be **E0502** (cannot borrow `self.free_list` as mutable while `self.segments` is borrowed). Resolution, line by line:

1. `let seg: &Segment<N> = unsafe { self.segment(si) };` — `self.segment(si)` is `unsafe fn segment(&self, i) -> &Segment<N>` whose body is `(*self.segments[i].get()).assume_init_ref()`. The expression `self.segments[i].get()` borrows `&self.segments` only for the **duration of the `.get()` call** (it returns a `*mut`, a `Copy` raw pointer that carries no lifetime). The returned `&Segment<N>` has the lifetime of `segment`'s `&self` receiver — i.e. it is tied to `&self` *as a whole*, **not** to the `segments` field specifically, and NLL sees it as a shared borrow of `*self` that ends when `seg` is last used.

2. Because `seg` is a shared (`&`) borrow of `*self`, and `self.free_list.push(..)` needs a mutable borrow of `*self`, these *would* conflict **if `seg` outlived the push**. They don't, because of two-phase / NLL field reasoning: `self.free_list.push` reborrows a **disjoint field**. The compiler permits simultaneous `&self.segments[..]`-derived shared data and `&mut self.free_list` **only when the shared borrow is recognized as borrowing a disjoint place**. Here `seg`'s provenance is a raw pointer, so NLL does not extend a `&self.segments` borrow across the loop body — the shared borrow region is just the `self.segment(si)` call. After that call returns, `seg` is a value of type `&Segment<N>` whose lifetime the borrow checker associates with the anonymous region of `segment`'s receiver; pushing to `self.free_list` (a *different field*) does not invalidate it because field-disjoint mutation is allowed (`self.free_list` and `self.segments` are different fields of the same struct). 

   This is the **identical pattern** the existing `IndexHeap::sweep` already relies on at `index_heap.rs:420-429`: `let arena = &self.arena;` (shared) coexisting with `let sides = &mut self.sides;` (mutable) because they are disjoint fields. My `sweep_with` reproduces that at the `IndexArena` level: `seg` (data behind `self.segments`, accessed via raw ptr) coexists with `&mut self.free_list`.

3. **To make this bullet-proof regardless of NLL's field-sensitivity**, the raw-pointer indirection is the belt-and-suspenders: `self.segment(si)` returns a reference *not derived from a tracked borrow of `self.segments`* (it launders through `*mut`). So even the most conservative borrow-checker treats `seg` as independent of `self`, and `self.free_list.push`/`self.open_segment()` are unconstrained. This is the *whole point* of routing every segment read through `unsafe fn segment(&self, i) -> &Segment` rather than `&self.segments[i]`.

4. **The release arm** (`fully_dead && !is_current`): `let seg_mut = unsafe { (*self.segments[si].get()).assume_init_mut() };` produces `&mut Segment<N>` from the same raw pointer. `seg` (shared) is *not used after `seg.is_fully_dead()`*, so there is no live `&seg` when `seg_mut` is formed — and even if there were, both come from raw pointers, so the borrow checker does not see aliasing. `seg_mut.release()` then needs `&mut Segment`, which it has. (`release` is `&mut self` on `Segment` — §1.8.)

5. **`&mut self` methods that now "conflict":** None newly conflict. `alloc`'s free-list pop path does `let seg = self.segment(addr.segment());` (shared, raw-ptr-derived) then writes through `seg.nodes[..].get()` (a raw write, no `self` borrow) — and reads `self.free_list.pop()` *before* that, so there is no overlap. `get_mut(&mut self)` forms one `&mut N` from a raw ptr — no `self`-field borrow conflict. The only `&mut self` methods are `alloc`, `get_mut`, `sweep`/`sweep_with`; none hold a tracked `&self.segments` borrow across a `&mut self.<otherfield>` use.

---

## 5. Determinism / byte-identity statement (the prompt's item 5)

**Stated for the record, and to be reproduced in the commit message and a code comment on `alloc_bump`:**

> In the B2 runtime, every `IndexArena` access occurs while `IndexHeap` holds its global `RwLock` *write* lock (`global_index_heap().write()`, `index_heap.rs:506-669`). The write lock is exclusive, so at most one thread is inside `alloc`/`alloc_bump`/`bump_in`/`ensure_bump_room`/`open_segment` at any instant. Under that serialization:
> - `bump.fetch_add(1, Relaxed)` returns `0, 1, 2, …` in the same order the pre-B2 `off = seg.len; seg.len += 1` did.
> - `publish`'s `compare_exchange_weak(off, off+1, …)` always finds `len == off` (the single bumper just claimed `off` and all prior offsets are already published), so it succeeds on the first attempt — equivalent to the old `seg.len += 1`.
> - `open_segment` picks `idx = seg_count` and advances it monotonically — same indices as the old `self.segments.len()` push.
> - The free list is the same `Vec<Addr>`, rebuilt by the same word-parallel `sweep_with` in the same push order, popped LIFO by the same `alloc`.
>
> Therefore **every `Addr` is assigned identically to pre-B2, and all observable behavior (hash-cons identity, `inner_ptr` content identity, sweep stats, conformance output) is byte-identical** → the gate (slab 4329/0, index 4172/0, conformance 483/0 ~840 cycles, lib 49 both, oracle 0 panics) is preserved by construction. The atomics and `UnsafeCell` add only the *capability* for concurrency; they do not change single-threaded semantics.

---

## REVIEW VERDICT (applied 2026-05-30, before implementation)
Adversarially reviewed against source — design is SOUND (no soundness gap, unlike B1.c). The unsafe
`impl Send/Sync` is a compile prerequisite (the `static RwLock<IndexHeap>` needs `IndexArena: Sync`); the
two-cursor publish protocol (Release/Acquire + CAS-extend-prefix), the raw-pointer `segment()` borrow
laundering, and the `alloc(&mut)`/`alloc_bump(&self)` free-list split are all correct and byte-identical
at runtime (IndexHeap serializes). **ONE CORRECTION to §6.1 (applied):** the production import block stays
PLAIN `std` (no `#[cfg(loom)]` swap of `UnsafeCell`/atomics/`Mutex`) — the production code uses
`UnsafeCell::get()`, which loom's `UnsafeCell` does NOT provide, so swapping it under `--cfg loom` would
break the production build. The loom imports (`use loom::cell::UnsafeCell; use loom::sync::atomic::*;
use loom::sync::Arc; use loom::thread;`) go LOCAL inside `#[cfg(loom)] mod loom_model`, shadowing super's
std types — which is exactly what §6.2's `LoomSeg` protocol-mirror approach needs (loom instruments the
mirror, not the production type). The `cell_ptr` shim in §6.1 is DROPPED (unneeded). Everything else
implemented as designed.

**APPLICATION DEVIATIONS (2, both correct fixes to errors in the design's literal code):**
1. `sweep_with` E0502 — §2.11/§4's claim that `self.segment(si)` "launders" the borrow was wrong (the
   `&self` lifetime threads into the return). Fixed faithfully to §4's intent: take a `*mut Segment` LOCAL
   from `UnsafeCell::get()` (a raw pointer carries no lifetime), form per-use `&*`/`&mut *` borrows — sound
   under `&mut self` (quiescence), coexists with `&mut self.free_list` (disjoint field/memory).
2. §7 test asserted `d == a` (the LIVE slot — impossible); corrected to `d == b` (the freed slot reused LIFO).

**LOOM RUN-CONFIG (verified-green; the §6.2 debug/weak command does NOT work):** the loom lane requires
`--release` + `LOOM_MAX_PREEMPTIONS=2`, and the model uses STRONG `compare_exchange` + `thread::yield_now()`
(see the code comment on `loom_model`). Scientific ledger: H1 (`yield_now` for spin) — REFUTED (still
overflowed); H2 (weak-CAS spurious-failure spin) — REFUTED (still overflowed); H3 (debug frames overflow
loom's generator coroutine stack — fails on first thread in 0.00s) — CONFIRMED, `--release` fixes it.
B2 gate ALL GREEN: green-wall byte-identity (slab 4330/0, index 4173/0, conf 483/0 @840 cycles, lib 49
both, oracle 0) + ASAN@FANOUT=0 (slab 483/0 0-UAF, index M11-pt 221/0 343-cycles 0-UAF) + loom 1/0.

## 6. The loom model (the prompt's item 6)

### 6.1 The `loom::` vs `std::` cfg aliasing the module needs — ⚠️ SUPERSEDED by the REVIEW VERDICT above

The cleanest pattern, given the *production* code uses `std::sync::atomic` and `std::cell::UnsafeCell` directly: add a private `sync` alias module so the **same** `Segment`/`IndexArena` code compiles against loom's types under `#[cfg(loom)]`. But because B2 must stay byte-identical and minimal, I scope the aliasing tightly: change the production imports to go through a module-local alias, so the `#[cfg(loom)]` build swaps in loom's drop-in replacements.

Replace the import block (§1.1) with:

```rust
// Under `--cfg loom` the loom shims replace std's atomics/UnsafeCell so the loom
// model checker can explore all interleavings of the bump/publish protocol; the
// production build uses std directly (zero overhead). loom's `AtomicUsize`/
// `AtomicU64`/`AtomicBool`/`UnsafeCell` are API-compatible drop-ins, EXCEPT
// `UnsafeCell::get()` is replaced by `with`/`with_mut` closures — so the few
// `.get()` call sites are funneled through helper methods that have a loom arm.
#[cfg(loom)]
use loom::cell::UnsafeCell;
#[cfg(loom)]
use loom::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(loom)]
use loom::sync::Mutex;

#[cfg(not(loom))]
use std::cell::UnsafeCell;
#[cfg(not(loom))]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(not(loom))]
use std::sync::Mutex;

use std::hint;
use std::mem::MaybeUninit;
```

**The one real incompatibility:** loom's `UnsafeCell::get()` does **not** exist; loom uses `cell.with(|ptr| …)` / `cell.with_mut(|ptr| …)` so it can track access. The production `node_at`/`write_claimed`/`segment`/`get_mut` call `self.nodes[off].get()` / `self.segments[i].get()`. To keep ONE body, I introduce two tiny helpers with a loom arm and route all raw-cell access through them:

```rust
// --- cell access shim: std uses `.get()`, loom uses `.with`/`.with_mut` ---
#[cfg(not(loom))]
#[inline]
unsafe fn cell_ptr<T>(c: &UnsafeCell<T>) -> *mut T {
    c.get()
}
// Under loom there is no stable `*mut` outside a `with` closure, so loom builds
// take a *different* method body (below) rather than this raw-pointer escape.
```

Because loom genuinely cannot hand out a `*mut` that escapes `with`, the honest approach for the loom model is: **the loom test does not exercise `node_at`/`segment` (the directory) at all** — it exercises ONE `Segment`'s bump/publish/read protocol directly, using loom-aware access *inside* the test, against a minimal `LoomSlots` mirror of the `Segment` slot machinery. This is exactly what the prompt asks for ("hits `IndexArena` directly from multiple loom threads IN ISOLATION… 2 loom writer threads each `bump_one`+write+publish into one segment"). The protocol primitives (`bump_one`/`publish` + the `len.load(Acquire)` read) are pure-atomic and identical between std and loom; only the cell write/read needs loom's `with_mut`/`with`. So the loom module re-expresses the protocol over a loom `UnsafeCell` directly.

> Rationale for not forcing the production `Segment` through loom: loom's `UnsafeCell::get`-less API would force `with`/`with_mut` closures into the production hot path (a real overhead and a readability cost) purely to satisfy loom. The protocol being proven (claim-unique / write / publish-contiguous / read-published) is a property of the *cursors + cell discipline*, which the loom module reproduces faithfully in ~40 lines. This is the standard loom idiom (model the protocol, not the whole type).

### 6.2 The loom test module (NEW, end of `index_arena.rs`)

```rust
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
// state space):
//   RUSTFLAGS="--cfg loom -C target-cpu=native" \
//     systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400% \
//     cargo test --lib --features index-gc \
//     backend::eval::cesk::index_arena::loom_model -- --nocapture
// (`-C target-cpu=native` is RE-ADDED because setting RUSTFLAGS overrides
// .cargo/config.toml, which would otherwise drop the gxhash AES/SSE2 flags.)
// Optionally bound exploration: LOOM_MAX_PREEMPTIONS=3.
#[cfg(loom)]
mod loom_model {
    use super::*;
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
                slots: (0..cap).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect(),
                len: AtomicUsize::new(0),
                bump: AtomicUsize::new(0),
                cap,
            }
        }
        fn bump_one(&self) -> Option<usize> {
            let off = self.bump.fetch_add(1, Ordering::Relaxed);
            if off >= self.cap { None } else { Some(off) }
        }
        // claim+write+publish for a single value; returns the claimed offset.
        fn claim_write_publish(&self, value: usize) -> Option<usize> {
            let off = self.bump_one()?;
            // exclusive: `off` uniquely claimed.
            self.slots[off].with_mut(|p| unsafe { (*p).write(value) });
            // publish contiguously.
            while self
                .len
                .compare_exchange_weak(off, off + 1, Ordering::Release, Ordering::Relaxed)
                .is_err()
            {
                loom::hint::spin_loop();
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
                        assert!(v == 0xA0 || v == 0xB0, "slot {off} read torn/uninit value {v:#x}");
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
            assert!(seen[0] && seen[1], "both tags present in the contiguous prefix");
        });
    }
}
```

> **Why `LoomSeg` and not the real `Segment`:** loom's `UnsafeCell` has no `.get()` → `*mut` escape; access must be inside `with`/`with_mut` closures (so loom can instrument it). The production `Segment::node_at`/`write_claimed` return/take raw references that escape, which is exactly what loom forbids. `LoomSeg` reproduces the *identical* cursor protocol (`bump.fetch_add(Relaxed)` claim, `len.CAS(Release)` publish, `len.load(Acquire)` read) over loom-instrumented cells, so loom proves the protocol. This is the canonical loom usage; the prompt explicitly notes "loom's `AtomicUsize`/`UnsafeCell` are drop-in… show the `loom::` vs `std::` cfg aliasing" — the aliasing is the `#[cfg(loom)] use loom::… / #[cfg(not(loom))] use std::…` block in §6.1, and the model lives in the `#[cfg(loom)]` module.

### 6.3 Cargo.toml + build.rs additions for loom

`Cargo.toml`, after `[dev-dependencies]` (loom is only a *test*-cfg dep, gated so it never enters the normal build graph and never perturbs the 49-warning count):

```toml
# loom — concurrency permutation model checker for the B2 IndexArena bump/publish
# protocol (src/backend/eval/cesk/index_arena.rs). Compiled ONLY under `--cfg loom`
# (a dedicated, capped CI lane), so it is absent from every normal build/test.
[target.'cfg(loom)'.dependencies]
loom = "0.7"
```

`build.rs` `main()` (append after line 11, before the closing brace at line 12) — declare the custom cfg so rustc 1.97-nightly does NOT emit an `unexpected_cfgs` warning (which would break the "lib 49 warnings BOTH builds" gate):

```rust
    // Declare the `loom` custom cfg so `#[cfg(loom)]` in index_arena.rs does not
    // trip the `unexpected_cfgs` lint (Rust ≥1.80). Without this, the lint adds a
    // warning to every build → breaks the lib-49-warnings green-wall gate.
    println!("cargo::rustc-check-cfg=cfg(loom)");
```

> I verified there is **no** existing `[lints]`/`check-cfg`/`unexpected_cfgs` config in the repo, so this is the canonical place. The `cargo::` (double-colon) form is the current syntax supported by the 1.97 toolchain.

---

## 7. Non-loom unit test adjustments (the prompt's item 7)

I audited all 19 existing tests (`index_arena.rs:603-976`). They exercise the **public API** (`alloc`, `get`, `get_mut`, `mark`, `sweep`, `sweep_with`, `bump_in`, `ensure_bump_room`, `segment_count`, `free_slots`, `alloc_count`, `mark_from_roots*`). Because the public API signatures are preserved **except** `bump_in`/`ensure_bump_room` going `&self` (which is *more* permissive — a `&mut`-bound `arena` can still call `&self` methods), **almost all tests compile and pass unchanged.** Specific findings:

| Test (line) | Verdict |
|---|---|
| `addr_pack_unpack_roundtrip` (604) | Unchanged — `Addr` untouched. |
| `alloc_get_within_one_segment` (618) | Unchanged. `alloc` still `&mut self`, ascending offsets identical. |
| `alloc_spans_segments_with_stable_addresses` (634) | Unchanged. Stable addresses now even stronger (boxed slice never reallocs). |
| `mark_bits_set_test_clear` (652) | Unchanged. |
| `sweep_reclaims_unmarked_and_reuses_slots` (664) | Unchanged. Free-list LIFO reuse preserved (the `alloc` `&mut` pop path). The assert `[dead1, dead2].contains(&r1)` holds — same push order. |
| `sweep_releases_fully_dead_segments` (696) | Unchanged. `bytes_released > 0`: `release` returns `capacity * size_of` = `4*8 = 32 > 0`. ✓ |
| `current_segment_never_released_even_if_dead` (721) | Unchanged. |
| `transitive_mark_then_sweep_reclaims_only_unreachable` (760) | Unchanged. |
| `transitive_mark_visits_shared_substructure_once` (794) | Unchanged. |
| `transitive_mark_terminates_on_cycle` (808) | **`*arena.get_mut(a) = TestNode::One(b);`** — `get_mut` stays `&mut self`, body now `assume_init_mut`. The slot was published by `alloc`, so `assume_init_mut` is sound. **Compiles + passes unchanged.** |
| `mark_from_roots_with_sources_children_from_closure` (821) | Unchanged. |
| `ensure_bump_room_and_bump_in_co_locate` (844) | **`ensure_bump_room`/`bump_in` now `&self`** — the test binds `let mut arena` and calls them; `&self` methods are callable on a `&mut` place. Offsets/segment transitions identical. **Compiles + passes unchanged.** |
| `sweep_with_reports_released_segment_indices` (862) | Unchanged. |
| B1.b tests (886, 914, 943, 958) | **All four unchanged** — they drive `alloc`/`mark`/`sweep`/`get`/`free_slots`/`segment_count` and assert exact stats. The word-parallel reclaim loop is preserved verbatim (only `seg.marks[wi]` access path changed, same values), `is_fully_dead` preserved (only `len` source changed), and the partial-word release-prevention (test 958) still works because `set_mark(99)` sets the bit and `is_fully_dead`'s mask preserves it. **Byte-identical stats.** |

**Conclusion: ZERO existing tests need source changes.** Every signature change is widening (`&mut`→`&self`) or internal (body uses `MaybeUninit`), and the observable behavior (addresses, stats, reuse order) is byte-identical. This is the strongest possible evidence the green wall holds.

**One NEW non-loom unit test** to pin the B2-specific `&self` `alloc_bump` path (so it is covered in *both* builds without loom):

```rust
    #[test]
    fn alloc_bump_fresh_only_skips_free_list() {
        // `alloc_bump` (&self) must NEVER consume the free list — it claims fresh
        // bump space only. Sweep frees a slot; a following `alloc_bump` bumps past
        // it (free_slots stays nonzero), whereas `alloc` (&mut) would reuse it.
        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(64);
        let a = arena.alloc(1);
        let _b = arena.alloc(2);
        arena.mark(a);
        let stats = arena.sweep();
        assert_eq!(stats.reclaimed_to_free_list, 1, "the unmarked slot is freed");
        assert_eq!(arena.free_slots(), 1);
        // `&self` fresh bump: does not pop the free list.
        let c = arena.alloc_bump(3);
        assert_eq!(arena.free_slots(), 1, "alloc_bump leaves the free list intact");
        assert_ne!(c, a, "alloc_bump did not reuse the freed slot");
        assert_eq!(*arena.get(c), 3);
        // And `alloc` (&mut) still reuses, proving the two paths differ as designed.
        let d = arena.alloc(4);
        assert_eq!(arena.free_slots(), 0, "alloc reuses the freed slot");
        assert_eq!(d, a, "alloc reused the freed slot LIFO");
    }
```

> This test passes in both builds (it never uses loom). It pins the §2.2 decision (the free-list/`&self` split) so a regression that accidentally routes `alloc_bump` through the free list is caught.

---

## 8. Commit split (the prompt's item 8)

**Recommendation: ONE commit.** Justification:

1. **The plan-of-record's B2.1/B2.2/B2.3 sub-rungs are about an *original* combined effort that included a FANOUT>0 concurrency-flip (B2.3).** The prompt's reframing **removes** the runtime concurrency flip from B2 (it's deferred to D), collapsing B2 to: "directory + two-cursor interior + `unsafe impl Sync` + loom." 

2. **Throwaway analysis:** The plan (`phase-b-plan.md:104-124`) suggests landing B2.1 (directory, `alloc` still `&mut`) separately. But B2.1's `&mut self` `open_segment`/`alloc` are **throwaway** — D needs the `UnsafeCell` directory with the `&self` `open_segment` anyway, and the prompt mandates the `&self` interior *now*. Splitting "structure (B2.1, `&mut`) then protocol (B2.2, `&self`)" would mean writing an intermediate `&mut self` `open_segment` that B2.2 immediately rewrites to `&self` + `dir_lock` — pure churn. **Any non-final directory shape is throwaway since the final (D-ready) shape is known.** So land the final shape directly.

3. **The `unsafe impl Sync` is a compile *prerequisite* of the `UnsafeCell` directory** (without it the `static RwLock<IndexHeap>` breaks). So "structure" and "Sync" cannot be separate commits — the structure commit wouldn't compile. That fuses B2.1+B2.2+the impl into one atomic, compiling unit.

4. **The loom test is `#[cfg(loom)]`** — it does not affect the default/index builds at all, so bundling it adds zero risk to the green-wall commit. The `build.rs` `check-cfg` line and the `[target.'cfg(loom)'.dependencies]` are tiny and inert in normal builds. There is no value in a separate "loom" commit; the loom model *is* the protocol proof for the code in the same commit, so they belong together (review them as a unit).

**Therefore: one commit.** Suggested message (mirroring the plan's mandated wording at `phase-b-plan.md:186`):

```
perf(cesk-gc): B2 — lock-free-capable IndexArena interior (UnsafeCell directory +
two-cursor bump/publish; &self alloc_bump/bump_in/ensure_bump_room/open_segment;
unsafe impl Send/Sync) + loom protocol model

IndexArena's node-slot path is now &self / lock-free-CAPABLE: a never-realloc
Box<[UnsafeCell<MaybeUninit<Box<Segment>>>]> directory published by an atomic
seg_count, and a per-segment two-cursor protocol (bump=claim, len=publish) with
Release/Acquire ordering. alloc (free-list reuse) and sweep stay &mut self at
quiescence — the only producer of free slots is sweep, gated closed while workers
exist, so reuse is ABA-free (TLA+ NoConcurrentFree). A loom model (#[cfg(loom)])
proves the claim/publish/read happens-before: 2 writers + 1 reader, asserting
unique claim, no torn/uninit read, contiguous len.

Byte-identical at runtime: IndexHeap still serializes every arena access behind
its RwLock write lock, so bump.fetch_add is single-threaded and Addr assignment
is identical to pre-B2. IndexHeap side-arena append (sides/space_table/memo_table/
hash_cons) remains lock-guarded pending D.

Gate: slab nextest 4329/0, index 4172/0, conformance 483/0 (~840 cycles), lib 49
both builds, debug oracle 0 panics; ASAN at FANOUT=0 (no single-threaded
regression); loom model green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

## 9. Risks specific to this code (the prompt's item 9)

| # | Risk | Mitigation in this design | Residual |
|---|---|---|---|
| **R1** | **Uninit read** — reader dereferences a slot before its `MaybeUninit::write`. | `node_at`/`get` read only `off < len.load(Acquire)`; publish is `Release` *after* write. `get` debug-asserts `offset < len`. loom invariant (ii) proves it. | None under the protocol; debug assert catches API misuse. |
| **R2** | **Torn node** — reader sees a half-written `N`. | The `write` happens-before the `Release` publish; an `Acquire`-load observing `len>off` sees the whole write. `N: Copy` (`Node`, 24 B) is written by one `MaybeUninit::write` (a single non-atomic store the Release fences). loom invariant (ii). | None under the protocol. |
| **R3** | **Non-contiguous `len`** — `[0,len)` has a hole. | `publish` CAS spins until `len==off`, so `len` advances strictly `0→1→2…`; a higher-offset claimant waits for lower ones. `is_fully_dead`/`sweep`/`live_node_count` iterate `[0,len)` and stay correct. loom invariant (iii). In the single-bumper runtime there is never a hole (CAS first-try). | None. (A future multi-bumper segment relies on the CAS form — implemented.) |
| **R4** | **ABA via free-list** — a slot is reused concurrently with a claim. | Free-list reuse is `&mut self`, quiescence-only; concurrent `&self` `alloc_bump`/`bump_in` claim **fresh** bump only, never pop. `&mut` is statically exclusive of `&self`. TLA+ `NoConcurrentFree` premise. New test `alloc_bump_fresh_only_skips_free_list` pins it. | None — the only producer of free slots (sweep) can't run while workers exist. |
| **R5** | **`Sync` unsoundness** — the `unsafe impl` is wrong. | SAFETY block ties soundness to the four protocol clauses (i)-(iv); loom mechanically checks (i)-(iii); (iv) is a `&mut`/`&self` static-exclusion argument. Bounds are `N: Send`(Send)/`N: Send+Sync`(Sync) — conservative. `Node: Send+Sync` (Copy, handle-ids only). | Low — loom + the static argument cover it; D will re-verify when the IndexHeap spine joins. |
| **R6** | **Directory cost** — 16384 cells × 8 B = **128 KiB per arena**, eagerly allocated in `with_segment_capacity`, even for a 2-slot test arena. | See quantification below. | **Accepted.** |
| **R7** | **`open_segment` under `dir_lock` while a reader spins** — a writer in `open_segment` holds `dir_lock`; concurrent `alloc_bump` losers loop on `open_segment`. | `dir_lock` is taken only on the rare grow path (once per `capacity` allocs = once per 262144 in prod). A loser re-reads `cur_seg` (Acquire) and retries the fast bump — it does not also block on `dir_lock` unless it too needs to grow. No deadlock (single lock, no nesting; `dir_lock` never taken while holding the IndexHeap `RwLock`'s internal lock — they're independent). | None at runtime (single-threaded); benign spin under future concurrency. |
| **R8** | **`Mutex` poisoning** — a panic while holding `dir_lock` poisons it. | `open_segment` does only infallible work under the lock (a `Box::new` + atomic stores); the only panic source is OOM in `Box::new`, which aborts anyway. `.expect("…dir_lock poisoned")` per the "prefer expect" constraint. | Negligible. |

### R6 quantification (the prompt asks: is 128 KiB/arena acceptable? quantify)

- **Per arena:** `MAX_SEGMENTS = 16384` cells; each cell is `UnsafeCell<MaybeUninit<Box<Segment<N>>>>` = one pointer = **8 bytes** ⇒ **131,072 bytes = 128 KiB**, allocated once in `with_segment_capacity`, untouched (uninit) for unused indices.
- **Today's cost:** `segments: Vec::new()` then `open_segment` pushes one `Segment` ⇒ a `Vec` with capacity ~1-4 (amortized growth), i.e. ~8-32 B of spine. So B2 adds **~128 KiB per live arena**.
- **Runtime blast radius:** there is exactly **ONE** runtime arena (the `static GLOBAL_INDEX_HEAP`), and only in the `--features index-gc` build (in the default slab build it is never *constructed* — the `OnceLock` stays empty). So **production cost = 128 KiB, once.** Against the production segment storage (262144 slots × 24 B = **6 MiB per segment**), 128 KiB is **2% of a single segment** — negligible.
- **Test blast radius:** the directory is allocated by every `IndexArena::with_segment_capacity`/`new` call. Verified call sites: **13 in `index_arena.rs` tests + 7 in `index_heap.rs` tests + the global init = ~21 arenas**, each constructed and dropped within one `#[test]`. They are not simultaneous (nextest runs tests in separate threads but each arena is short-lived), so peak extra RSS in the test binary is bounded by `concurrency × 128 KiB`. With nextest default concurrency (≤ num-cpus, say 16-32) that is **≤ ~4 MiB transient** — trivially within the `MemoryMax` caps, and dwarfed by the conformance suite's working set.
- **The `MaybeUninit::uninit()` cells are not zeroed or touched**, so the 128 KiB is a single `malloc` of untouched pages — most stay un-faulted (the OS lazily backs them), so the *resident* cost is far below 128 KiB until segments are actually opened (one page, 4 KiB, holds 512 cells).

**Verdict: ACCEPTED.** 128 KiB virtual / ~one page resident per arena, one runtime arena in the index build only, ~4 MiB transient peak in tests. This is the deliberate space-for-stability trade the never-realloc directory buys (the property that makes `&self` directory reads sound). If it ever mattered, the directory could be lazily chunked (D-phase), but it does not matter at B2's scale.

---

## Implementation sequencing (for when you apply this)

1. `Cargo.toml`: add `[target.'cfg(loom)'.dependencies] loom = "0.7"`.
2. `build.rs`: add `println!("cargo::rustc-check-cfg=cfg(loom)");` in `main()`.
3. `index_arena.rs`: replace the import block (§6.1 cfg-aliased form + `MaybeUninit`/`hint`), `Segment` struct (§1.1) + impl (§1.2-1.8), `IndexArena` struct (§2.1) + impl (§2.3-2.12), add `unsafe impl Send/Sync` (§3), add the new unit test (§7), add the `#[cfg(loom)] mod loom_model` (§6.2). Leave `Addr`, `ArenaNode`, `SweepStats`, `mark_from_roots`, `mark_from_roots_with` bodies unchanged.
4. Verify both builds compile, run the green wall capped, then run the loom lane capped.

### Verification commands (capped, FOREGROUND — per the hard constraint)

```bash
# Both-build compile + green wall (the byte-identical gate):
systemd-run --user --scope -p MemoryMax=48G -p MemorySwapMax=0 -p CPUQuota=1800% \
  scripts/a5_greenwall.sh B2 --with-oracle

# ASAN at FANOUT=0 (no single-threaded regression — NOT FANOUT>0):
METTATRON_PARALLEL_FANOUT_DEPTH=0 \
systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p TasksMax=256 \
  scripts/a5_asan_both.sh

# loom protocol model (RUSTFLAGS re-adds target-cpu=native — it overrides .cargo/config.toml):
RUSTFLAGS="--cfg loom -C target-cpu=native" \
systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400% \
  cargo test --lib --features index-gc \
  backend::eval::cesk::index_arena::loom_model -- --nocapture
```

---

### Critical Files for Implementation
- /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/eval/cesk/index_arena.rs  (the total rewrite — every struct/method/test/loom block above lands here)
- /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/eval/cesk/index_heap.rs  (the caller; do NOT edit — verified its `mark`/`sweep` borrow-split and write-lock serialization are the byte-identity guarantee; lines 75, 159-248, 380-430, 476-481)
- /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/Cargo.toml  (add `[target.'cfg(loom)'.dependencies] loom = "0.7"`; `[features] index-gc` at line 280 unchanged)
- /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/build.rs  (add `cargo::rustc-check-cfg=cfg(loom)` in `main()` to preserve the lib-49-warnings gate)
- /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/eval/cesk/index_node.rs  (defines `Node: Copy` + `ChildRef`/`ByteRef`/`SpanRef`; confirms `Node: Send + Sync`, which the `unsafe impl Sync` relies on)
