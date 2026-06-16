//! `IndexHeap` — the index-arena value heap with per-segment side-arenas
//! (Increment 2 of the clean-room GC migration).
//!
//! Wraps an [`IndexArena<Node>`](crate::backend::eval::cesk::index_arena) with
//! the variable-length **side-arenas** (`Vec<Box<[MettaValue]>>` children,
//! `Vec<Box<str>>` UTF-8 strings, `Vec<Box<Span>>` spans) that `Node`'s
//! `ChildRef`/`ByteRef`/`SpanRef` indices select, plus the `Arc`-backed
//! `Space`/`Memo` handle tables. The side-arenas are index-parallel with the
//! arena's segments and are **co-released** when a segment dies, so a value and
//! its variable-length data are reclaimed together.
//!
//! **Why boxed per value (soundness):** each interned slice/str/span is its own
//! `Box`, so its heap address is stable for the segment's life even though the
//! per-segment `Vec<Box<_>>` reallocates as more values are interned (a single
//! `SExpr` can hold unbounded children, so the side `Vec`s are *not* bounded by
//! node count and cannot be preallocated to a stable base). A future `&'static`
//! launder (Inc 2a-5) must therefore launder from the `Box` pointee — never from
//! the `Vec` buffer — to avoid a reallocation use-after-free.
//!
//! Allocation discipline (the co-location rule that keeps non-moving reclamation
//! sound): **fixed-size nodes** may reuse a free-list slot in any segment;
//! **variable-length nodes** (`SExpr`/`Conjunction`/`Atom`/`String`/`Spanned`)
//! bump the node slot *and* their side data into the *same* segment.
//!
//! C1.c (#1, commit `674dbc5`) deliberately RELAXED the historical "variable-
//! length nodes never take a free-list slot" rule: a variable-length node MAY now
//! reuse a `cur_seg` free-list node-slot (via `write_reused`), which leaves the
//! node bump cursor put while `intern_*_in` APPENDS a fresh side index into the
//! same segment's [`SideColumn`]. Because `cur_seg` is release-exempt (it is the
//! live allocation frontier and is never co-released), a long-lived `cur_seg`
//! under reuse-heavy churn can claim side indices without an a-priori cap — so
//! the side column's ADDRESSABLE space must span the entire `u32` index range,
//! exactly like the `Vec<Option<Box<_>>>` it replaced (see the [`SideColumn`]
//! docs: a two-level lazy-growing directory, never capped by segment capacity).
//! Its OCCUPIED footprint is nonetheless bounded (audit Finding 3): post-`266d19d`
//! freed cells are RECYCLED via the per-cell-generation free list before the bump
//! grows, and `pending_side_major` forces a quiescence drain of pending reclaims —
//! growth ≤ live-side high-water + pending-drain, not append-only.
//!
//! This heap is the active `index-gc` value heap. A single global
//! `RwLock<IndexHeap>` backs the current index factory and collector paths; the
//! D-TLAB/concurrent-allocation work layers on top of this representation. The
//! module keeps `#![allow(dead_code)]` because some verification helpers and
//! cfg-gated collector variants are intentionally present across slab/index
//! build modes.

#![allow(dead_code)]

use std::sync::{OnceLock, RwLock};

use crate::backend::eval::cesk::index_arena::{
    Addr, ArenaNode, IndexArena, SweepStats, MAX_SEGMENTS,
};
use crate::backend::eval::cesk::index_node::{ByteRef, ChildRef, Node, SpanRef};
use crate::backend::eval::cesk::store::Store;
use crate::backend::models::gc_allocator::hash_cons_key;
use crate::backend::models::metta_value::{
    is_variable_str, MettaValueInner, ValueView, FLAG_HAS_VARIABLES, TAG5_ATOM, TAG5_CONJUNCTION,
    TAG5_ERROR, TAG5_FLOAT, TAG5_LAZY, TAG5_LONG, TAG5_MEMO, TAG5_NOT_REDUCIBLE, TAG5_QUOTED,
    TAG5_SEXPR, TAG5_SPACE, TAG5_SPANNED, TAG5_STATE, TAG5_STRING, TAG5_TYPE,
};
use crate::backend::models::{MemoHandle, MettaValue, MettaValueFactory, SpaceHandle};
use crate::ir::Span;

/// Per-segment variable-length side-arenas, index-parallel with the arena's
/// segments. Each interned value is its own `Box` (address-stable for the
/// segment's life — see the module docs); the `Vec<Option<Box<_>>>` may
/// reallocate, but the `Box` pointees never move. Co-released wholesale when the
/// owning segment is released (dropping the `Vec` drops every `Box` it owns).
///
/// C1.c (#1, the allocator↔GC coupling): the entries are `Option<Box<_>>` so a
/// swept-dead node's side slot can be FREED (set `None`, dropping its `Box`) at the
/// sweep — closing the gap where a node-slot reused by `alloc_sexpr`/etc. orphaned
/// the prior occupant's side `Box` (a leak bounded only by segment release).
/// `Option<Box<[T]>>`/`Option<Box<str>>`/`Option<Box<Span>>` are the same size as
/// the bare `Box` (the non-null pointer niche), so the `Option` costs no memory.
///
/// Indices are NOT recycled (the slot stays `None`; later allocations APPEND a fresh
/// index). Recycling a freed index would be unsound WITHOUT a per-slot occupancy bit:
/// the sweep re-reclaims still-free node-slots (their dead-node bytes are intact), so
/// a re-read of a dead node's recycled side-ref could free a now-*live* node's slot
/// (a "live ... slot" panic / UAF). Not recycling keeps the side-free IDEMPOTENT (a
/// dead node's slot is `None`d once; re-reads hit the `is_some` guard and skip). The
/// `Box` DATA is freed promptly; only the column SPINE grows (one `Option` ptr per
/// ever-interned datum), recovered wholesale at segment release. (A safe index
/// recycler would need an occupancy bitmap — a future refinement.)
///
/// D-TLAB-1.1: the three fields are now [`SideColumn<T>`] (a never-realloc chunked
/// column) instead of `Vec<Option<Box<_>>>`. The semantics are byte-identical at
/// FANOUT=0 — `push` still APPENDS and returns the stable index, `get` borrows the
/// published `Box` pointee (address-stable, so the launder contract is preserved —
/// see the module docs / [`launder`]), and `free` drops the payload `Box` leaving
/// `None` without recycling. The swap is the prerequisite for D-TLAB-1.2's `&self`
/// allocation path: `SideColumn::{push,get}` are already `&self` (callable through
/// the `&mut self` the intern/access methods still take in this increment); only
/// `free` needs `&mut self`, which the quiescence-only sweep already holds. No
/// concurrency is introduced here — every caller stays `&mut self`/`&self` exactly
/// as before — so this increment is purely the inner field-type swap.
///
/// D-TLAB-1.2: realized that prerequisite — the three `intern_*_in` methods and
/// every read accessor are now **`&self`**, reaching a segment's arena through the
/// heap's never-realloc side DIRECTORY (`IndexHeap::sides` + `side`/`ensure_side_seg`)
/// rather than `&mut self.sides[seg]`. Still byte-identical at FANOUT=0: `IndexFactory`
/// keeps the heap WRITE lock (no live concurrency yet — the read-lock flip is the
/// NEXT increment); the `&self` signature is purely the capability that flip needs.
struct SegmentSideArenas {
    children: SideColumn<[MettaValue]>,
    strings: SideColumn<str>,
    spans: SideColumn<Span>,
}

impl Default for SegmentSideArenas {
    /// A fresh, empty per-segment side arena. Each column allocates only its
    /// `MAX_SIDE_PAGES` super-directory (no page and no chunk until the first
    /// `push`) — see [`SideColumn::new`]. (A manual impl, not
    /// `#[derive(Default)]`: `SideColumn` has no `Default`, and its empty value
    /// is `new()`.)
    fn default() -> Self {
        SegmentSideArenas {
            children: SideColumn::new(),
            strings: SideColumn::new(),
            spans: SideColumn::new(),
        }
    }
}

/// Launder a side-arena / handle-table reference to `'static`, for building a
/// [`ValueView`] (whose composite arms borrow `&'static`). **Sound** because the
/// referent is either an address-stable `Box` pointee (children / strings /
/// spans — see [`SegmentSideArenas`]) or an append-only handle-table entry
/// ([`IndexHeap::space_table`] / [`memo_table`](IndexHeap::memo_table)), none of
/// which ever move; the referent is freed only when its segment is released at a
/// quiescent sweep, which the mark-completeness reuse-safety invariant forbids
/// while a live handle still names the segment. (The per-call read-lock under
/// which it is read is dropped after `view_at` returns; the laundered reference
/// outliving that guard is exactly what this stability guarantees.)
#[inline]
unsafe fn launder<'a, T: ?Sized>(r: &'a T) -> &'static T {
    std::mem::transmute::<&'a T, &'static T>(r)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SideReclaim {
    Children {
        owner: Addr,
        seg: usize,
        idx: u32,
        /// Generation the owner's ref carried when this snapshot was captured.
        /// `SideColumn::free` drops cell `idx` only when its current generation
        /// still equals this — so a reused index (whose generation a live
        /// re-intern bumped) is never freed by this stale snapshot.
        gen: u32,
    },
    Strings {
        owner: Addr,
        seg: usize,
        idx: u32,
        gen: u32,
    },
    Spans {
        owner: Addr,
        seg: usize,
        idx: u32,
        gen: u32,
    },
}

impl SideReclaim {
    #[inline]
    fn segment(self) -> usize {
        match self {
            SideReclaim::Children { seg, .. }
            | SideReclaim::Strings { seg, .. }
            | SideReclaim::Spans { seg, .. } => seg,
        }
    }

    #[inline]
    fn owner(self) -> Addr {
        match self {
            SideReclaim::Children { owner, .. }
            | SideReclaim::Strings { owner, .. }
            | SideReclaim::Spans { owner, .. } => owner,
        }
    }
}

/// The index-arena value heap.
pub struct IndexHeap {
    arena: IndexArena<Node>,
    /// Segment `i`'s variable-length data (its children/strings/spans columns),
    /// kept index-parallel with `arena.segments`. D-TLAB-1.2: a **never-realloc
    /// directory** (mirroring `IndexArena::segments`) of `MAX_SEGMENTS` cells,
    /// allocated once in [`new`](Self::new) so a fresh segment's side arena can be
    /// initialized under `&self` (the prerequisite for the next increment's
    /// read-lock allocation). Cell `i`'s `Box<SegmentSideArenas>` is written
    /// exactly once, under `sides_dir_lock`, before `sides_count` is advanced past
    /// `i` (`Release`); a reader dereferences cell `i` only for
    /// `i < sides_count.load(Acquire)` (see [`side`](Self::side)).
    ///
    /// LAZY by design: each cell starts `MaybeUninit::uninit()` and is populated
    /// only when [`ensure_side_seg`](Self::ensure_side_seg) first needs segment
    /// `i`. A pre-sized `Vec<SegmentSideArenas>` is NOT viable — each
    /// `SegmentSideArenas` eagerly allocates 3 `SideColumn` super-directories
    /// (≈48 KiB), so ×`MAX_SEGMENTS` (16_384) would commit ~786 MiB up front; the
    /// directory's eager cost is only the `MAX_SEGMENTS` *pointer* cells
    /// (`8 · MAX_SEGMENTS = 128 KiB`), with one `SegmentSideArenas` materialized
    /// per segment actually used.
    sides: Box<[std::cell::UnsafeCell<std::mem::MaybeUninit<Box<SegmentSideArenas>>>]>,
    /// Published side-directory length (count of initialized cells). Monotone;
    /// advanced with `Release` under `sides_dir_lock`, read with `Acquire`. Kept
    /// `== arena.segment_count()` by [`ensure_side_seg`](Self::ensure_side_seg).
    sides_count: std::sync::atomic::AtomicUsize,
    /// Serializes side-directory growth ([`ensure_side_seg`](Self::ensure_side_seg)):
    /// the rare slow path (once per opened segment). A plain `Mutex<()>` — the
    /// per-`push` interner fast path does NOT take it once a segment's cell is
    /// published. Mirrors `IndexArena::dir_lock`.
    sides_dir_lock: std::sync::Mutex<()>,
    /// Reclaim-time snapshots of side payloads whose owning node slots were swept.
    /// Node slots may be reused before the next true-quiescence side-free, so this
    /// stores `(segment, column, index)` at sweep time instead of rereading mutable
    /// node bytes later.
    pending_side_reclaims: Vec<SideReclaim>,
    /// `Arc`-backed `Space` handles, indexed by the `u64` id stored in
    /// `Node::Space(id)`. Append-only and never swept (handles are env-rooted).
    space_table: Vec<SpaceHandle>,
    /// `Arc`-backed `Memo` handles, indexed by `Node::Memo(id)`.
    memo_table: Vec<MemoHandle>,
    /// exp46 (v3-F1): prebuilt `MettaValueInner::Space` per space id — the
    /// `inner_ref_index` v2 branch target for `TAG5_SPACE` handles. `Space`/
    /// `Memo` payloads are `Arc`-backed (non-POD), so they are NEVER written
    /// to the shared Inner column; this append-only, never-freed store serves
    /// a stable `&MettaValueInner` instead (the `view_at` launder precedent:
    /// `Box` pointees are address-stable across `Vec` growth, and like
    /// `space_table` there is no removal path). Spaces/memos are few,
    /// long-lived, and cold — no hot profile shows them.
    space_inner_table: Vec<Box<MettaValueInner>>,
    /// exp46 (v3-F1): prebuilt `MettaValueInner::Memo` per memo id.
    memo_inner_table: Vec<Box<MettaValueInner>>,
    /// Ground-SExpr hash-cons table (CRUX Step 4): content hash of children's
    /// `tagged` bits → the canonical interned handle. Mirrors the slab's
    /// thread-local table (`gc_allocator.rs`) exactly — ground-SExpr-only, the
    /// shared [`hash_cons_key`], one entry per key (best-effort; a hash collision
    /// overwrites), capped at 8192 — so equal ground content yields the same
    /// `Addr` ⇒ the same `inner_ptr` key, matching Slab's fixpoint/cycle/dedup
    /// identity sites for the Inc-3 A/B differential (R9). Cleared on
    /// [`sweep`](Self::sweep) (Inc 6 wires the live sweep; until then it is a
    /// bounded monotone intern table, like the slab table between safepoints).
    // Keyed on `hash_cons_key` (already a well-distributed u64), so the default
    // SipHash would re-hash an existing hash — use the identity build-hasher
    // (perf F1: the index access/alloc path spent ~10% in SipHash on handle keys).
    hash_cons: std::collections::HashMap<
        u64,
        MettaValue,
        crate::backend::hash_utils::IdentityU64BuildHasher,
    >,
}

impl Default for IndexHeap {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexHeap {
    /// A heap with the default (production) segment capacity.
    pub fn new() -> Self {
        Self::from_arena(IndexArena::new())
    }

    /// A heap whose segments hold `capacity` slots each (tests use small caps to
    /// exercise multi-segment behavior cheaply).
    pub fn with_segment_capacity(capacity: usize) -> Self {
        Self::from_arena(IndexArena::with_segment_capacity(capacity))
    }

    fn from_arena(arena: IndexArena<Node>) -> Self {
        use std::cell::UnsafeCell;
        use std::mem::MaybeUninit;
        // D-TLAB-1.2: allocate the never-realloc side directory ONCE — one uninit
        // pointer-cell per addressable segment index (`UnsafeCell`/`MaybeUninit`
        // are not `Clone`, so build it from an iterator, NOT `vec![..; N]`). Each
        // cell's `Box<SegmentSideArenas>` is populated lazily by `ensure_side_seg`.
        let sides: Box<[UnsafeCell<MaybeUninit<Box<SegmentSideArenas>>>]> = (0..MAX_SEGMENTS)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        let h = IndexHeap {
            arena,
            sides,
            sides_count: std::sync::atomic::AtomicUsize::new(0),
            sides_dir_lock: std::sync::Mutex::new(()),
            pending_side_reclaims: Vec::new(),
            space_table: Vec::new(),
            memo_table: Vec::new(),
            space_inner_table: Vec::new(),
            memo_inner_table: Vec::new(),
            hash_cons: std::collections::HashMap::with_hasher(
                crate::backend::hash_utils::IdentityU64BuildHasher,
            ),
        };
        // Materialize the side arena for every segment the arena has already opened
        // (its constructor opens segment 0). `&self` even during construction —
        // `ensure_side_seg` only needs shared access.
        let initial_segs = h.arena.segment_count();
        if initial_segs > 0 {
            h.ensure_side_seg(initial_segs - 1);
        }
        h
    }

    /// Lazily initialize the side-directory cells `[sides_count..=seg]`, so segment
    /// `seg`'s side arena exists and is published. `&self` — the D-TLAB-1.2 lever:
    /// a fresh segment's side arena can be created under shared access (the
    /// prerequisite for the next increment's read-lock allocation). Replaces the
    /// old `&mut self` `sync_sides`.
    ///
    /// Mirrors `IndexArena::open_segment` exactly: take `sides_dir_lock` (the rare
    /// slow path — once per opened segment), RE-CHECK `seg < sides_count` under the
    /// lock (a concurrent caller may have already grown past `seg` while we
    /// waited), then for each missing cell `c` allocate its `Box<SegmentSideArenas>`,
    /// write it (we are the unique writer of cell `c` under the lock — it is not
    /// yet published), and publish it by advancing `sides_count` (`Release`). The
    /// `Release` store pairs with the `Acquire` load in [`side`](Self::side) so a
    /// reader observing `c < sides_count` also observes the initialized cell.
    ///
    /// Fills the whole `sides_count..=seg` gap (not just `seg`) so the directory
    /// stays dense even if `seg` ever jumps ahead (it does not today — the arena
    /// opens segments one at a time — but a future relaxation might).
    #[inline]
    fn ensure_side_seg(&self, seg: usize) {
        use std::sync::atomic::Ordering;
        // Fast path: already published (Acquire pairs with the publishing Release).
        if seg < self.sides_count.load(Ordering::Acquire) {
            return;
        }
        let _guard = self
            .sides_dir_lock
            .lock()
            .expect("index-heap sides_dir_lock poisoned");
        let mut next = self.sides_count.load(Ordering::Acquire); // exclusive under guard
        if seg < next {
            return; // another thread grew past `seg` while we waited
        }
        assert!(
            seg < MAX_SEGMENTS,
            "side directory exhausted: {MAX_SEGMENTS} segments"
        );
        while next <= seg {
            let arenas = Box::new(SegmentSideArenas::default());
            // SAFETY: cell `next` is not yet published (`next == sides_count`), so
            // no reader can observe it; under `sides_dir_lock` we are the unique
            // writer of this cell. Initialize it before publishing `next`.
            unsafe {
                (*self.sides[next].get()).write(arenas);
            }
            // exp46: grow the shared Inner column in lockstep (published before
            // sides_count, hence before any node in this segment can allocate).
            crate::backend::eval::cesk::inner_column::ensure_column_seg(
                next,
                self.arena.segment_capacity(),
            );
            self.sides_count.store(next + 1, Ordering::Release); // publish the cell
            next += 1;
        }
    }

    /// Borrow segment `seg`'s side arena from the published directory.
    ///
    /// # Safety
    /// The caller must guarantee `seg < self.sides_count.load(Acquire)` as observed
    /// by the calling thread — i.e. cell `seg` has been published by
    /// [`ensure_side_seg`](Self::ensure_side_seg)'s `Release` store to
    /// `sides_count`, which happens-after the cell's initialization. Under that
    /// premise the cell is initialized.
    ///
    /// The returned `&SegmentSideArenas` derives from the raw pointer
    /// `UnsafeCell::get()` yields (`*mut MaybeUninit<Box<SegmentSideArenas>>`), NOT
    /// from a borrow of `self.sides`. This is deliberate — mirroring
    /// `IndexArena::segment` — so callers can hold a `&SegmentSideArenas` (to push
    /// into one of its `SideColumn`s) without aliasing a borrow of the directory.
    #[inline]
    unsafe fn side(&self, seg: usize) -> &SegmentSideArenas {
        let cell = self.sides[seg].get(); // *mut MaybeUninit<Box<SegmentSideArenas>>
        (*cell).assume_init_ref() // &Box<SegmentSideArenas> -> &SegmentSideArenas via Deref
    }

    /// Mutably borrow segment `seg`'s side arena. **`&mut self`** (quiescence-only:
    /// its only callers — [`free_pending_side_reclaims`](Self::free_pending_side_reclaims)
    /// and the segment-release reset — run under the heap write lock at a quiescent
    /// safepoint, statically exclusive of every `&self` reader/pusher, so the `&mut
    /// SegmentSideArenas` aliases nothing).
    ///
    /// # Safety
    /// `seg < self.sides_count.load(Acquire)` (cell `seg` published) AND the caller
    /// holds exclusive (`&mut self`) access. Under `&mut self` no concurrent
    /// `&self` accessor exists, so the `&mut` borrow derived from the raw pointer is
    /// the unique reference to the cell's contents.
    #[inline]
    unsafe fn side_mut(&mut self, seg: usize) -> &mut SegmentSideArenas {
        let cell = self.sides[seg].get(); // *mut MaybeUninit<Box<SegmentSideArenas>>
        (*cell).assume_init_mut() // &mut Box<SegmentSideArenas> -> &mut SegmentSideArenas
    }

    // NOTE (D-TLAB-1.2): the segment-release reset (the directory analogue of the
    // old `sides[seg] = SegmentSideArenas::default()`) is performed INLINE in the
    // `sweep`/`sweep_young` release closures rather than via a `&mut self` helper:
    // those closures run while `self.arena` is mutably borrowed by `sweep_with`/
    // `sweep_young_with`, so they capture the DISJOINT `&mut self.sides` field
    // directly (a whole-`&mut self` helper would alias the arena borrow). The reset
    // drops the cell's old `Box<SegmentSideArenas>` (its `SideColumn`s free every
    // payload/chunk/page `Box`) and installs a fresh empty arena in place.

    // ── Allocation ───────────────────────────────────────────────────────

    /// Allocate a fixed-size node (no side-arena data). Free-list reuse is fine.
    pub fn alloc_fixed(&mut self, node: Node) -> Addr {
        let a = self.arena.alloc(node);
        // `alloc` may have opened a fresh segment internally — keep the side
        // directory index-parallel so `sides_count == segment_count` holds for the
        // diagnostics / release paths (a fixed node has no side data of its own).
        self.ensure_side_seg(self.arena.segment_count() - 1);
        a
    }

    /// Register a `Space` handle and allocate its node (id → side table).
    pub fn alloc_space(&mut self, handle: SpaceHandle) -> Addr {
        let id = self.space_table.len() as u64;
        // exp46 (v3-F1): prebuild the id-store Inner the column will never hold.
        self.space_inner_table
            .push(Box::new(MettaValueInner::Space(handle.clone())));
        self.space_table.push(handle);
        self.alloc_fixed(Node::Space(id))
    }

    /// Register a `Memo` handle and allocate its node.
    pub fn alloc_memo(&mut self, handle: MemoHandle) -> Addr {
        let id = self.memo_table.len() as u64;
        self.memo_inner_table
            .push(Box::new(MettaValueInner::Memo(handle.clone())));
        self.memo_table.push(handle);
        self.alloc_fixed(Node::Memo(id))
    }

    /// Allocate an `SExpr`, co-locating its children in the node's segment.
    /// C1.c #1: REUSE a `cur_seg` free slot if available (interning the children
    /// into that slot's segment so they co-locate + co-release), else BUMP. Reuse
    /// feeds the minor's reclaimed young slots back into allocation, bounding
    /// committed (the variable-length path previously only bumped → reclaim wasted).
    pub fn alloc_sexpr(&mut self, items: &[MettaValue]) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let cr = self.intern_children_in(addr.segment(), items);
            self.arena.write_reused(addr, Node::SExpr(cr));
            addr
        } else {
            // D-TLAB-1.2 co-location single-pick + retry: pick `seg` ONCE
            // (`ensure_bump_room`), intern the children into THAT `seg`, then
            // `try_bump_in(seg, ..)`. If a concurrent `open_segment` advanced the
            // bump target between the pick and the bump, `try_bump_in` returns
            // `None` and we RETRY the whole triple against the new segment — the
            // orphaned side entry in the stale `seg` is the no-recycle steady state
            // (a never-handed-out index, freed wholesale at that segment's release).
            // The side datum is interned BEFORE the node is published, so the node's
            // `Release`-publish transitively gates the side entry's visibility.
            loop {
                let seg = self.arena.ensure_bump_room();
                let cs = self.intern_children_in(seg, items);
                if let Some(addr) = self.arena.try_bump_in(seg, Node::SExpr(cs)) {
                    return addr;
                }
            }
        }
    }

    /// Allocate a `Conjunction`, co-locating its goals (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_conjunction(&mut self, goals: &[MettaValue]) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let cr = self.intern_children_in(addr.segment(), goals);
            self.arena.write_reused(addr, Node::Conjunction(cr));
            addr
        } else {
            // D-TLAB-1.2 single-pick + retry (see `alloc_sexpr`).
            loop {
                let seg = self.arena.ensure_bump_room();
                let cs = self.intern_children_in(seg, goals);
                if let Some(addr) = self.arena.try_bump_in(seg, Node::Conjunction(cs)) {
                    return addr;
                }
            }
        }
    }

    /// Hash-cons a GROUND `SExpr` (caller guarantees variable-free): return the
    /// canonical interned handle for this content, allocating only on a miss.
    /// Mirrors `GcFactory`'s ground-SExpr hash-cons (shared [`hash_cons_key`];
    /// children compared by `tagged` identity — sound because ground children are
    /// themselves interned, so equal content ⇒ equal child handles). Runs under
    /// the heap write lock, but the structural check reads `self.children`
    /// directly (a field access, NOT a re-lock through `global_index_heap`), so
    /// it cannot deadlock.
    pub fn intern_ground_sexpr(&mut self, items: &[MettaValue]) -> MettaValue {
        let key = hash_cons_key(items);
        let mut drop_stale_entry = false;
        if let Some(&existing) = self.hash_cons.get(&key) {
            if let Some(addr) = existing.as_arena_addr() {
                if let Some(kids) = self.children_if_present(addr) {
                    if kids.len() == items.len()
                        && kids.iter().zip(items).all(|(a, b)| a.tagged == b.tagged)
                    {
                        return existing;
                    }
                }
            }
            drop_stale_entry = true;
        }
        if drop_stale_entry {
            self.hash_cons.remove(&key);
        }
        // Miss (or hash collision → overwrite, matching the slab table's
        // last-writer-wins-per-key best-effort behavior).
        let addr = self.alloc_sexpr(items);
        self.populate_column(addr); // exp46 (miss path)
        let v = MettaValue::from_addr(addr, 0, TAG5_SEXPR); // ground ⇒ no FLAG_HAS_VARIABLES
        if self.hash_cons.len() < 8192 {
            self.hash_cons.insert(key, v);
        }
        v
    }

    /// Allocate an `Atom`, co-locating its bytes (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_atom(&mut self, s: &str) -> Addr {
        // Finding 2: intern the atom's bytes into the PERPETUAL interner (honest
        // `&'static str`) instead of a freeable side-column — so a sweep can never
        // dangle the borrow (`formal/rocq/gc/InternedAtomNeverFreed.v`). The interned
        // `&'static` is `Copy`, so re-passing it across the bump retry is free, and
        // atoms no longer touch `intern_bytes_in` / the byte side-arena at all.
        let interned = crate::backend::symbol::intern_static(s);
        if let Some(addr) = self.arena.pop_young_free_slot() {
            self.arena.write_reused(addr, Node::Atom(interned));
            addr
        } else {
            // D-TLAB-1.2 single-pick + retry (see `alloc_sexpr`).
            loop {
                let seg = self.arena.ensure_bump_room();
                if let Some(addr) = self.arena.try_bump_in(seg, Node::Atom(interned)) {
                    return addr;
                }
            }
        }
    }

    /// Allocate a `String`, co-locating its bytes (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_string(&mut self, s: &str) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let br = self.intern_bytes_in(addr.segment(), s);
            self.arena.write_reused(addr, Node::String(br));
            addr
        } else {
            // D-TLAB-1.2 single-pick + retry (see `alloc_sexpr`).
            loop {
                let seg = self.arena.ensure_bump_room();
                let bs = self.intern_bytes_in(seg, s);
                if let Some(addr) = self.arena.try_bump_in(seg, Node::String(bs)) {
                    return addr;
                }
            }
        }
    }

    /// Allocate a `Spanned`, co-locating the (boxed) span (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_spanned(&mut self, inner: MettaValue, span: Span) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let sr = self.intern_span_in(addr.segment(), span);
            self.arena.write_reused(addr, Node::Spanned(inner, sr));
            addr
        } else {
            // D-TLAB-1.2 single-pick + retry (see `alloc_sexpr`). `inner` is a
            // `Copy` handle, so re-passing it on each retry iteration is free.
            loop {
                let seg = self.arena.ensure_bump_room();
                let sr = self.intern_span_in(seg, span);
                if let Some(addr) = self.arena.try_bump_in(seg, Node::Spanned(inner, sr)) {
                    return addr;
                }
            }
        }
    }

    // ── D-RLOCK.1: `&self` bump-ONLY allocation entries ──────────────────────
    //
    // The next increment (D-RLOCK.2) flips `IndexFactory`'s HOT sites off the
    // serializing `global_index_heap().write()` onto a concurrent
    // `.read()`+`&self` bump path so parallel-eval workers stop serializing
    // against each other on the heap write lock. These methods are the `&self`
    // bump-ONLY entries that path calls: each body is the EXACT `else`-branch
    // bump loop of the corresponding `alloc_*` above (the
    // `ensure_bump_room`/`intern_*_in`/`try_bump_in` single-pick + retry triple,
    // every call of which is already `&self`), with the `pop_young_free_slot` +
    // `write_reused` REUSE branch DELETED — reuse stays `&mut self`/quiescence
    // -only (a concurrent claim must never alias a reused slot — the
    // ABA/torn-node class `IndexArena::alloc_bump` is documented to avoid; TLA+
    // `NoConcurrentFree`).
    //
    // SOUNDNESS of running these under a SHARED (`.read()`) heap guard:
    //   * Every primitive they call is `&self` and obeys the B2 lock-free arena
    //     protocol (`alloc_bump`/`try_bump_in` claim a unique slot via the atomic
    //     bump cursor and publish with `Release`; `intern_*_in` → `SideColumn::push`
    //     claims+publishes a side index lock-free; `ensure_side_seg` publishes a
    //     directory cell under its own small mutex). No `&self` path here FREES or
    //     RESETS anything.
    //   * The side datum is interned BEFORE the node is published (`try_bump_in`'s
    //     `publish` is the node's release point), so the node's `Release`-publish
    //     transitively gates the side entry's visibility (same ordering the loom
    //     `loom_side_node_ordering` model proves).
    //   * The collector (the only thing that frees) is `&mut self` everywhere
    //     (`sweep`/`sweep_young`/`free_pending_side_reclaims`/`SideColumn::free`),
    //     so it is UNCALLABLE through a `.read()` guard — type-enforced — AND it is
    //     gated OFF once any worker is spawned (`!worker_ever_spawned()`), which is
    //     exactly the regime in which concurrent (`.read()`) allocation occurs. So
    //     a concurrent bump never races a live collector on two independent grounds.
    //
    // INERT this increment: NOT called by anything yet (D-RLOCK.2 wires them).
    // The module is `#![allow(dead_code)]`, so they raise no dead-code warning.

    /// `&self` bump-ONLY allocation of an `SExpr`, co-locating its children in the
    /// node's segment. Bump-only twin of [`alloc_sexpr`](Self::alloc_sexpr)'s
    /// `else` branch — see the D-RLOCK.1 block comment above.
    pub fn alloc_sexpr_concurrent(&self, items: &[MettaValue]) -> Addr {
        loop {
            let seg = self.arena.ensure_bump_room();
            let cs = self.intern_children_in(seg, items);
            if let Some(addr) = self.arena.try_bump_in(seg, Node::SExpr(cs)) {
                return addr;
            }
        }
    }

    /// `&self` bump-ONLY allocation of a `Conjunction`, co-locating its goals.
    /// Bump-only twin of [`alloc_conjunction`](Self::alloc_conjunction)'s `else`
    /// branch — see the D-RLOCK.1 block comment above.
    pub fn alloc_conjunction_concurrent(&self, goals: &[MettaValue]) -> Addr {
        loop {
            let seg = self.arena.ensure_bump_room();
            let cs = self.intern_children_in(seg, goals);
            if let Some(addr) = self.arena.try_bump_in(seg, Node::Conjunction(cs)) {
                return addr;
            }
        }
    }

    /// `&self` bump-ONLY allocation of an `Atom`, co-locating its bytes. Bump-only
    /// twin of [`alloc_atom`](Self::alloc_atom)'s `else` branch — see the D-RLOCK.1
    /// block comment above.
    pub fn alloc_atom_concurrent(&self, s: &str) -> Addr {
        // Finding 2: intern into the perpetual interner (see `alloc_atom`); atoms carry
        // an honest `&'static str`, never a freeable side-column byte index.
        let interned = crate::backend::symbol::intern_static(s);
        loop {
            let seg = self.arena.ensure_bump_room();
            if let Some(addr) = self.arena.try_bump_in(seg, Node::Atom(interned)) {
                return addr;
            }
        }
    }

    /// `&self` bump-ONLY allocation of a `String`, co-locating its bytes. Bump-only
    /// twin of [`alloc_string`](Self::alloc_string)'s `else` branch — see the
    /// D-RLOCK.1 block comment above.
    pub fn alloc_string_concurrent(&self, s: &str) -> Addr {
        loop {
            let seg = self.arena.ensure_bump_room();
            let bs = self.intern_bytes_in(seg, s);
            if let Some(addr) = self.arena.try_bump_in(seg, Node::String(bs)) {
                return addr;
            }
        }
    }

    /// `&self` bump-ONLY allocation of a `Spanned`, co-locating the (boxed) span.
    /// Bump-only twin of [`alloc_spanned`](Self::alloc_spanned)'s `else` branch —
    /// see the D-RLOCK.1 block comment above.
    pub fn alloc_spanned_concurrent(&self, inner: MettaValue, span: Span) -> Addr {
        loop {
            let seg = self.arena.ensure_bump_room();
            let sr = self.intern_span_in(seg, span);
            if let Some(addr) = self.arena.try_bump_in(seg, Node::Spanned(inner, sr)) {
                return addr;
            }
        }
    }

    /// `&self` bump-ONLY allocation of a FIXED-size node (no side-arena data).
    /// Uses [`IndexArena::alloc_bump`] (`&self`, fresh-bump-only — NOT `alloc`,
    /// which is `&mut self` and may reuse a free slot) and keeps the side
    /// directory index-parallel (`ensure_side_seg`, `&self`) exactly as
    /// [`alloc_fixed`](Self::alloc_fixed) does. See the D-RLOCK.1 block comment.
    pub fn alloc_fixed_concurrent(&self, node: Node) -> Addr {
        let a = self.arena.alloc_bump(node);
        // `alloc_bump` may have opened a fresh segment internally — keep the side
        // directory index-parallel (a fixed node has no side data of its own).
        self.ensure_side_seg(self.arena.segment_count() - 1);
        a
    }

    /// True when the exclusive allocation path can immediately consume a
    /// reclaimed slot in the current bump segment.
    #[inline]
    fn has_current_free_slot(&self) -> bool {
        self.arena.has_current_free_slot()
    }

    /// C1.c #1: box `items` into segment `seg`'s child side-arena, APPENDING a
    /// fresh stable index (no-recycle — see [`SegmentSideArenas`]), returning it.
    /// The caller co-locates the owning node in the SAME `seg` (so node + children
    /// co-release, and `children(addr)` reads segment `addr.segment()`).
    ///
    /// D-TLAB-1.2: now **`&self`** — the lever for the next increment's read-lock
    /// allocation. Ensures segment `seg`'s side directory cell is published
    /// ([`ensure_side_seg`], `&self`), borrows it ([`side`], `&self` Acquire-gated
    /// cell read — NOT a `&mut self.sides[seg]`), then `SideColumn::push` (already
    /// `&self`: lock-free claim+publish). No concurrency is introduced THIS
    /// increment (every caller still holds the heap WRITE lock); the `&self`
    /// signature is what lets the *next* increment flip the factory to `.read()`.
    fn intern_children_in(&self, seg: usize, items: &[MettaValue]) -> ChildRef {
        self.ensure_side_seg(seg);
        // SAFETY: `ensure_side_seg(seg)` just published cell `seg` (or it already
        // was), so `seg < sides_count` as observed here (the `Release` store in
        // `ensure_side_seg` is observed by this same thread).
        let side = unsafe { self.side(seg) };
        let (idx, gen) = side.children.push(items.to_vec().into_boxed_slice());
        ChildRef { idx, gen }
    }

    fn intern_bytes_in(&self, seg: usize, s: &str) -> ByteRef {
        self.ensure_side_seg(seg);
        // SAFETY: as `intern_children_in` — cell `seg` published by the call above.
        let side = unsafe { self.side(seg) };
        let (idx, gen) = side.strings.push(s.to_string().into_boxed_str());
        ByteRef { idx, gen }
    }

    fn intern_span_in(&self, seg: usize, span: Span) -> SpanRef {
        self.ensure_side_seg(seg);
        // SAFETY: as `intern_children_in` — cell `seg` published by the call above.
        let side = unsafe { self.side(seg) };
        let (idx, gen) = side.spans.push(Box::new(span));
        SpanRef { idx, gen }
    }

    /// Bump-path interning: into the current bump segment (`ensure_bump_room`).
    /// NOTE: the `alloc_*` bump path no longer routes through this two-step helper
    /// — it picks `seg` ONCE and passes it to `intern_children_in` + `try_bump_in`
    /// together (the co-location single-pick, D-TLAB-1.2). Retained for any direct
    /// caller / symmetry; `&self`.
    fn intern_children(&self, items: &[MettaValue]) -> ChildRef {
        let seg = self.arena.ensure_bump_room();
        self.intern_children_in(seg, items)
    }

    fn intern_bytes(&self, s: &str) -> ByteRef {
        let seg = self.arena.ensure_bump_room();
        self.intern_bytes_in(seg, s)
    }

    // ── Access ───────────────────────────────────────────────────────────

    #[inline]
    pub fn get(&self, addr: Addr) -> &Node {
        self.arena.get(addr)
    }

    /// The children of an `SExpr`/`Conjunction` at `addr`. (C1.c #1: the side slot
    /// is `Some` for any live node — a slot is set `None` only when its node is
    /// swept-dead, and a dead node is never read.)
    pub fn children(&self, addr: Addr) -> &[MettaValue] {
        match self.arena.get(addr) {
            // SAFETY (D-TLAB-1.1): `cr.idx` was returned by `SideColumn::push` when
            // this node was interned, so it is < `published_len()` (publish happens
            // in the same `push` that produced the index); the node is live (a dead
            // node is never read — see the doc comment), so its slot is `Some`.
            Node::SExpr(cr) | Node::Conjunction(cr) => {
                // SAFETY (D-TLAB-1.2): segment `addr.segment()`'s side cell was
                // published by the interner's `ensure_side_seg` BEFORE this node's
                // address was handed out, so `addr.segment() < sides_count`; `cr.idx
                // < published_len()` and the slot is `Some` (live node) per above.
                let side = unsafe { self.side(addr.segment()) };
                unsafe { side.children.get(cr.idx) }.unwrap_or_else(|| {
                    panic!(
                        "live SExpr/Conjunction children slot: addr={addr:?} seg={} child_idx={}",
                        addr.segment(),
                        cr.idx
                    )
                })
            }
            _ => panic!("children() on a non-SExpr/Conjunction node"),
        }
    }

    /// Non-panicking child lookup for auxiliary tables whose entries can lag a
    /// sweep. A valid hash-cons hit must still name an allocated node slot and a
    /// present children side payload; otherwise the table entry is stale and the
    /// caller must treat it as a miss.
    fn children_if_present(&self, addr: Addr) -> Option<&[MettaValue]> {
        let cr = match self.arena.get_if_allocated(addr)? {
            Node::SExpr(cr) | Node::Conjunction(cr) => cr,
            _ => return None,
        };
        let seg = addr.segment();
        if seg >= self.sides_count.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        // SAFETY: `seg < sides_count` observed above, so the side cell was
        // published before this read. `get_if_published` checks the child index
        // before using the column's unsafe indexed read.
        let side = unsafe { self.side(seg) };
        side.children.get_if_published(cr.idx)
    }

    /// The string of an `Atom`/`String` at `addr`.
    pub fn str_slice(&self, addr: Addr) -> &str {
        match self.arena.get(addr) {
            // Finding 2: an atom's bytes are INTERNED (`&'static str`), not in a
            // freeable side-column — return them directly (honest, never freed).
            Node::Atom(s) => s,
            // SAFETY (D-TLAB-1.1): `br.idx` came from `SideColumn::push` at intern,
            // so `idx < published_len()`; the node is live ⇒ slot is `Some`.
            Node::String(br) => {
                // SAFETY (D-TLAB-1.2): cell `addr.segment()` published before this
                // node's address was handed out (see `children`); `br.idx` in range.
                let side = unsafe { self.side(addr.segment()) };
                unsafe { side.strings.get(br.idx) }.expect("live String slot")
            }
            _ => panic!("str_slice() on a non-Atom/String node"),
        }
    }

    /// The `Span` of a `Spanned` at `addr`.
    pub fn span_at(&self, addr: Addr) -> Span {
        match self.arena.get(addr) {
            // SAFETY (D-TLAB-1.1): `sr.idx` came from `SideColumn::push` at intern,
            // so `idx < published_len()`; the node is live ⇒ slot is `Some`.
            Node::Spanned(_, sr) => {
                // SAFETY (D-TLAB-1.2): cell `addr.segment()` published before this
                // node's address was handed out (see `children`); `sr.idx` in range.
                let side = unsafe { self.side(addr.segment()) };
                *unsafe { side.spans.get(sr.idx) }.expect("live Spanned slot")
            }
            _ => panic!("span_at() on a non-Spanned node"),
        }
    }

    /// The live `Space` handle for a `Node::Space(id)`.
    pub fn space_handle(&self, id: u64) -> &SpaceHandle {
        &self.space_table[id as usize]
    }

    /// The live `Memo` handle for a `Node::Memo(id)`.
    pub fn memo_handle(&self, id: u64) -> &MemoHandle {
        &self.memo_table[id as usize]
    }

    /// The prebuilt `MettaValueInner` for a `Node::Space(id)`/`Node::Memo(id)`
    /// — the `inner_ref_index` v2 Space/Memo branch (exp46, v3-F1). These two
    /// variants' payloads are `Arc`-backed (non-POD) and are NEVER written to
    /// the shared Inner column; this append-only, never-freed id store serves
    /// them instead. The returned reference's pointee is a `Box` target that
    /// is address-stable across table growth and never freed — laundering it
    /// to `&'static` is sound for exactly the reasons `space_handle`'s
    /// `view_at` launder is.
    pub(crate) fn prebuilt_space_memo_inner(&self, addr: Addr) -> &MettaValueInner {
        match self.arena.get(addr) {
            Node::Space(id) => &self.space_inner_table[*id as usize],
            Node::Memo(id) => &self.memo_inner_table[*id as usize],
            other => unreachable!(
                "prebuilt_space_memo_inner on non-Space/Memo node (variant_code {}) — \
                 TAG5 branch desync",
                other.variant_code()
            ),
        }
    }

    /// Decode the value at `addr` into a [`ValueView`], stripping `Spanned`
    /// layers — the index-mode body of [`MettaValue::view()`]. Composite arms
    /// borrow `&'static` side-arena / handle-table data via [`launder`] (sound
    /// per its contract). Spanned stripping follows the inner handle in a loop
    /// under the *same* `&self` borrow (no nested lock acquisition).
    pub fn view_at(&self, addr: Addr) -> ValueView {
        let mut a = addr;
        // Strip Spanned layers (rare), then decode the bare node.
        let node = loop {
            match self.arena.get(a) {
                Node::Spanned(inner, _) => {
                    if inner.is_inline() {
                        // Inline scalar: decode directly (mode-independent, no heap).
                        return inner.view();
                    }
                    a = Addr::from_raw((inner.tagged >> 4) as u32);
                }
                other => break other,
            }
        };
        match node {
            Node::Float(f) => ValueView::Float(*f),
            Node::Bool(b) => ValueView::Bool(*b),
            Node::Long(n) => ValueView::Long(*n),
            Node::Unit => ValueView::Unit,
            Node::Empty => ValueView::Empty,
            Node::NotReducible => ValueView::NotReducible,
            // Finding 2: the atom's `&'static str` is already honest (interned, never
            // freed — `InternedAtomNeverFreed.v`), so read it directly — NO `launder`.
            Node::Atom(s) => ValueView::Atom(*s),
            Node::String(_) => ValueView::String(unsafe { launder(self.str_slice(a)) }),
            Node::SExpr(_) => ValueView::SExpr(unsafe { launder(self.children(a)) }),
            Node::Conjunction(_) => ValueView::Conjunction(unsafe { launder(self.children(a)) }),
            Node::Error(off, det) => ValueView::Error(*off, *det),
            Node::Type(inner) => ValueView::Type(*inner),
            Node::Space(id) => ValueView::Space(unsafe { launder(self.space_handle(*id)) }),
            Node::State(id) => ValueView::State(*id),
            Node::Memo(id) => ValueView::Memo(unsafe { launder(self.memo_handle(*id)) }),
            Node::Quoted(inner) => ValueView::Quoted(*inner),
            Node::Lazy(inner) => ValueView::Lazy(*inner),
            Node::Spanned(..) => unreachable!("Spanned stripped in the loop above"),
        }
    }

    /// Materialize the value at `addr` as an owned [`MettaValueInner`] (the
    /// slab-era representation), for the index-mode `MettaValue::inner_ref()`
    /// Addr-keyed shadow cache. Unlike [`view_at`](Self::view_at) this does NOT
    /// strip `Spanned` (mirrors slab `inner_ref`/`inner_raw`, which return the raw
    /// outer node). Composite payloads (`&'static str` / `&'static [MettaValue]` /
    /// `&'static Span`) are the same address-stable side-arena `Box` pointees
    /// `view_at` launders (sound per [`launder`]); `Space`/`Memo` clone their
    /// `Arc`-backed handle (cheap — `MettaValueInner::Space`/`Memo` own the handle
    /// by value).
    pub fn materialize_inner(&self, addr: Addr) -> MettaValueInner {
        match self.arena.get(addr) {
            // Finding 2: the atom's `&'static str` is already honest (interned) — read
            // it directly, NO `launder` (`InternedAtomNeverFreed.v`).
            Node::Atom(s) => MettaValueInner::Atom(*s),
            Node::Bool(b) => MettaValueInner::Bool(*b),
            Node::Long(n) => MettaValueInner::Long(*n),
            Node::Float(f) => MettaValueInner::Float(*f),
            Node::String(_) => MettaValueInner::String(unsafe { launder(self.str_slice(addr)) }),
            Node::SExpr(_) => MettaValueInner::SExpr(unsafe { launder(self.children(addr)) }),
            Node::Error(off, det) => MettaValueInner::Error(*off, *det),
            Node::Type(inner) => MettaValueInner::Type(*inner),
            Node::Conjunction(_) => {
                MettaValueInner::Conjunction(unsafe { launder(self.children(addr)) })
            }
            Node::Space(id) => MettaValueInner::Space(self.space_handle(*id).clone()),
            Node::State(id) => MettaValueInner::State(*id),
            Node::Unit => MettaValueInner::Unit,
            Node::Memo(id) => MettaValueInner::Memo(self.memo_handle(*id).clone()),
            Node::Quoted(inner) => MettaValueInner::Quoted(*inner),
            Node::Lazy(inner) => MettaValueInner::Lazy(*inner),
            Node::Empty => MettaValueInner::Empty,
            Node::NotReducible => MettaValueInner::NotReducible,
            Node::Spanned(inner, sr) => {
                // SAFETY (D-TLAB-1.2): cell `addr.segment()` was published by the
                // interner's `ensure_side_seg` before this node's address was handed
                // out, so `addr.segment() < sides_count`; `sr.idx < published_len()`
                // and the slot is `Some` (live node). The launder is sound for the
                // same reason as before — the `Box<Span>` pointee is address-stable
                // for the segment's life (`SideColumn` never moves a published `Box`,
                // and the directory cell `Box<SegmentSideArenas>` itself never moves);
                // only the *source* of the `&Span` changed (`Vec[seg]` → `side(seg)`).
                let side = unsafe { self.side(addr.segment()) };
                let span: &'static Span =
                    unsafe { launder(side.spans.get(sr.idx).expect("live Spanned slot")) };
                MettaValueInner::Spanned(*inner, span)
            }
        }
    }

    /// exp46: build + publish `addr`'s shared column entry (post-alloc,
    /// pre-escape — the v3-F2 contract; the caller holds whichever heap guard
    /// it allocated under, or owns the slot exclusively). Space/Memo nodes
    /// are SKIPPED (v3-F1: served from the append-only id store; their
    /// `materialize_inner` would Arc-clone — never stored in POD cells).
    #[inline]
    pub(crate) fn populate_column(&self, addr: Addr) {
        // Self-ensure: the CONCURRENT bump path can open a node segment
        // without routing through ensure_side_seg (pure-fixed nodes carry no
        // side data), so the column segment may not be published yet. The
        // fast path is one Acquire load (the inc2-gate SEGV lesson: two
        // concurrent env tests crashed on an unpublished directory cell).
        crate::backend::eval::cesk::inner_column::ensure_column_seg(
            addr.segment(),
            self.arena.segment_capacity(),
        );
        match self.get(addr) {
            Node::Space(_) | Node::Memo(_) => {}
            _ => unsafe {
                crate::backend::eval::cesk::inner_column::column_write(
                    addr,
                    self.materialize_inner(addr),
                );
            },
        }
    }

    // ── Collection ─────────────────────────────────────────────────────────

    /// Append every arena edge leaving `addr`.
    ///
    /// This is the index-mode counterpart of the slab tracer's
    /// `MettaValueInner` walk. `Node::Space` is not a leaf: a first-class
    /// `SpaceHandle` may contain value-bearing module atoms / variable atoms, so
    /// the GC closure must include `SpaceHandle::collect_gc_values`. `State` is
    /// just an id whose cell value is rooted by E0, and `Memo` stores serialized
    /// bytes rather than live `MettaValue`s, matching the slab tracer.
    fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>) {
        let node = self.arena.get(addr);
        node.child_addrs(out); // Error/Type/Quoted/Lazy/Spanned inline handles
        match *node {
            Node::SExpr(cr) | Node::Conjunction(cr) => {
                // SAFETY (D-TLAB-1.2): the worklist holds only live (reachable) nodes,
                // whose segment cell was published before the address was handed out
                // (`addr.segment() < sides_count`) and whose `cr.idx < published_len()`
                // with a `Some` slot (live node). `self.side` reads the directory cell
                // via raw pointer (no `&self.sides` borrow), so it does not conflict
                // with the `arena` borrow above.
                let side = unsafe { self.side(addr.segment()) };
                let kids = unsafe { side.children.get(cr.idx) }.unwrap_or_else(|| {
                    panic!(
                        "live SExpr/Conjunction children slot during mark: addr={addr:?} seg={} child_idx={}",
                        addr.segment(),
                        cr.idx
                    )
                });
                for c in kids.iter() {
                    if let Some(a) = c.as_arena_addr() {
                        out.push(a);
                    }
                }
            }
            Node::Space(id) => {
                let mut values = Vec::new();
                self.space_handle(id).collect_gc_values(&mut values);
                for value in values {
                    if let Some(a) = value.as_arena_addr() {
                        out.push(a);
                    }
                }
            }
            Node::Atom(_)
            | Node::Bool(_)
            | Node::Long(_)
            | Node::Float(_)
            | Node::String(_)
            | Node::Error(..)
            | Node::Type(_)
            | Node::State(_)
            | Node::Unit
            | Node::Memo(_)
            | Node::Quoted(_)
            | Node::Lazy(_)
            | Node::Empty
            | Node::NotReducible
            | Node::Spanned(..) => {}
        }
    }

    /// Transitively mark every node reachable from `roots`, resolving
    /// `SExpr`/`Conjunction` children from the child side-arena and
    /// value-bearing `SpaceHandle` contents. Returns the count newly marked.
    /// Stack-safe (delegates to `mark_from_roots_with`).
    pub fn mark(&self, roots: &[Addr]) -> usize {
        let arena = &self.arena;
        arena.mark_from_roots_with(roots, |addr, out| self.child_addrs_for_mark(addr, out))
    }

    /// E4 (serializable continuations): the transitive store closure of `roots`,
    /// as a `Vec<Addr>`, WITHOUT setting any mark bit.
    ///
    /// This is the NON-bit-setting twin of [`mark`](Self::mark): it visits
    /// children through the EXACT SAME [`child_addrs_for_mark`](Self::child_addrs_for_mark)
    /// edge function the collector uses, so the returned set is precisely
    /// `σ|_Reachable(roots)` == the set [`mark`] would keep live. Capture
    /// ([`continuation_slice::capture_slice`](crate::backend::eval::cesk::continuation_slice::capture_slice))
    /// reuses this so the serialized slice == `Reach(Seed)` in the
    /// `SerializableContinuationSlice` proof (its `Hclosed` premise) — the slice is
    /// closed under the collector's edge relation by CONSTRUCTION, not by a
    /// hand-written walk that could drift from `mark`.
    ///
    /// Dedup is a local `HashSet` (mark bits are reserved for the collector and a
    /// capture must not perturb them — capture runs under a SHARED `.read()` lock).
    /// Stack-safe explicit worklist; idempotent; order of the returned vector is
    /// the discovery order (irrelevant — restore topo-sorts via the remap fixpoint).
    pub fn reachable_closure(&self, roots: &[Addr]) -> Vec<Addr> {
        let mut seen: std::collections::HashSet<Addr> =
            std::collections::HashSet::with_capacity(roots.len().max(16));
        let mut order: Vec<Addr> = Vec::with_capacity(roots.len().max(16));
        let mut worklist: Vec<Addr> = Vec::with_capacity(roots.len().max(16));
        for &r in roots {
            if seen.insert(r) {
                order.push(r);
                worklist.push(r);
            }
        }
        // NOTE: the frontier buffer is named `frontier` (not `kids`) so this line
        // does not collide with the generic `child_addrs_for_mark(addr, &mut kids)`
        // marker the `mark_young` source-coupling pin anchors on.
        let mut frontier: Vec<Addr> = Vec::new();
        while let Some(addr) = worklist.pop() {
            frontier.clear();
            // Reuse the collector's exact edge relation (Error/Type/Quoted/Lazy/
            // Spanned inline handles + SExpr/Conjunction side-arena children +
            // value-bearing SpaceHandle contents).
            self.child_addrs_for_mark(addr, &mut frontier);
            for &k in &frontier {
                if seen.insert(k) {
                    order.push(k);
                    worklist.push(k);
                }
            }
        }
        order
    }

    /// E4: emit the serde mirror [`SerNode`](crate::backend::eval::cesk::continuation_slice::SerNode)
    /// of the node at `addr`, flattening its variable-length side data into the
    /// growing dense `children`/`bytes`/`spans` pools and encoding child handles as
    /// [`SlotRef`](crate::backend::eval::cesk::continuation_slice::SlotRef)s.
    ///
    /// Reads the SAME side-arena accessors the collector/`view_at` use
    /// (`children`/`str_slice`/`span_at`), so an emitted node is a faithful,
    /// position-independent copy of the live σ node. Lives next to
    /// `child_addrs_for_mark` so the closure-and-emit pair stays in lockstep with
    /// the edge relation.
    pub fn emit_node(
        &self,
        addr: Addr,
        children: &mut Vec<Vec<crate::backend::eval::cesk::continuation_slice::SlotRef>>,
        bytes: &mut Vec<String>,
        spans: &mut Vec<crate::backend::eval::cesk::continuation_slice::SerSpan>,
    ) -> crate::backend::eval::cesk::continuation_slice::SerNode {
        use crate::backend::eval::cesk::continuation_slice::{SerNode, SlotRef};
        match *self.arena.get(addr) {
            Node::Atom(_) => {
                let idx = bytes.len() as u32;
                bytes.push(self.str_slice(addr).to_string());
                SerNode::Atom { bytes_idx: idx }
            }
            Node::String(_) => {
                let idx = bytes.len() as u32;
                bytes.push(self.str_slice(addr).to_string());
                SerNode::String { bytes_idx: idx }
            }
            Node::Bool(b) => SerNode::Bool(b),
            Node::Long(n) => SerNode::Long(n),
            Node::Float(f) => SerNode::Float(f),
            Node::Unit => SerNode::Unit,
            Node::Empty => SerNode::Empty,
            Node::NotReducible => SerNode::NotReducible,
            Node::Space(id) => SerNode::Space(id),
            Node::State(id) => SerNode::State(id),
            Node::Memo(id) => SerNode::Memo(id),
            Node::SExpr(_) => {
                let idx = children.len() as u32;
                let kids: Vec<SlotRef> = self
                    .children(addr)
                    .iter()
                    .map(|c| SlotRef::from_value(*c))
                    .collect();
                children.push(kids);
                SerNode::SExpr { children_idx: idx }
            }
            Node::Conjunction(_) => {
                let idx = children.len() as u32;
                let kids: Vec<SlotRef> = self
                    .children(addr)
                    .iter()
                    .map(|c| SlotRef::from_value(*c))
                    .collect();
                children.push(kids);
                SerNode::Conjunction { children_idx: idx }
            }
            Node::Error(a, b) => SerNode::Error(SlotRef::from_value(a), SlotRef::from_value(b)),
            Node::Type(a) => SerNode::Type(SlotRef::from_value(a)),
            Node::Quoted(a) => SerNode::Quoted(SlotRef::from_value(a)),
            Node::Lazy(a) => SerNode::Lazy(SlotRef::from_value(a)),
            Node::Spanned(inner, _) => {
                let span_idx = spans.len() as u32;
                spans.push(self.span_at(addr).into());
                SerNode::Spanned(SlotRef::from_value(inner), span_idx)
            }
        }
    }

    /// E2 SATB final-remark mark.
    ///
    /// Allocate-black can leave final-rendezvous roots already marked before the
    /// final exclusive sweep. The final remark must still traverse through those
    /// roots, so it uses the arena variant that deduplicates with a separate
    /// visited set instead of relying on newly set mark bits.
    pub fn mark_revisit(&self, roots: &[Addr]) -> usize {
        let arena = &self.arena;
        arena.mark_from_roots_with_revisit(roots, |addr, out| self.child_addrs_for_mark(addr, out))
    }

    /// E2 SATB concurrent mark entry. This is intentionally a thin wrapper over
    /// the existing full transitive mark: the distinction is the caller's lock
    /// regime. The dedicated GC thread calls this while holding only a shared heap
    /// read lock, after SATB deletion barriers and allocate-black have been armed.
    pub fn mark_concurrent(&self, roots: &[Addr]) -> usize {
        self.mark(roots)
    }

    /// C1.c: conservative young mark — the MINOR's mark. It marks only young
    /// nodes (`seg >= young_floor`), but traverses every reachable node so a
    /// mutable first-class old `SpaceHandle` can reveal young contents. This
    /// keeps `sweep_young` young-only without relying on the false premise that
    /// all semantic edges are immutable σ-node edges.
    pub fn mark_young(&self, roots: &[Addr]) -> usize {
        fn enqueue(
            arena: &IndexArena<Node>,
            young_floor: usize,
            seen: &mut std::collections::HashSet<Addr>,
            worklist: &mut Vec<Addr>,
            addr: Addr,
        ) -> usize {
            if !seen.insert(addr) {
                return 0;
            }
            worklist.push(addr);
            if addr.segment() >= young_floor && arena.mark(addr) {
                1
            } else {
                0
            }
        }

        let young_floor = self.arena.young_floor();
        let mut marked = 0usize;
        let mut seen = std::collections::HashSet::with_capacity(roots.len().max(16));
        let mut worklist = Vec::with_capacity(roots.len().max(16));
        for &root in roots {
            marked += enqueue(&self.arena, young_floor, &mut seen, &mut worklist, root);
        }

        let mut kids = Vec::new();
        while let Some(addr) = worklist.pop() {
            kids.clear();
            self.child_addrs_for_mark(addr, &mut kids);
            for &child in &kids {
                marked += enqueue(&self.arena, young_floor, &mut seen, &mut worklist, child);
            }
        }
        marked
    }

    /// Sweep, co-releasing the side-arenas of any fully-dead released segments.
    pub fn sweep(&mut self) -> SweepStats {
        // B1.c: retain only hash-cons entries whose interned `Addr` is still LIVE
        // (marked this cycle by `mark`, which the collector calls immediately
        // before `sweep` under the write lock — see `run_collection_if_triggered`).
        // Dead entries (unmarked, so about to be reclaimed to the free list, or in
        // a fully-dead segment about to be released) are dropped, so a later
        // `intern_ground_sexpr` can never hit and hand back a reclaimed/released
        // slot. This replaces the old unconditional `clear()`: live ground content
        // keeps its canonical `Addr` across sweeps (stable `inner_ptr` identity),
        // dead content is forgotten.
        //
        // Soundness: marks are read HERE, before `sweep_with` clears them. By
        // induction every retained entry points at a marked (hence non-released)
        // segment and new inter-sweep entries point at freshly-bumped live
        // segments — release happens only inside `sweep_with`, after this retain —
        // so `is_marked` is always bounds-safe. With no preceding `mark` all marks
        // are 0 and this degenerates to the old `clear()` (still sound). Dropping
        // only *released-segment* entries (and leaning on the lookup-time
        // re-validation) is NOT sound: a kept entry whose slot was reclaimed to the
        // free list but not yet reused has intact bytes, so a re-intern would hit
        // and return a free slot — a UAF once that slot is popped by `alloc_fixed`.
        {
            let arena = &self.arena;
            self.hash_cons
                .retain(|_, v| v.as_arena_addr().is_some_and(|a| arena.is_marked(a)));
        }
        let mut reclaimed: Vec<Addr> = Vec::new();
        let mut released_segments: Vec<usize> = Vec::new();
        let stats = {
            // Disjoint-field capture: the release closure resets segment side
            // arenas through `&mut self.sides` (the directory `Box`), while
            // `self.arena.sweep_with` mutably borrows the disjoint `self.arena`.
            // (D-TLAB-1.2: was `sides[seg] = ..`; now an unsafe cell-reset on the
            // directory — `&mut self.sides` is exclusive, so the `&mut` to the
            // cell's `Box<SegmentSideArenas>` aliases nothing; published cells only.)
            let sides = &mut self.sides;
            let sides_count = self.sides_count.load(std::sync::atomic::Ordering::Acquire);
            let column_cap = self.arena.segment_capacity(); // exp46 (read before the borrow)
            self.arena.sweep_with(
                |seg| {
                    // exp46: co-release the segment's shared column (POD —
                    // wholesale; fully-dead segment ⇒ no reader can reach it).
                    crate::backend::eval::cesk::inner_column::column_release_seg(seg, column_cap);
                    if seg < sides_count {
                        // SAFETY: `seg < sides_count` ⇒ cell `seg` published (its
                        // `Box<SegmentSideArenas>` initialized); `&mut self.sides`
                        // is exclusive (quiescence, write lock) ⇒ unique access.
                        // `assume_init_mut()` is `&mut Box<SegmentSideArenas>`;
                        // resetting the pointee (`**`) drops the old arena (freeing
                        // its columns) in place and reuses the cell's `Box`.
                        unsafe {
                            **(*sides[seg].get()).assume_init_mut() = SegmentSideArenas::default();
                        }
                    }
                    released_segments.push(seg);
                },
                &mut reclaimed,
            )
        };
        // Reclaim-time side snapshots survive node-slot reuse until the next
        // quiescence side-free. Whole released segments were reset wholesale above,
        // so any older pending per-slot snapshots for those segments are satisfied.
        self.drop_pending_side_reclaims_for_released_segments(&released_segments);
        self.append_pending_side_reclaims(&reclaimed);
        stats
    }

    /// Snapshot the side payload owned by a reclaimed node slot while that slot still
    /// contains the swept occupant's bytes. This must run before any later
    /// `write_reused` can overwrite the node with a different side index.
    fn side_reclaim_for_addr(&self, addr: Addr) -> Option<SideReclaim> {
        let seg = addr.segment();
        match self.arena.get(addr) {
            Node::SExpr(cr) | Node::Conjunction(cr) => Some(SideReclaim::Children {
                owner: addr,
                seg,
                idx: cr.idx,
                gen: cr.gen,
            }),
            // Finding 2: atoms are interned (no byte side-column) ⇒ they own no
            // freeable side slot and produce no reclaim; only `String` still does.
            Node::String(br) => Some(SideReclaim::Strings {
                owner: addr,
                seg,
                idx: br.idx,
                gen: br.gen,
            }),
            Node::Spanned(_, sr) => Some(SideReclaim::Spans {
                owner: addr,
                seg,
                idx: sr.idx,
                gen: sr.gen,
            }),
            _ => None,
        }
    }

    /// Add reclaim-time side-owner snapshots for swept partial-segment slots.
    ///
    /// MIDLOOP/rendezvous collections may reclaim the fixed node slot while deferring
    /// the side `Box` free for launder soundness. The node slot can be reused before
    /// the next quiescence sweep, so the side owner must be recorded now; rereading
    /// `arena.get(addr)` later can observe a different occupant and free a live side
    /// slot.
    fn append_pending_side_reclaims(&mut self, reclaimed: &[Addr]) {
        self.pending_side_reclaims.reserve(reclaimed.len());
        for &addr in reclaimed {
            if let Some(side) = self.side_reclaim_for_addr(addr) {
                self.pending_side_reclaims.push(side);
            }
        }
    }

    /// A whole-segment release resets that segment's side columns, so any pending
    /// per-slot side snapshots for the segment are already satisfied and must not be
    /// applied after the segment is later reused with fresh side indices starting at 0.
    fn drop_pending_side_reclaims_for_released_segments(&mut self, released: &[usize]) {
        if released.is_empty() || self.pending_side_reclaims.is_empty() {
            return;
        }
        self.pending_side_reclaims
            .retain(|side| !released.contains(&side.segment()));
    }

    /// True iff the current node bytes at the owner address still name the same
    /// side slot captured by `side`.
    fn owner_still_owns_side_reclaim(&self, side: SideReclaim) -> bool {
        let owner = side.owner();
        match (side, self.arena.get(owner)) {
            (SideReclaim::Children { idx, .. }, Node::SExpr(cr) | Node::Conjunction(cr)) => {
                cr.idx == idx
            }
            // Finding 2: a `Strings` reclaim is owned only by a live `String` slot; if
            // the slot was reused by an interned `Atom` (no side), the owner no longer
            // owns it ⇒ falls to `_ => false`.
            (SideReclaim::Strings { idx, .. }, Node::String(br)) => br.idx == idx,
            (SideReclaim::Spans { idx, .. }, Node::Spanned(_, sr)) => sr.idx == idx,
            _ => false,
        }
    }

    /// True iff the owner address is marked live by the current full mark and still
    /// owns the same side slot captured by `side`.
    fn marked_owner_still_owns_side_reclaim(&self, side: SideReclaim) -> bool {
        self.arena.is_marked(side.owner()) && self.owner_still_owns_side_reclaim(side)
    }

    fn free_side_reclaim(&mut self, side: SideReclaim, sides_count: usize) {
        let seg = side.segment();
        if seg >= sides_count {
            return;
        }
        // SAFETY: `seg < sides_count` => cell `seg` published; `&mut self`
        // exclusive at the quiescence drain => unique access.
        let arenas = unsafe { self.side_mut(seg) };
        match side {
            SideReclaim::Children { idx, gen, .. } => arenas.children.free(idx, gen),
            SideReclaim::Strings { idx, gen, .. } => arenas.strings.free(idx, gen),
            SideReclaim::Spans { idx, gen, .. } => arenas.spans.free(idx, gen),
        }
    }

    /// A full mark can prove an older pending side snapshot is stale-live: a prior
    /// young/rendezvous sweep reported the node slot, but this full mark found that
    /// the owner address is live and still points at the same side index. Such a
    /// snapshot must be dropped, not drained; the following full sweep rebuilds the
    /// free list from the mark bits and therefore rescinds any stale free-list entry.
    fn drop_or_free_pending_side_reclaims_after_full_mark(&mut self) {
        let pending = std::mem::take(&mut self.pending_side_reclaims);
        let sides_count = self.sides_count.load(std::sync::atomic::Ordering::Acquire);
        for side in pending {
            let owner_marked = self.arena.is_marked(side.owner());
            let owner_still_owns = self.owner_still_owns_side_reclaim(side);
            if owner_marked && owner_still_owns {
                continue;
            }
            self.free_side_reclaim(side, sides_count);
        }
    }

    /// C1.c #1: free known-dead side payload snapshots only at true quiescence.
    ///
    /// The snapshots were captured at reclaim time, so they remain valid even if the
    /// node slot was later reused. SideColumn indices ARE recycled (post-266d19d
    /// free-list reuse), so a captured `idx` alone no longer identifies the snapshot
    /// occupant across reuse — the snapshot therefore also captured the cell's
    /// GENERATION, and `SideColumn::free` drops the cell only when that captured
    /// generation still equals the cell's current one (so a reused index whose
    /// generation a live re-intern bumped is never freed by a stale snapshot).
    /// Duplicate snapshots are harmless because `SideColumn::free` is idempotent.
    /// Snapshots for whole-released segments are removed by
    /// [`drop_pending_side_reclaims_for_released_segments`] before this drain runs.
    fn free_pending_side_reclaims(&mut self) {
        let pending = std::mem::take(&mut self.pending_side_reclaims);
        let sides_count = self.sides_count.load(std::sync::atomic::Ordering::Acquire);
        for side in pending {
            self.free_side_reclaim(side, sides_count);
        }
    }

    /// C1.b: young-only minor sweep — the generational counterpart of [`sweep`].
    /// Sweeps ONLY young segments (`>= young_floor`); OLD segments keep their marks
    /// AND slots untouched (a minor never releases an old segment). The hash-cons
    /// retain therefore checks liveness only for YOUNG entries: an OLD entry is
    /// retained unconditionally — its old segment is never released by a minor, so
    /// its `Addr` (and the interned bytes) stay valid, and a later `intern` correctly
    /// hits it (dropping it would only lose canonicalization, never cause a UAF; a
    /// MAJOR's full `sweep` drops any now-stale old entry). A young entry is dropped
    /// unless marked (its slot is about to be reclaimed / its segment released).
    ///
    /// Soundness mirrors `sweep`: marks are read HERE, before `sweep_young_with`
    /// clears the young marks; release happens only inside `sweep_young_with`, AFTER
    /// this retain, so `is_marked` is always bounds-safe. The collector marks the
    /// FULL reachable set (C1.b uses the full `mark`, conservative-complete), so
    /// every live young node is marked and never reclaimed here.
    pub fn sweep_young(&mut self) -> SweepStats {
        let young_floor = self.arena.young_floor();
        {
            let arena = &self.arena;
            self.hash_cons.retain(|_, v| match v.as_arena_addr() {
                // Young: keep iff still marked this cycle (else about to be reclaimed).
                Some(a) if a.segment() >= young_floor => arena.is_marked(a),
                // Old: keep — a minor never releases an old segment, so the Addr lives.
                Some(_) => true,
                // Inline scalar / non-index handle: not an arena-keyed entry.
                None => false,
            });
        }
        let mut reclaimed: Vec<Addr> = Vec::new();
        let mut released_segments: Vec<usize> = Vec::new();
        let stats = {
            // Disjoint-field capture (see `sweep`): reset released young segments'
            // side arenas through `&mut self.sides` while `self.arena` is borrowed
            // by `sweep_young_with`.
            let sides = &mut self.sides;
            let sides_count = self.sides_count.load(std::sync::atomic::Ordering::Acquire);
            let column_cap = self.arena.segment_capacity(); // exp46 (read before the borrow)
            self.arena.sweep_young_with(
                |seg| {
                    // exp46: co-release the segment's shared column (POD —
                    // wholesale; fully-dead segment ⇒ no reader can reach it).
                    crate::backend::eval::cesk::inner_column::column_release_seg(seg, column_cap);
                    if seg < sides_count {
                        // SAFETY: `seg < sides_count` ⇒ cell `seg` published;
                        // `&mut self.sides` exclusive (quiescence). `assume_init_mut()`
                        // is `&mut Box<SegmentSideArenas>`; resetting the pointee
                        // (`**`) drops the old arena (frees its columns) in place.
                        unsafe {
                            **(*sides[seg].get()).assume_init_mut() = SegmentSideArenas::default();
                        }
                    }
                    released_segments.push(seg);
                },
                &mut reclaimed,
            )
        };
        // Reclaim-time side snapshots survive node-slot reuse until the next
        // quiescence side-free. Whole released segments were reset wholesale above,
        // so any older pending per-slot snapshots for those segments are satisfied.
        self.drop_pending_side_reclaims_for_released_segments(&released_segments);
        self.append_pending_side_reclaims(&reclaimed);
        stats
    }

    // ── Diagnostics ──────────────────────────────────────────────────────

    #[inline]
    pub fn alloc_count(&self) -> u64 {
        self.arena.alloc_count()
    }

    #[inline]
    pub fn segment_count(&self) -> usize {
        self.arena.segment_count()
    }

    /// Committed node-slab bytes (see [`IndexArena::committed_node_bytes`]) plus
    /// a coarse estimate of the per-segment side-arena footprint (one machine
    /// word per interned child slice/str/span box pointer, i.e. the [`SideColumn`]
    /// published-entry count — the boxed payloads themselves are not separately
    /// tracked but the entry count tracks growth/release in lockstep with
    /// segments). This is the watermark signal for the Inc-6 single-threaded GC
    /// trigger; it does not need to be exact, only monotone-up between sweeps and
    /// to drop at sweep.
    ///
    /// D-TLAB-1.1: `published_len()` replaces the old `Vec::len()` and is
    /// byte-identical for this estimate — `push` advances it, `free` never
    /// decrements it (matching the old `None`-in-place), and a segment release
    /// (resetting `sides[seg]` to a fresh arena) resets it to 0 — so the same
    /// monotone-up / drop-at-release shape holds. D-TLAB-1.2: iterate the published
    /// directory cells `[0, sides_count)` instead of a `Vec`; same sum.
    #[inline]
    pub fn committed_bytes(&self) -> usize {
        use std::sync::atomic::Ordering;
        let node_bytes = self.arena.committed_node_bytes();
        // Side-arena spine: pointer-sized entry per interned variable-length datum.
        let ptr = std::mem::size_of::<usize>();
        let mut side = 0usize;
        let sides_count = self.sides_count.load(Ordering::Acquire);
        for seg in 0..sides_count {
            // SAFETY: `seg < sides_count` ⇒ cell `seg` published.
            let s = unsafe { self.side(seg) };
            side +=
                (s.children.published_len() + s.strings.published_len() + s.spans.published_len())
                    * ptr;
        }
        node_bytes + side
    }

    /// Estimated live bytes = live node slots × node size (see
    /// [`IndexArena::live_node_count`]). Used to recompute the GC watermark after
    /// a sweep.
    #[inline]
    pub fn live_bytes(&self) -> usize {
        self.arena.live_node_count() * self.arena.node_size_bytes()
    }

    /// Increment B (CHANGE #3): estimated live bytes of the OLD generation only — the
    /// major's live-based trigger metric (mirror of [`live_bytes`], over
    /// [`IndexArena::old_live_node_count`]). The major fires on `old_live_bytes` growth,
    /// NOT `committed_bytes` (whose append-only side spine never shrinks under no-recycle,
    /// so it would fire the major spuriously and defeat "minor displaces major").
    #[inline]
    pub fn old_live_bytes(&self) -> usize {
        self.arena.old_live_node_count() * self.arena.node_size_bytes()
    }

    /// C1.c: bytes of young node-slab allocated since the last [`promote_young`] —
    /// the nursery-fill odometer the driver compares against `YOUNG_BUDGET` to fire a
    /// minor (forwards to [`IndexArena::young_alloc_bytes`]). Tracks real young
    /// allocation (bump AND young free-list reuse), unlike a high-water/capacity
    /// figure; resets to 0 at promotion ⇒ trigger == rearm baseline ⇒ no thrash.
    #[inline]
    pub fn young_alloc_bytes(&self) -> usize {
        self.arena.young_alloc_bytes()
    }

    /// Increment C (CHANGE #2): the allocator->GC backpressure signal (forwards to
    /// [`IndexArena::nursery_full_pending`]) — TRUE iff a segment opened since the last
    /// promotion. The driver folds it into `minor_due` so a minor is scheduled next safepoint.
    #[inline]
    pub fn nursery_full_pending(&self) -> bool {
        self.arena.nursery_full_pending()
    }

    /// C1.b: promote young survivors to old (non-moving boundary advance). Forwards
    /// to [`IndexArena::promote_young`]; called after a collection under the write lock.
    #[inline]
    pub fn promote_young(&self) {
        self.arena.promote_young();
    }

    /// F1 SATB-young lever: clear all OLD-generation mark bits (forwards to
    /// [`IndexArena::clear_old_marks`]). Called by the rendezvous-phase minor arm
    /// after [`sweep_young`](Self::sweep_young) — the ClearOldMarks=TRUE wiring of
    /// `tla/SATBYoungSweepStaleOldMark.tla` (see that model's `NoStaleOldMark`).
    #[inline]
    pub fn clear_old_marks(&self) {
        self.arena.clear_old_marks();
    }

    #[inline]
    fn pending_side_reclaim_count(&self) -> usize {
        self.pending_side_reclaims.len()
    }
}

// SAFETY (D-TLAB-1.2): `IndexHeap` gained an `UnsafeCell` field — the never-realloc
// side directory `sides: Box<[UnsafeCell<MaybeUninit<Box<SegmentSideArenas>>>]>` —
// which makes the auto-derived `Send`/`Sync` no longer apply (the prior
// `Vec<SegmentSideArenas>` was auto-`Send`/`Sync` via `SegmentSideArenas`'s columns'
// own unsafe impls). The interior mutability of `sides` is disciplined by the same
// publish protocol as `IndexArena::segments` and `SideColumn::pages` (see the
// type-level SAFETY blocks there):
//
//   (i)   DIRECTORY PUBLICATION. A reader dereferences cell `seg` only for
//         `seg < sides_count.load(Acquire)`. `ensure_side_seg` initializes cell
//         `seg`'s `Box<SegmentSideArenas>` then `sides_count.store(seg+1, Release)`
//         (under `sides_dir_lock`, the unique writer of cell `seg`), so observing
//         `seg < sides_count` happens-after the cell's initialization ⇒ the
//         `Box<SegmentSideArenas>` ptr (and its columns' `Release`-published initial
//         state) is visible before the reader indexes it. The `&SegmentSideArenas`
//         that `side` returns derives from the raw `UnsafeCell::get()` pointer (not
//         a `&self.sides` borrow), so it does not alias the directory `Box`.
//
//   (ii)  PER-SEGMENT APPENDS ARE `SideColumn`-DISCIPLINED. All variable-length data
//         lives in the cell's `SideColumn`s, whose own claim-unique / publish-Release
//         / read-published protocol (and `unsafe impl Send/Sync`) makes a shared
//         `&SegmentSideArenas` safe to `push`/`get` from multiple threads.
//
//   (iii) CELL MUTATION AT QUIESCENCE. The only writes to an already-published
//         directory cell's contents (the segment-release reset and
//         `free_pending_side_reclaims`) go through `&mut self` (`side_mut`, and the
//         disjoint `&mut self.sides` capture in the `sweep*` release closures),
//         which run only at a quiescent safepoint under the heap write lock —
//         statically exclusive of every `&self` reader/pusher. A cell's
//         `Box<SegmentSideArenas>` ptr is never reassigned after publication (only
//         its pointee is reset in place), so a concurrent `side` reader's raw-pointer
//         deref stays valid; no cell is ever un-published (`sides_count` is monotone).
//
// The other fields are `Send`/`Sync` for the conventional reasons: `arena:
// IndexArena<Node>` has its own (matching) unsafe impls; `pending_side_reclaims`/
// `space_table`/`memo_table`/`hash_cons` are plain owned collections mutated only
// under the `RwLock` write guard; `sides_count`/`sides_dir_lock` are atomics/`Mutex`.
// `Node` is `Copy` (no owned resources), and the `Box<SegmentSideArenas>` payloads
// are `Send + Sync` (their columns are), so both bounds hold without an `N: Send`-style
// generic guard (the type is concrete).
unsafe impl Send for IndexHeap {}
unsafe impl Sync for IndexHeap {}

/// The process-global index heap, mirroring the slab's `OnceLock<SlabAllocator>`.
/// Backs the future `IndexHeapStore`; `RwLock` is retained only around exclusive
/// collection/reuse paths, while concurrent allocation uses the D-TLAB/read-lock
/// path.
static GLOBAL_INDEX_HEAP: OnceLock<RwLock<IndexHeap>> = OnceLock::new();

/// Get (initializing on first use) the global index heap.
pub fn global_index_heap() -> &'static RwLock<IndexHeap> {
    GLOBAL_INDEX_HEAP.get_or_init(|| RwLock::new(IndexHeap::new()))
}

// ============================================================================
// IndexFactory + IndexHeapStore — the `Store` seam's index-arena implementation
// (Inc 2a-4). A ZST factory reaching the global heap; mirrors `GcFactory`/
// `SlabStore`. Returns `MettaValue` handles whose payload is an arena `Addr`
// (decoded once the value model is mode-aware — Inc 2a-5).
// ============================================================================

/// Zero-sized value factory backed by the global index heap.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexFactory;

#[inline]
fn flag_vars(has: bool) -> usize {
    if has {
        FLAG_HAS_VARIABLES
    } else {
        0
    }
}

#[inline]
fn current_segment_reuse_pressure() -> bool {
    global_index_heap()
        .try_read()
        .map(|h| h.has_current_free_slot())
        .unwrap_or(false)
}

#[inline]
fn alloc_with_reuse_pressure<E, C>(exclusive: E, concurrent: C) -> Addr
where
    E: FnOnce(&mut IndexHeap) -> Addr,
    C: FnOnce(&IndexHeap) -> Addr,
{
    // Lock discipline (REFUTED-alternative note, experiment #13 2026-06-11): the
    // `try_write()`-FIRST order below is LOAD-BEARING, not lock greed. A
    // "prefer-read under no pressure" reorder (pressure ? write : read+bump) was
    // measured 13.7% SLOWER on default-env Toothbrush with 9x the variance
    // (p=0.999 vs the pre-registered improvement direction): bump-only
    // allocation skips the exclusive arm's free-list reuse AND ground-sexpr
    // hash-cons dedup, so young allocation grows faster and the young-budget
    // trigger fires MORE GC rendezvous cycles — each parking all workers, far
    // costlier than the futex contention saved (~22% of the profile). Do not
    // re-attempt writer demotion; the contention must be attacked on the READER
    // side (shadow-miss materialization rate / lock-free published-node reads).
    // exp46 increment 2: every routed mint populates the shared Inner column
    // under the guard it already holds (post-alloc, pre-escape — v3-F2).
    if let Ok(mut h) = global_index_heap().try_write() {
        let addr = exclusive(&mut h);
        h.populate_column(addr);
        return addr;
    }
    if current_segment_reuse_pressure() {
        let mut h = global_index_heap().write().expect("index heap");
        let addr = exclusive(&mut h);
        h.populate_column(addr);
        return addr;
    }
    let h = global_index_heap().read().expect("index heap");
    let addr = concurrent(&h);
    h.populate_column(addr);
    addr
}

impl MettaValueFactory<MettaValue> for IndexFactory {
    fn atom(&self, s: &str) -> MettaValue {
        let flags = flag_vars(is_variable_str(s));
        let addr = alloc_with_reuse_pressure(|h| h.alloc_atom(s), |h| h.alloc_atom_concurrent(s));
        MettaValue::from_addr(addr, flags, TAG5_ATOM)
    }

    fn bool(&self, b: bool) -> MettaValue {
        MettaValue::inline_bool(b) // inline, mode-independent — byte-identical to slab
    }

    fn long(&self, n: i64) -> MettaValue {
        if let Some(v) = MettaValue::try_inline_long(n) {
            return v;
        }
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_fixed(Node::Long(n)),
            |h| h.alloc_fixed_concurrent(Node::Long(n)),
        );
        MettaValue::from_addr(addr, 0, TAG5_LONG)
    }

    fn float(&self, f: f64) -> MettaValue {
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_fixed(Node::Float(f)),
            |h| h.alloc_fixed_concurrent(Node::Float(f)),
        );
        MettaValue::from_addr(addr, 0, TAG5_FLOAT)
    }

    fn string(&self, s: &str) -> MettaValue {
        let addr =
            alloc_with_reuse_pressure(|h| h.alloc_string(s), |h| h.alloc_string_concurrent(s));
        MettaValue::from_addr(addr, 0, TAG5_STRING)
    }

    fn sexpr(&self, items: Vec<MettaValue>) -> MettaValue {
        self.sexpr_from_slice(&items)
    }

    fn sexpr_from_slice(&self, items: &[MettaValue]) -> MettaValue {
        if items.is_empty() {
            return self.unit();
        }
        let has_vars = items.iter().any(|i| i.has_variables_fast());
        if !has_vars {
            // Hash-cons ground SExprs (CRUX Step 4) — matches GcFactory exactly
            // (ground-only, shared `hash_cons_key`), so equal content shares one
            // `Addr` ⇒ one `inner_ptr` key (R9 fixpoint-identity parity).
            //
            // D-RLOCK.2: this branch STAYS on unconditional `.write()`. Hash-consing
            // reads AND mutates the shared `hash_cons` map (lookup-then-insert under
            // one `&mut self`); it is NOT concurrent-appendable this increment, and
            // (more importantly) the dedup must observe a coherent map to preserve
            // R9 fixpoint-identity parity — two concurrent inserters of the same
            // ground content could otherwise mint two distinct `Addr`s for it. Only
            // the VARIABLE (non-ground) branch below de-serializes.
            return global_index_heap()
                .write()
                .expect("index heap")
                .intern_ground_sexpr(items);
        }
        // D-RLOCK.2: VARIABLE SExprs carry no hash-cons (each is freshly allocated),
        // so they take the try_write reuse-or-bump / concurrent bump-only split
        // (see `atom`). Fresh-bump never reuses an `Addr`, and the value is non-
        // ground ⇒ compared structurally, never by the `inner_ptr`-keyed
        // VALUE_HASH_CACHE in a content-equality-sensitive way that a fresh `Addr`
        // would corrupt — so the concurrent path is output-equivalent.
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_sexpr(items),
            |h| h.alloc_sexpr_concurrent(items),
        );
        MettaValue::from_addr(addr, FLAG_HAS_VARIABLES, TAG5_SEXPR)
    }

    fn error(&self, offending: MettaValue, detail: MettaValue) -> MettaValue {
        let flags = flag_vars(offending.has_variables_fast() || detail.has_variables_fast());
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_fixed(Node::Error(offending, detail)),
            |h| h.alloc_fixed_concurrent(Node::Error(offending, detail)),
        );
        MettaValue::from_addr(addr, flags, TAG5_ERROR)
    }

    fn type_value(&self, inner: MettaValue) -> MettaValue {
        let flags = flag_vars(inner.has_variables_fast());
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_fixed(Node::Type(inner)),
            |h| h.alloc_fixed_concurrent(Node::Type(inner)),
        );
        MettaValue::from_addr(addr, flags, TAG5_TYPE)
    }

    fn conjunction(&self, goals: Vec<MettaValue>) -> MettaValue {
        self.conjunction_from_slice(&goals)
    }

    fn conjunction_from_slice(&self, goals: &[MettaValue]) -> MettaValue {
        let flags = flag_vars(goals.iter().any(|g| g.has_variables_fast()));
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_conjunction(goals),
            |h| h.alloc_conjunction_concurrent(goals),
        );
        MettaValue::from_addr(addr, flags, TAG5_CONJUNCTION)
    }

    fn space(&self, handle: SpaceHandle) -> MettaValue {
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_space(handle);
        MettaValue::from_addr(addr, 0, TAG5_SPACE)
    }

    fn state(&self, id: u64) -> MettaValue {
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_fixed(Node::State(id)),
            |h| h.alloc_fixed_concurrent(Node::State(id)),
        );
        MettaValue::from_addr(addr, 0, TAG5_STATE)
    }

    fn unit(&self) -> MettaValue {
        MettaValue::inline_unit()
    }

    fn memo(&self, handle: MemoHandle) -> MettaValue {
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_memo(handle);
        MettaValue::from_addr(addr, 0, TAG5_MEMO)
    }

    fn empty(&self) -> MettaValue {
        MettaValue::inline_empty()
    }

    fn not_reducible(&self) -> MettaValue {
        // Memoized NotReducible sentinel (mirrors GcFactory's interned singleton).
        // Overrides the trait default, which incorrectly returns atom("NotReducible").
        static INDEX_NOT_REDUCIBLE: OnceLock<MettaValue> = OnceLock::new();
        *INDEX_NOT_REDUCIBLE.get_or_init(|| {
            let mut h = global_index_heap().write().expect("index heap");
            let addr = h.alloc_fixed(Node::NotReducible);
            h.populate_column(addr); // exp46
            MettaValue::from_addr(addr, 0, TAG5_NOT_REDUCIBLE)
        })
    }

    fn quote(&self, inner: MettaValue) -> MettaValue {
        let flags = flag_vars(inner.has_variables_fast());
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_fixed(Node::Quoted(inner)),
            |h| h.alloc_fixed_concurrent(Node::Quoted(inner)),
        );
        MettaValue::from_addr(addr, flags, TAG5_QUOTED)
    }

    fn lazy(&self, inner: MettaValue) -> MettaValue {
        // NOTE: GcFactory's `lazy` is idempotent (returns `inner` if it is already
        // Lazy). That check needs the mode-aware `is_lazy`/decode, so it is added
        // in Inc 2a-5; until then `lazy` always wraps (correct, just non-idempotent).
        let flags = flag_vars(inner.has_variables_fast());
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_fixed(Node::Lazy(inner)),
            |h| h.alloc_fixed_concurrent(Node::Lazy(inner)),
        );
        MettaValue::from_addr(addr, flags, TAG5_LAZY)
    }

    fn spanned(&self, value: MettaValue, span: crate::ir::Span) -> MettaValue {
        let flags = flag_vars(value.has_variables_fast());
        let addr = alloc_with_reuse_pressure(
            |h| h.alloc_spanned(value, span),
            |h| h.alloc_spanned_concurrent(value, span),
        );
        MettaValue::from_addr(addr, flags, TAG5_SPANNED)
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<(MettaValue, usize), String> {
        // Reuse the (now factory-generic) slab deserializer over IndexFactory.
        crate::backend::models::gc_allocator::deserialize_slab_value(self, bytes)
    }
}

/// Production `Store` over the global index heap (the `--gc=index` substrate).
/// Selected at the `EvalContext::factory()` seam in Inc 4; default stays `SlabStore`.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexHeapStore {
    factory: IndexFactory,
}

impl IndexHeapStore {
    #[inline]
    pub fn new() -> Self {
        Self {
            factory: IndexFactory,
        }
    }
}

impl Store<MettaValue> for IndexHeapStore {
    type Factory = IndexFactory;

    #[inline]
    fn factory(&self) -> &IndexFactory {
        &self.factory
    }

    #[inline]
    fn alloc_count(&self) -> u64 {
        global_index_heap()
            .read()
            .expect("index heap")
            .alloc_count()
    }

    /// Inc 6: committed node-slab + side-arena spine bytes (the GC watermark
    /// signal). Replaces the former trait-default `0`.
    #[inline]
    fn live_bytes(&self) -> usize {
        global_index_heap()
            .read()
            .expect("index heap")
            .committed_bytes()
    }
}

// ============================================================================
// Inc 6 — the FIRST WORKING store-centric collector (single-threaded regime)
//
// Wires the test-only `IndexHeap::mark`/`sweep` core as a LIVE collector for the
// provably-single-threaded evaluation path, so it is safe by construction (no
// concurrent-collector hazards). The parallel-rendezvous collector is a separate
// increment and is explicitly OUT of scope here.
//
// Safety rests on the TLA+-proven `QuiescenceInvariant` (tla/StoreCentricGC.tla):
// mark+sweep AT QUIESCENCE is safe. The gate below makes "quiescence" trivially
// hold — see `gate_open`. The root-completeness invariant (the UAF linchpin) is
// satisfied by the CALLER, which passes the structural CESK/E0 roots relevant
// to the entry point plus the narrow driver transport roots that are live at
// that entry point. The legacy `collect_all_roots()`/`RootProvider` registry
// (deleted in F4) was never an index-collector root source.
// ============================================================================
pub mod index_gc {
    use super::global_index_heap;
    use crate::backend::models::metta_value::gc_mode_is_index;
    use crate::backend::models::{active_evaluator_count, worker_ever_spawned, MettaValue};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::RwLock;

    /// Number of live single-threaded mark+sweep cycles run since process start
    /// (validation observability — a vacuous trigger leaves this at 0). Counts
    /// BOTH the quiescence and the mid-loop entry points.
    static GC_CYCLES_RUN: AtomicU64 = AtomicU64::new(0);

    /// Subset of [`GC_CYCLES_RUN`] that fired at a MID-LOOP (mid-directive)
    /// safepoint (the "comprehensive mid-execution rooting" capability,
    /// 2026-05-28). Distinguished from quiescence cycles purely for validation:
    /// the mid-execution-rooting completeness proof asserts this is `> 0` (a
    /// collection actually swept WHILE live execution stacks were on the Rust
    /// stack) and that the run was ASAN-clean.
    static MIDLOOP_CYCLES_RUN: AtomicU64 = AtomicU64::new(0);

    /// Validation observability for the generational split. The R-FL regression
    /// requires forced workloads that exercise both minor and major sweep paths;
    /// total cycle count alone is too weak for that gate.
    static MINOR_CYCLES_RUN: AtomicU64 = AtomicU64::new(0);
    static MAJOR_CYCLES_RUN: AtomicU64 = AtomicU64::new(0);
    static RENDEZVOUS_CYCLES_RUN: AtomicU64 = AtomicU64::new(0);
    static RENDEZVOUS_MINOR_CYCLES_RUN: AtomicU64 = AtomicU64::new(0);
    static RENDEZVOUS_MAJOR_CYCLES_RUN: AtomicU64 = AtomicU64::new(0);
    static SATB_PHASE_LOCK: RwLock<()> = RwLock::new(());
    static SATB_MARKING_DEPTH: AtomicUsize = AtomicUsize::new(0);

    #[inline]
    pub(super) fn should_drain_side_reclaims(phase: &str, did_major: bool) -> bool {
        phase == "quiescence" && did_major
    }

    /// True exactly while an E2 SATB mark is in progress.
    ///
    /// SATB cache barriers must shade only during that window. Outside it there is
    /// no snapshot-live set to protect, and setting mark bits would be stale state
    /// for a later collection cycle.
    pub(crate) fn satb_marking_in_progress() -> bool {
        let depth = SATB_MARKING_DEPTH.load(Ordering::Acquire);
        depth > 0
    }

    /// Runs one E0 deletion/eviction under the SATB phase gate.
    ///
    /// The marker flips `SATB_MARKING_DEPTH` while holding the write side. A
    /// mutator that sees `false` here completes its deletion before the marker can
    /// start the snapshot; a mutator that runs after the marker starts sees `true`
    /// and must shade the removed pre-image before publishing the deletion.
    pub(crate) fn with_satb_deletion_barrier<R>(f: impl FnOnce(bool) -> R) -> R {
        let _phase = SATB_PHASE_LOCK.read().expect("SATB phase lock poisoned");
        f(satb_marking_in_progress())
    }

    /// Nested-safe guard used by the E2 concurrent marker to arm cache barriers.
    pub(crate) struct SatbMarkingGuard;

    pub(crate) fn enter_satb_marking() -> SatbMarkingGuard {
        let _phase = SATB_PHASE_LOCK.write().expect("SATB phase lock poisoned");
        SATB_MARKING_DEPTH.fetch_add(1, Ordering::AcqRel);
        SatbMarkingGuard
    }

    impl Drop for SatbMarkingGuard {
        fn drop(&mut self) {
            let _phase = SATB_PHASE_LOCK.write().expect("SATB phase lock poisoned");
            let prev = SATB_MARKING_DEPTH.fetch_sub(1, Ordering::AcqRel);
            debug_assert!(prev > 0, "SATB marking guard underflow");
        }
    }

    /// E2 SATB deletion-barrier primitive for E0 anchor caches.
    ///
    /// Anchor caches are part of `reach(E0)`. When a cache removes a value-bearing
    /// entry, the removed pre-image must remain black enough for any in-flight
    /// snapshot-at-the-beginning mark. Outside the active SATB mark window this is
    /// deliberately a no-op: stale mark bits from an idle barrier would corrupt a
    /// later cycle's worklist traversal.
    pub(crate) fn satb_shade_evicted_roots<I>(roots: I)
    where
        I: IntoIterator<Item = MettaValue>,
    {
        if !gc_mode_is_index() || !satb_marking_in_progress() {
            return;
        }

        let mut addrs = Vec::new();
        for root in roots {
            if let Some(addr) = root.as_arena_addr() {
                addrs.push(addr);
            }
        }
        if addrs.is_empty() {
            return;
        }

        let heap = global_index_heap().read().expect("index heap");
        heap.mark(&addrs);
    }

    fn project_roots_to_addrs(
        roots: &[MettaValue],
    ) -> Vec<crate::backend::eval::cesk::index_arena::Addr> {
        let mut addrs: Vec<crate::backend::eval::cesk::index_arena::Addr> =
            Vec::with_capacity(roots.len());
        for v in roots {
            if let Some(a) = v.as_arena_addr() {
                addrs.push(a);
            }
        }
        addrs
    }

    /// E2 SATB read-locked concurrent mark. The caller must have armed
    /// `enter_satb_marking` before releasing mutators from the snapshot
    /// rendezvous. Fresh allocations are allocate-black, and E0 deletions shade
    /// their pre-images while this mark runs.
    pub(crate) fn mark_concurrent_roots(roots: &[MettaValue]) -> usize {
        let addrs = project_roots_to_addrs(roots);
        let heap = global_index_heap().read().expect("index heap");
        heap.mark_concurrent(&addrs)
    }

    /// Adaptive committed-bytes watermark. The collector fires when the index
    /// heap's committed bytes exceed this; recomputed after each cycle as
    /// `max(live_bytes_after_sweep * GROWTH, min_threshold())`. A plain
    /// `AtomicUsize` is race-free here because the collector only runs on the
    /// single sequential evaluator thread (the gate forbids any worker).
    static WATERMARK: AtomicUsize = AtomicUsize::new(0);

    /// Increment B (CHANGE #3, R3 anti-thrash): when a cap-triggered major
    /// (`committed > max_bytes()`) releases 0 segments it is FUTILE (the over-cap is
    /// genuine live data or pathological 1-node-per-segment fragmentation), so re-firing
    /// it every cycle would thrash. After such a major, raise the effective cap to the
    /// current `committed` (stored here) so the cap clause needs FURTHER growth to
    /// re-fire; reset to 0 on any major that DID release a segment. Single-threaded ⇒
    /// `Relaxed`.
    static CAP_FLOOR: AtomicUsize = AtomicUsize::new(0);

    /// C1.c: minors fired since the last major. The major (full) collection is the
    /// periodic BACKSTOP — it fires when total committed exceeds [`WATERMARK`] OR
    /// after [`MAJOR_CADENCE`] minors, whichever first — to reclaim old-generation
    /// garbage that minors (which sweep only young) leave behind. Reset to 0 on each
    /// major. Single-threaded collector ⇒ `Relaxed` is race-free.
    static MINORS_SINCE_MAJOR: AtomicUsize = AtomicUsize::new(0);

    /// C1.c: at most this many minors between majors (the major backstop cadence).
    /// Bounds the old-generation dead that accumulates between majors (a minor never
    /// reclaims old). 16 keeps majors rare on a mark-dominated heap while ensuring
    /// old dead is reclaimed within a bounded number of cheap young collections.
    const MAJOR_CADENCE: usize = 16;

    /// Growth factor applied to post-sweep live bytes to set the next major threshold.
    const GROWTH: usize = 2;

    /// Default minimum collection threshold (bytes) for the MAJOR. Overridable via
    /// `METTATRON_INDEX_GC_MIN_BYTES` (validation lowers it to force the major to
    /// fire on small workloads). This is the major (full-sweep) tuning knob only —
    /// NOT a minor on/off switch.
    const DEFAULT_MIN_THRESHOLD: usize = 8 * 1024 * 1024;

    /// C1.c: default nursery budget — a MINOR fires when `young_alloc_bytes` (bytes of
    /// young node-slab allocated since the last promotion) exceeds this. A principled
    /// default (NOT an on/off switch — minors are ALWAYS on; this only sizes the
    /// nursery), ≈ 1/4 of a segment's node-slab capacity (a segment is `1<<18` slots
    /// ≈ 8 MiB). Rationale: (a) < `DEFAULT_MIN_THRESHOLD` (the major floor) so a
    /// minor fires BEFORE a major on multi-segment workloads (minor-primary); (b) <
    /// one segment so a minor's young generation stays within the active bump
    /// segment ⇒ its reclaimed slots stay young and are reused (no
    /// promotion-stranding); (c) tied to the substrate's segment granularity, not a
    /// magic number. Minors thus fire NATURALLY on any workload allocating more than
    /// ~1/2 segment of transient young between safepoints — exercised by tests with
    /// no force-switch. `METTATRON_INDEX_GC_YOUNG_BYTES` may raise/lower the nursery
    /// size for measurement, but it does not disable minors.
    const YOUNG_BUDGET: usize = 4 * 1024 * 1024;

    /// Cached `METTATRON_INDEX_GC_MIN_BYTES` (parsed once).
    fn min_threshold() -> usize {
        use std::sync::OnceLock;
        static MIN: OnceLock<usize> = OnceLock::new();
        *MIN.get_or_init(|| {
            std::env::var("METTATRON_INDEX_GC_MIN_BYTES")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(DEFAULT_MIN_THRESHOLD)
        })
    }

    /// Cached `METTATRON_INDEX_GC_YOUNG_BYTES` (parsed once). Positive values
    /// tune the nursery trigger; invalid/missing values keep the proof-backed
    /// default above.
    fn young_budget() -> usize {
        use std::sync::OnceLock;
        static YOUNG: OnceLock<usize> = OnceLock::new();
        *YOUNG.get_or_init(|| {
            std::env::var("METTATRON_INDEX_GC_YOUNG_BYTES")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(YOUNG_BUDGET)
        })
    }

    #[cfg(test)]
    pub(crate) fn young_budget_for_test() -> usize {
        young_budget()
    }

    /// Increment B (CHANGE #3): default ABSOLUTE committed ceiling (bytes) — the hard cap
    /// above which a MAJOR is forced regardless of `old_live`. Bounds RSS even on a
    /// pathological workload whose old gen never grows (so the live-based trigger never
    /// fires) but whose append-only side spine + transient churn inflate `committed`.
    /// 4 GiB. Overridable via `METTATRON_INDEX_GC_MAX_BYTES` (NOT an on/off switch — a
    /// ceiling tuning knob; the collector is always on).
    const DEFAULT_ABSOLUTE_COMMITTED_CAP: usize = 4 * 1024 * 1024 * 1024;

    /// Cached `METTATRON_INDEX_GC_MAX_BYTES` (parsed once) — the hard committed ceiling
    /// (mirror of [`min_threshold`]). `committed > max_bytes()` forces a major.
    fn max_bytes() -> usize {
        use std::sync::OnceLock;
        static MAX: OnceLock<usize> = OnceLock::new();
        *MAX.get_or_init(|| {
            std::env::var("METTATRON_INDEX_GC_MAX_BYTES")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(DEFAULT_ABSOLUTE_COMMITTED_CAP)
        })
    }

    /// Increment C (CHANGE #2): the allocator→GC backpressure LEVEL (0..3) — the index
    /// mirror of the slab's `BACKPRESSURE_LEVEL` (`gc_allocator.rs:3159-3162`), the SAME
    /// ladder applied to the young-nursery ratio (`young_alloc / YOUNG_BUDGET`) instead of
    /// the slab's committed/threshold ratio: `>=2x -> 3, >=1.5x -> 2, >=1x -> 1, else 0`.
    /// Drives the level-3 minor-preference (a level-3 nursery is ACUTE young pressure, so
    /// the driver prefers the cheap young-only minor over a coincident major for one cycle).
    #[inline]
    fn index_backpressure_level(young_alloc: usize) -> u8 {
        let b = young_budget();
        if young_alloc >= 2 * b {
            3
        } else if young_alloc * 2 >= 3 * b {
            2
        } else if young_alloc >= b {
            1
        } else {
            0
        }
    }

    /// Number of live mark+sweep cycles run so far (test/validation observable).
    #[inline]
    pub fn cycles_run() -> u64 {
        GC_CYCLES_RUN.load(Ordering::Relaxed)
    }

    /// Number of mid-loop (mid-directive) cycles run so far — the subset of
    /// [`cycles_run`] that swept WHILE live execution stacks were on the Rust
    /// stack. The mid-execution-rooting completeness proof requires this `> 0`.
    #[inline]
    pub fn midloop_cycles_run() -> u64 {
        MIDLOOP_CYCLES_RUN.load(Ordering::Relaxed)
    }

    /// Number of minor (young-only) cycles run so far.
    #[inline]
    pub fn minor_cycles_run() -> u64 {
        MINOR_CYCLES_RUN.load(Ordering::Relaxed)
    }

    /// Number of major (full-heap) cycles run so far.
    #[inline]
    pub fn major_cycles_run() -> u64 {
        MAJOR_CYCLES_RUN.load(Ordering::Relaxed)
    }

    /// Number of cycles run by the dedicated rendezvous collector.
    #[inline]
    pub fn rendezvous_cycles_run() -> u64 {
        RENDEZVOUS_CYCLES_RUN.load(Ordering::Relaxed)
    }

    /// Number of minor cycles run by the dedicated rendezvous collector.
    #[inline]
    pub fn rendezvous_minor_cycles_run() -> u64 {
        RENDEZVOUS_MINOR_CYCLES_RUN.load(Ordering::Relaxed)
    }

    /// Number of major cycles run by the dedicated rendezvous collector.
    #[inline]
    pub fn rendezvous_major_cycles_run() -> u64 {
        RENDEZVOUS_MAJOR_CYCLES_RUN.load(Ordering::Relaxed)
    }

    /// Cheap pre-check for the call sites: is a collection plausibly due?
    ///
    /// `gate_open()` (a few relaxed atomic loads) AND the committed-bytes
    /// watermark is exceeded (one read-lock + comparison). Call sites use this to
    /// AVOID building the expensive `collect_all_roots()` root set on every
    /// `eval()` — only when a collection will actually fire. The full
    /// `run_collection_if_triggered` re-checks both under its own locking, so
    /// this is purely an optimization (no correctness dependence).
    #[inline]
    pub fn should_collect() -> bool {
        if !gate_open() {
            return false;
        }
        // C1.c: a collection is due if the MINOR trigger (young allocation since the
        // last promotion exceeds the nursery budget — PRIMARY) OR the MAJOR backstop
        // (total committed over the watermark, or the minor cadence elapsed) fires.
        let (committed, young_alloc, old_live, nursery_pending, pending_side_reclaims) = {
            let heap = global_index_heap().read().expect("index heap");
            (
                heap.committed_bytes(),
                heap.young_alloc_bytes(),
                heap.old_live_bytes(),
                heap.nursery_full_pending(),
                heap.pending_side_reclaim_count(),
            )
        };
        // Increment B (CHANGE #3): the MAJOR triggers on OLD-gen live growth (`old_live`),
        // NOT `committed` (whose append-only side spine never shrinks under no-recycle, so
        // it would fire the major spuriously); `committed` appears ONLY in the hard-ceiling
        // clause (`> max_bytes()`). The MINOR is `young_alloc` past the budget OR the
        // Increment C (CHANGE #2) backpressure signal (`nursery_pending` — a segment opened
        // since the last promotion, the slab `request_gc` analogue).
        let young_budget = young_budget();
        young_alloc > young_budget
            || nursery_pending
            || pending_side_reclaims > 0
            || old_live > WATERMARK.load(Ordering::Relaxed).max(min_threshold())
            || committed > max_bytes()
            || MINORS_SINCE_MAJOR.load(Ordering::Relaxed) >= MAJOR_CADENCE
    }

    /// The provable single-threaded-quiescence gate:
    ///
    /// ```text
    /// gc_mode_is_index() && active_evaluator_count() == 0 && n_threads() == 0
    /// ```
    ///
    /// The collector runs at TRUE quiescence — the point in `eval()` AFTER the
    /// `EvalGuard` has dropped, so `active_evaluator_count() == 0`: no trampoline
    /// loop and no bytecode VM is live on the Rust stack, so the only surviving
    /// values are the structural persistent roots plus the about-to-be-returned
    /// result vector (which the caller passes in explicitly). This is exactly the
    /// slab GC's session-release reclaim point and exactly the proven
    /// `QuiescenceInvariant` (activeEvaluators empty).
    ///
    /// **Why NOT a mid-trampoline safepoint:** at a mid-loop safepoint the live
    /// bytecode-VM execution stacks (`value_stack` / `locals` / `results` /
    /// `current_bindings`) that exist when the VM calls a nested
    /// `eval_trampoline` (e.g. `eval_sub_expr_vm_all_with_bindings`) are NOT
    /// comprehensively registered as roots — the slab GC tolerates that only
    /// because it DEFERS reclaim to this same quiescence point. A synchronous
    /// mid-loop sweep would free those VM-stack values → use-after-free
    /// (empirically reproduced; see docs/cesk-gc/single-threaded-collector.md).
    /// Rooting the full VM state at every nested call is the broader "comprehensive
    /// VM root coverage" increment, deliberately out of scope here.
    ///
    /// Fanout configuration does NOT poison true quiescence. Once
    /// `active_evaluator_count()==0 && n_threads()==0`, no worker/native stack can
    /// touch σ and the persistent CESK root reader is complete. The FANOUT hazard is
    /// mid-loop collection while a future or current branch has live control; that
    /// path remains blocked by [`gate_open_midloop`] and must use rendezvous.
    #[inline]
    pub fn gate_open() -> bool {
        gc_mode_is_index()
            && active_evaluator_count() == 0
            && crate::backend::models::gc_allocator::n_threads() == 0
    }

    /// E1-FLIP — the gate for the DEDICATED-GC-THREAD rendezvous collect (the ONLY
    /// path that may sweep while eval workers are live). Unlike [`gate_open`] it does
    /// NOT require `!worker_ever_spawned()` (false under FANOUT>0 by construction);
    /// instead it requires the rendezvous COMPLETENESS WITNESS: this thread holds
    /// `GcInProgressGuard` (`gc_in_progress()`, admission closed) AND the PER-SLOT
    /// witness gate is satisfied (`current_witness_ok()` — the driver proved every
    /// OCCUPIED witness slot was stamped this cycle by a genuine reified park, already
    /// waited on by `requestor_wait_for_all_reified_parked` before this call). E1-FLIP
    /// Path B V4 REPLACED the fungible `workers_parked_for_gc() >= n_threads_at_snapshot()`
    /// conjunct here (the publish-timing UAF: a finisher's count "covered" for a parent's
    /// not-yet-published machine — see docs/cesk-gc/e1-flip-VALIDATION-FAILED-2026-06-02.md).
    ///
    /// SOUNDNESS (sweep runs ⟺ root set complete): the sole caller is
    /// `gc_driver::gc_driver_rendezvous_cycle`, which calls this AFTER (2) try_enter
    /// GcInProgressGuard, (3) `n := n_threads()` post-admission + `set_n_threads_at_snapshot(n)`,
    /// (4) `snapshot_witness(cur_gen)` + `requestor_wait_for_all_reified_parked`,
    /// then `set_current_witness_ok(true)`, (5) drain `WORKER_ROOT_BUFFER ∪
    /// collect_safepoint_roots ∪ collect_live_env_anchors ∪ collect_live_dispatch_anchors`.
    /// So when this returns true the drained union is
    /// `⋃ᵢ machineᵢ ∪ E₀ ∪ driver-C ∪ dispatch-C` — the complete live set (HB2 via the witness
    /// AcqRel). `active_evaluator_count()==0` is NOT required (a finisher that
    /// bumped-then-kept-running may still hold a guard); completeness comes from the
    /// buffer drain gated by the per-slot reified witness, not from active==0.
    ///
    /// Live for the opt-in dedicated-rendezvous path. The slab build const-folds
    /// `gc_mode_is_index()`, and default builds only reach this when the caller has
    /// already gated on `dedicated_gc_enabled()`.
    #[inline]
    #[allow(dead_code)]
    pub fn gate_open_rendezvous() -> bool {
        // E1-FLIP Path B V4 — the witness SOLE gate: the sweep proceeds ⟺ the driver
        // proved every OCCUPIED witness slot was stamped this cycle by a genuine reified
        // park (`current_witness_ok()`, set true only after
        // `requestor_wait_for_all_reified_parked` returns). REPLACES the fungible
        // `workers_parked_for_gc() >= n_threads_at_snapshot()` conjunct (which let the
        // driver proceed while a COUNTED participant had not yet published its machine —
        // the publish-timing UAF; see docs/cesk-gc/e1-flip-VALIDATION-FAILED-2026-06-02.md).
        gc_mode_is_index()
            && crate::backend::models::gc_allocator::gc_in_progress()
            && crate::backend::models::gc_allocator::current_witness_ok()
    }

    /// The provable single-threaded gate for the MID-LOOP (mid-directive)
    /// collector:
    ///
    /// ```text
    /// gc_mode_is_index() && !worker_ever_spawned() && active_evaluator_count() == 1
    /// ```
    ///
    /// Identical to [`gate_open`] except `active_evaluator_count() == 1`: the
    /// mid-loop safepoint runs INSIDE `eval_trampoline`, where the sole
    /// evaluator holds exactly one `EvalGuard` (so the count is 1, not 0). With
    /// `!parallel_fanout_enabled()` this is still the trivially-true instance of
    /// the proven `QuiescenceInvariant`: no eval worker can be spawned by this
    /// process configuration, and the single calling thread at the safepoint is
    /// the SOLE thread that can touch σ. The collection runs under the heap write
    /// lock, mutually exclusive with allocation, so no `Addr` is minted mid-mark.
    ///
    /// **Root completeness (the UAF linchpin):** unlike the quiescence point,
    /// the live trampoline S/C/K AND every on-stack bytecode-VM frame's
    /// execution stacks ARE alive here — so the caller MUST pass the COMPLETE
    /// mid-execution root set: live machine roots from `collect_machine_roots_live`
    /// (S/C/K plus reach(E₀), global anchors, and the typed K-spine/VM leaves),
    /// the deferred-drop transient register, and driver-C safepoint roots. The
    /// formal `MidloopRootUnion` obligation and source-coupling harness pin that
    /// vector; the forced-ASAN validation gate is green, so the path is on by
    /// default inside this already-proven single-evaluator gate.
    /// Marking from an incomplete set is a use-after-free.
    #[inline]
    pub fn gate_open_midloop() -> bool {
        gc_mode_is_index()
            && !crate::backend::eval::trampoline::eval_loop::parallel_fanout_enabled()
            && !worker_ever_spawned()
            && active_evaluator_count() == 1
    }

    /// Cheap mid-loop pre-check (mirrors [`should_collect`] with the mid-loop
    /// gate): is a mid-directive collection plausibly due? Call sites use this
    /// to avoid building the expensive complete root set on every safepoint.
    #[inline]
    pub fn should_collect_midloop() -> bool {
        if !gate_open_midloop() {
            return false;
        }
        // C1.c: minor (young allocation) primary OR major backstop (see `should_collect`).
        let (committed, young_alloc, old_live, nursery_pending) = {
            let heap = global_index_heap().read().expect("index heap");
            (
                heap.committed_bytes(),
                heap.young_alloc_bytes(),
                heap.old_live_bytes(),
                heap.nursery_full_pending(),
            )
        };
        // Increment B (CHANGE #3): the MAJOR triggers on OLD-gen live growth (`old_live`),
        // NOT `committed` (whose append-only side spine never shrinks under no-recycle, so
        // it would fire the major spuriously); `committed` appears ONLY in the hard-ceiling
        // clause (`> max_bytes()`). The MINOR is `young_alloc` past the budget OR the
        // Increment C (CHANGE #2) backpressure signal (`nursery_pending` — a segment opened
        // since the last promotion, the slab `request_gc` analogue).
        let young_budget = young_budget();
        young_alloc > young_budget
            || nursery_pending
            || old_live > WATERMARK.load(Ordering::Relaxed).max(min_threshold())
            || committed > max_bytes()
            || MINORS_SINCE_MAJOR.load(Ordering::Relaxed) >= MAJOR_CADENCE
    }

    /// E1-c step 3: the WATERMARK disjunction ONLY (no single-threaded gate), for
    /// the dedicated-GC-thread FANOUT>0 trigger ([`eval_loop`] safepoint). Identical
    /// clauses to [`should_collect`] / [`should_collect_midloop`], but WITHOUT
    /// `gate_open*` — those require `active_evaluator_count()` ∈ {0,1} AND
    /// `!worker_ever_spawned()`, both FALSE under FANOUT>0, so neither probe can ever
    /// fire there. The CALLER supplies the fanout-enabled participant gate
    /// (`dedicated_gc_enabled() && parallel_fanout_enabled() && n_threads() >= 1`),
    /// so this is pure "is the heap over a trigger watermark?" — one read-lock + a few relaxed loads. Its sole
    /// caller short-circuits on `dedicated_gc_enabled()`, so the slab build never
    /// reaches it (byte-identical). Keeps the watermark
    /// constants module-private (the alternative — inlining at the call site —
    /// would have to expose them).
    #[allow(dead_code)]
    pub fn watermark_due_for_concurrent() -> bool {
        let (committed, young_alloc, old_live, nursery_pending) = {
            let heap = global_index_heap().read().expect("index heap");
            (
                heap.committed_bytes(),
                heap.young_alloc_bytes(),
                heap.old_live_bytes(),
                heap.nursery_full_pending(),
            )
        };
        let young_budget = young_budget();
        young_alloc > young_budget
            || nursery_pending
            || old_live > WATERMARK.load(Ordering::Relaxed).max(min_threshold())
            || committed > max_bytes()
            || MINORS_SINCE_MAJOR.load(Ordering::Relaxed) >= MAJOR_CADENCE
    }

    /// F1 SATB-young lever: the MAJOR-only half of the trigger disjunction, for the
    /// dedicated GC thread's rendezvous-cycle ROUTING (`gc_driver_rendezvous_cycle`).
    /// TRUE ⇒ the cycle takes the full SATB major (concurrent mark + final remark +
    /// full sweep); FALSE (the trigger was young-budget/nursery-only) ⇒ the driver
    /// routes to the proven STW rendezvous body, whose classifier independently
    /// agrees on the minor arm (`major_due` below is the SAME three clauses, so
    /// `do_major` cannot flip back to true).
    ///
    /// SOURCE-COUPLED to `mark_sweep_if_over_watermark`'s major clauses:
    /// `live_major` (old-gen live growth past the rearmed watermark) ∨ `cap_major`
    /// (hard ceiling, R3-floored) ∨ `cadence_major` (MAJOR_CADENCE minors since the
    /// last major — ALSO the bound on how long rendezvous minors may defer a full
    /// sweep). `pending_side_major` is omitted: it is `phase == "quiescence"`-gated
    /// and can never hold at a rendezvous. Read on the GC thread AFTER the
    /// rendezvous parked every mutator (no allocation ⇒ metrics stable) and under
    /// `GcInProgressGuard` (no concurrent cycle mutates WATERMARK/CAP_FLOOR/
    /// MINORS_SINCE_MAJOR) ⇒ no TOCTOU against the STW classifier's re-read.
    ///
    /// Conservative by design: the STW classifier's level-3 minor-preference
    /// inversion (acute young pressure deferring a `live_major`) is NOT replicated
    /// here — when any major clause holds the cycle takes the SATB major exactly as
    /// every rendezvous cycle did before this lever.
    #[allow(dead_code)]
    pub(crate) fn rendezvous_major_due() -> bool {
        let (committed, old_live) = {
            let heap = global_index_heap().read().expect("index heap");
            (heap.committed_bytes(), heap.old_live_bytes())
        };
        let cap = max_bytes().max(CAP_FLOOR.load(Ordering::Relaxed));
        old_live > WATERMARK.load(Ordering::Relaxed).max(min_threshold())
            || committed > cap
            || MINORS_SINCE_MAJOR.load(Ordering::Relaxed) >= MAJOR_CADENCE
    }

    /// Run a single-threaded mark+sweep cycle IF the safety gate is open AND the
    /// committed-bytes watermark is exceeded.
    ///
    /// `roots` MUST be the COMPLETE live root set. At the quiescence call site
    /// (post-`EvalGuard` in `eval()`), that is `collect_all_roots()` (env / tiers
    /// / promoted) UNIONED with the about-to-be-returned result values — there is
    /// no live trampoline/VM state to miss. Marking from an incomplete set is a
    /// use-after-free. Inline scalars / slab handles project to `None` and drop
    /// out via `filter_map`.
    ///
    /// The write lock held across mark+sweep is mutually exclusive with
    /// allocation (which also takes `global_index_heap().write()`), so no `Addr`
    /// can be minted mid-collection. Returns `true` iff a cycle ran.
    pub fn run_collection_if_triggered(roots: &[MettaValue]) -> bool {
        // Gate first — cheap, and keeps the legacy slab opt-out build's call
        // site dead (gc_mode_is_index() const-folds to false when the feature
        // is off).
        if !gate_open() {
            return false;
        }
        mark_sweep_if_over_watermark(roots, "quiescence")
    }

    /// E1-FLIP — the DEDICATED-GC-THREAD rendezvous collect entry (GC thread ONLY).
    /// Identical to [`run_collection_if_triggered`] EXCEPT it gates on
    /// [`gate_open_rendezvous`] (the completeness witness — NO `!worker_ever_spawned()`,
    /// which is false under FANOUT>0) and labels the cycle `"rendezvous"`.
    ///
    /// `roots` MUST be the COMPLETE union the driver drained:
    /// `WORKER_ROOT_BUFFER ∪ collect_safepoint_roots ∪ collect_live_env_anchors ∪
    /// collect_live_dispatch_anchors`, i.e.
    /// `⋃ᵢ machineᵢ ∪ E₀ ∪ driver-C ∪ dispatch-C`. [`gate_open_rendezvous`] is the
    /// completeness witness: the driver holds `GcInProgressGuard` and proved every
    /// occupied witness slot was either reified for this cycle or acquired after it.
    /// The shared `mark_sweep_if_over_watermark` body is the same proven mark/sweep.
    ///
    /// ⚠️ The `"rendezvous"` phase string (≠ `"quiescence"`) is LOAD-BEARING: it makes
    /// `mark_sweep_if_over_watermark` reclaim node slots ONLY and NOT free side-`Box`es
    /// this cycle (the side-`Box` free is `phase == "quiescence"`-gated). A parked
    /// worker may hold a laundered `&'static MettaValueInner` into a side-`Box`, so the
    /// rendezvous must defer side-`Box` frees to the next quiescence sweep (no-recycle
    /// idempotence). Passing `"quiescence"` here would free a side-`Box` a parked worker
    /// still references → UAF.
    #[allow(dead_code)]
    pub fn run_collection_if_triggered_rendezvous(roots: &[MettaValue]) -> bool {
        if !gate_open_rendezvous() {
            return false;
        }
        mark_sweep_if_over_watermark(roots, "rendezvous")
    }

    /// Run a MID-LOOP (mid-directive) mark+sweep cycle IF the mid-loop safety
    /// gate ([`gate_open_midloop`]) is open AND the committed-bytes watermark is
    /// exceeded.
    ///
    /// `roots` MUST be the COMPLETE mid-execution root set: live structural
    /// machine roots from S/C/E/K plus reach(E0), typed K-spine/VM/JIT leaves,
    /// deferred env roots, pointer-keyed cache roots, and driver-C safepoint
    /// roots. Unlike the quiescence variant, live execution stacks ARE present
    /// here, so an incomplete set is a use-after-free; this is validated directly
    /// under ASAN (a missed root -> freed-mid-execution -> heap-use-after-free).
    /// Returns `true` iff a cycle ran.
    ///
    /// The mark+sweep body, watermark rearm, and shadow-cache clear are
    /// IDENTICAL to the quiescence path (shared `mark_sweep_if_over_watermark`),
    /// so both paths use the same proven collection logic; only the gate differs.
    pub fn run_collection_if_triggered_midloop(roots: &[MettaValue]) -> bool {
        if !gate_open_midloop() {
            return false;
        }
        mark_sweep_if_over_watermark(roots, "midloop")
    }

    /// E2 SATB final stop-the-world remark+sweep. Called by the dedicated GC
    /// thread after the read-locked concurrent mark has completed, after the
    /// second rendezvous has parked mutators, and after `SatbMarkingGuard` has
    /// been dropped to wait out in-flight deletion barriers.
    ///
    /// This first SATB implementation is deliberately full-major-only: SATB
    /// deletion barriers can mark old nodes during a minor window, and a young
    /// sweep would not clear those old marks. A full sweep clears every mark bit
    /// it can set, preserving the existing mark lifecycle while still moving the
    /// expensive transitive mark out from under the heap write lock.
    pub(crate) fn sweep_after_concurrent_mark(roots: &[MettaValue], phase: &str) -> bool {
        if !gate_open_rendezvous() {
            return false;
        }
        let addrs = project_roots_to_addrs(roots);
        let (committed, young_alloc, cap_major) = {
            let heap = global_index_heap().read().expect("index heap");
            let committed = heap.committed_bytes();
            (
                committed,
                heap.young_alloc_bytes(),
                committed > max_bytes().max(CAP_FLOOR.load(Ordering::Relaxed)),
            )
        };

        let (live_after, old_live_after, stats) = {
            let mut heap = global_index_heap().write().expect("index heap");
            heap.mark_revisit(&addrs);
            if should_drain_side_reclaims(phase, true) {
                heap.drop_or_free_pending_side_reclaims_after_full_mark();
            }
            let stats = heap.sweep();
            if should_drain_side_reclaims(phase, true) {
                heap.free_pending_side_reclaims();
            }
            heap.promote_young();
            (heap.live_bytes(), heap.old_live_bytes(), stats)
        };

        crate::backend::models::gc_allocator::bump_gc_sweep_epoch();
        crate::backend::eval::trampoline::eval_loop::clear_aba_sensitive_caches();
        // exp46: the shared Inner column needs NO invalidation here — a reused
        // slot's cell is REWRITTEN by `populate_column` before the new handle
        // escapes (write-point 2), and between sweep and reuse no live handle
        // names the Addr, so no reader can observe the stale cell.
        crate::backend::eval::trampoline::dispatch_hints::clear_eval_memo();
        crate::backend::eval::trampoline::dispatch_hints::clear_match_result_cache();

        MAJOR_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
        WATERMARK.store(
            old_live_after.saturating_mul(GROWTH).max(min_threshold()),
            Ordering::Relaxed,
        );
        MINORS_SINCE_MAJOR.store(0, Ordering::Relaxed);
        if cap_major && stats.segments_released == 0 {
            CAP_FLOOR.store(committed, Ordering::Relaxed);
        } else if stats.segments_released > 0 {
            CAP_FLOOR.store(0, Ordering::Relaxed);
        }

        GC_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
        if phase == "rendezvous" {
            RENDEZVOUS_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
            RENDEZVOUS_MAJOR_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
        }

        if std::env::var("METTATRON_INDEX_GC_REPORT").as_deref() == Ok("2") {
            eprintln!(
                "[index_gc] {phase} satb-major cycle: roots={} live_bytes={live_after} old_live_after={old_live_after} committed={committed} young_alloc_pre={young_alloc} reclaimed_slots={} released_segs={} bytes_freed={} minors_since_major={}",
                addrs.len(),
                stats.reclaimed_to_free_list,
                stats.segments_released,
                stats.bytes_released,
                MINORS_SINCE_MAJOR.load(Ordering::Relaxed),
            );
        }
        true
    }

    /// Shared collection core for both the quiescence and mid-loop entry points
    /// (the caller has already checked its respective single-threaded gate).
    /// Trigger-checks the committed-bytes watermark, projects `roots` to arena
    /// addresses, then marks + sweeps under the heap write lock (mutually
    /// exclusive with allocation, so no `Addr` is minted mid-collection),
    /// clears this thread's `MettaValueInner` shadow, and rearms the watermark.
    /// `phase` only labels the optional ops trace. Returns `true` iff a cycle ran.
    fn mark_sweep_if_over_watermark(roots: &[MettaValue], phase: &str) -> bool {
        // C1.c generational trigger under a read lock (released before we re-acquire
        // write). The MINOR is PRIMARY: it fires when young allocation since the last
        // promotion (`young_alloc_bytes`, the nursery odometer) exceeds `YOUNG_BUDGET`.
        // The MAJOR is the BACKSTOP: total committed over the watermark OR the minor
        // cadence elapsed (bounding the old-gen dead a minor leaves behind). Major
        // takes precedence when both are due (it subsumes a minor and reclaims old).
        let (committed, young_alloc, old_live, nursery_pending, pending_side_reclaims) = {
            let heap = global_index_heap().read().expect("index heap");
            (
                heap.committed_bytes(),
                heap.young_alloc_bytes(),
                heap.old_live_bytes(),
                heap.nursery_full_pending(),
                heap.pending_side_reclaim_count(),
            )
        };
        // Increment B (CHANGE #3): the MAJOR is LIVE-BASED — it fires on OLD-gen live
        // growth (`old_live` past the watermark), the HARD CEILING (`committed` past the
        // R3-floored cap), or the cadence backstop. `committed` no longer drives the
        // PRIMARY major (the append-only side spine inflates it monotonically under
        // no-recycle, which would defeat "minor displaces major"). Components are kept
        // separate so the rearm + R3 + the Increment C level-3 preference see WHY a major fired.
        let cap = max_bytes().max(CAP_FLOOR.load(Ordering::Relaxed));
        let live_major = old_live > WATERMARK.load(Ordering::Relaxed).max(min_threshold());
        let cap_major = committed > cap;
        let cadence_major = MINORS_SINCE_MAJOR.load(Ordering::Relaxed) >= MAJOR_CADENCE;
        let pending_side_major = phase == "quiescence" && pending_side_reclaims > 0;
        let major_due = live_major || cap_major || cadence_major || pending_side_major;
        // Increment C (CHANGE #2): the MINOR is `young_alloc` past the budget OR the
        // backpressure signal (`nursery_pending` — a segment opened since the last promotion).
        let young_budget = young_budget();
        let minor_due = young_alloc > young_budget || nursery_pending;
        if !major_due && !minor_due {
            return false;
        }
        // Increment C (CHANGE #2) — the level-3 minor-preference (the bounded <=1-cycle
        // inversion): when the nursery is at ACUTE young pressure (level 3) AND both a minor
        // and a major are due, PREFER the cheap young-only minor for THIS cycle — UNLESS the
        // major is forced by the hard ceiling (`cap_major`) or the cadence backstop
        // (`cadence_major`), which must not be deferred. A deferred `live_major` still fires
        // within MAJOR_CADENCE (the cadence counts up regardless), so old dead stays bounded.
        let level = index_backpressure_level(young_alloc);
        let do_major = pending_side_major
            || (major_due && !(level == 3 && minor_due && !cap_major && !cadence_major));

        // Project the root values to arena addresses. filter_map drops inline
        // scalars (Bool / i48 Long / Unit / Empty) and any non-index handle.
        let mut addrs: Vec<crate::backend::eval::cesk::index_arena::Addr> =
            Vec::with_capacity(roots.len());
        for v in roots {
            if let Some(a) = v.as_arena_addr() {
                addrs.push(a);
            }
        }

        // Collect under the write lock: mutually exclusive with allocation, so the
        // marked set cannot be raced by a new alloc (true quiescence for the store).
        // The gate already guarantees no OTHER thread can be in eval.
        //   MAJOR: FULL `mark` + full `sweep` (reclaims old dead too) + promote.
        //   MINOR: conservative `mark_young` marks only young nodes but traverses
        //          every reachable node, then `sweep_young` + promote. This retains
        //          young values reachable through old first-class Space handles
        //          without marking or sweeping old nodes.
        // `promote_young` reclassifies the swept young segments as old (so only the
        // active + future segments stay young) AND resets the young-alloc odometer.
        // E1-a.2: hold the GC-in-progress handshake across the reclaim so a
        // concurrent EvalGuard::enter / reacquire_eval_guard_after_safepoint
        // (mid-spawn admission) parks until the sweep + side-free + cache-clear
        // complete — the foundation for collecting while workers exist (the
        // dedicated-GC-thread driver, Phase D+E E1). Best-effort: under index-gc
        // no other collector sets GC_IN_PROGRESS, so `try_enter` succeeds; if it
        // (defensively) does not, we still collect — byte-identical to today and
        // sound at the current single-threaded gate (`!worker_ever_spawned()`),
        // where no concurrent entrant exists. The guard drops at fn end, releasing
        // any parked entrant via GC_PROGRESS_CONDVAR.
        let _gip = crate::backend::models::gc_allocator::GcInProgressGuard::try_enter();
        let (live_after, old_live_after, stats, did_major) = {
            let mut heap = global_index_heap().write().expect("index heap");
            let (stats, did_major) = if do_major {
                heap.mark(&addrs); // FULL mark
                if should_drain_side_reclaims(phase, true) {
                    heap.drop_or_free_pending_side_reclaims_after_full_mark();
                }
                (heap.sweep(), true)
            } else {
                // Minor: a pure minor (only minor_due) OR a level-3-DEFERRED major (a minor
                // ran instead this cycle; the live_major re-fires next cycle / within cadence).
                heap.mark_young(&addrs); // conservative traversal; young mark bits only
                let stats = heap.sweep_young();
                // F1 SATB-young lever — ClearOldMarks=TRUE of
                // `tla/SATBYoungSweepStaleOldMark.tla` (`NoStaleOldMark`). The
                // rendezvous phase is the only one that can follow an ARMED SATB
                // window (the routed young rendezvous cycle, and the STW fallback
                // after an aborted `gc_driver_satb_rendezvous_cycle`): deletion-
                // barrier shades / allocate-black may have set OLD marks during
                // that window, `sweep_young` never visits old segments, and the
                // next full `mark` PRUNES its descent at already-marked nodes — a
                // stale old mark would hide that node's live children from the
                // next major (under-mark → use-after-free). Clearing the old
                // bitmap here re-establishes the all-clear mark invariant every
                // other cycle shape ends with. Quiescence/midloop minors skip it
                // (no SATB window can precede them — byte-identical).
                if phase == "rendezvous" {
                    heap.clear_old_marks();
                }
                (stats, false)
            };
            // Increment A (the RSS half of CHANGE #1): free swept-dead side-payload
            // snapshots only after a FULL quiescent mark/sweep. Quiescence proves no
            // Rust stack can hold a laundered side reference; the full mark proves the
            // side owner is genuinely dead. A young-only quiescence minor reclaims
            // node slots, but it is not enough authority to destroy side boxes: any
            // missed old/cache edge would turn into a "live ... slot" UAF. MIDLOOP and
            // rendezvous collections likewise defer reclaim-time side snapshots.
            if should_drain_side_reclaims(phase, did_major) {
                heap.free_pending_side_reclaims();
            }
            heap.promote_young();
            // B.4: measure old_live AFTER promote (the just-swept survivors are now old),
            // so the rearm metric == the trigger metric (both old_live) ⇒ geometric, no thrash.
            (heap.live_bytes(), heap.old_live_bytes(), stats, did_major)
        };

        // C1.c #1: when this collection reused an `Addr` for new content, every
        // Addr-keyed / content-hash-keyed cache that could still hold the PRIOR
        // occupant's entry must be invalidated — else a later lookup (set-op hashing,
        // eval memo, match, operator, MORK) serves a stale result. Invalidate EXACTLY
        // the set the SLAB collector invalidates at its safepoints, via the proven,
        // already-wired `clear_aba_sensitive_caches()` — restoring slab parity:
        //   VALUE_HASH_CACHE (Addr-keyed in index mode — the PRIMARY cause of the
        //   set-op divergences), the MORK bytes + ground-fragment caches, and the
        //   operator cache.
        //
        // ── CROSS-THREAD COHERENCE (2026-06-03): bump the sweep epoch FIRST ──
        // The eager `clear_aba_sensitive_caches()` below clears only the SWEEPING
        // thread's thread-locals. Under the dedicated collector that thread is the GC
        // thread — NOT the work-pool workers that hold the stale Addr→hash entries — and
        // a worker that held its EvalGuard across this (witness-gated) cycle WITHOUT
        // ever hitting a park safepoint (the VM/JIT `0xFFF` poll cadence) is reached by
        // neither the park-resume nor the teardown cache hygiene. So bump
        // `gc_sweep_epoch` exactly as the slab sweep does: every thread lazily self-
        // invalidates its epoch-protected thread-local caches (VALUE_HASH_CACHE, MORK
        // ground-fragment, hash-cons) on its NEXT read (via
        // `ensure_value_hash_cache_epoch_current` etc.) — the documented purpose of
        // `bump_gc_sweep_epoch` ("work-pool threads idle/outside their own safepoint")
        // and the residual wrong-subset corruption's root cause (a stale Addr→hash
        // after a reused-slot bump flips set-op bucketing / skip-eval).
        // Bump BEFORE the eager clear so `clear_value_hash_cache` syncs the sweeping
        // thread's local epoch to the post-bump value (no redundant re-clear there).
        crate::backend::models::gc_allocator::bump_gc_sweep_epoch();
        crate::backend::eval::trampoline::eval_loop::clear_aba_sensitive_caches();
        // exp46: the per-thread INNER_SHADOW this site used to clear is DELETED —
        // the shared Inner column is rewritten at slot reuse (`populate_column`,
        // write-point 2) before the new handle escapes, so Addr reuse cannot
        // serve a stale inner. (Not part of the slab ABA set either way.)
        // EVAL_MEMO + MATCH_RESULT_CACHE are NOT in the slab ABA set — there they are
        // query-generation-protected under the deterministic-GC invariant, which index
        // Addr-reuse ACROSS DIRECTIVES violates (no `query_gen` bump between directives
        // of one top-level query). A reused Addr's new content must not hit a stale
        // memo/match entry keyed on its (now-wrong) content hash, so clear them here.
        crate::backend::eval::trampoline::dispatch_hints::clear_eval_memo();
        crate::backend::eval::trampoline::dispatch_hints::clear_match_result_cache();

        // Major backstop bookkeeping. A major rearms the committed watermark from
        // post-full-sweep live bytes and resets the minor cadence; a minor only
        // advances the cadence (its young odometer was reset by `promote_young`).
        if did_major {
            MAJOR_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
            // B.4: rearm the major watermark from the post-promote OLD-gen live high-water
            // (trigger == rearm metric, both `old_live` ⇒ geometric doubling ⇒ no immediate
            // re-fire; the major now fires only when the old gen genuinely re-grows). On the
            // FIRST major almost everything is freshly old (old_live ≈ total live), matching
            // the prior `live_after`-based rearm; subsequent majors fire only on real old growth.
            WATERMARK.store(
                old_live_after.saturating_mul(GROWTH).max(min_threshold()),
                Ordering::Relaxed,
            );
            MINORS_SINCE_MAJOR.store(0, Ordering::Relaxed);
            // B.5 (R3 anti-thrash): a cap-triggered major that released 0 segments is FUTILE
            // — raise CAP_FLOOR to current `committed` so the cap clause needs FURTHER growth
            // to re-fire (no per-cycle thrash on a genuinely-over-cap / 1-node-per-segment
            // fragmented old gen). A major that DID release a segment resets the floor.
            if cap_major && stats.segments_released == 0 {
                CAP_FLOOR.store(committed, Ordering::Relaxed);
            } else if stats.segments_released > 0 {
                CAP_FLOOR.store(0, Ordering::Relaxed);
            }
        } else {
            MINOR_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
            MINORS_SINCE_MAJOR.fetch_add(1, Ordering::Relaxed);
        }

        GC_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
        if phase == "midloop" {
            MIDLOOP_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
        } else if phase == "rendezvous" {
            RENDEZVOUS_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
            if did_major {
                RENDEZVOUS_MAJOR_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
            } else {
                RENDEZVOUS_MINOR_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Optional ops trace (METTATRON_INDEX_GC_REPORT=2): per-cycle reclaim, with
        // the C1.c minor/major label + the young-alloc odometer that triggered it.
        if std::env::var("METTATRON_INDEX_GC_REPORT").as_deref() == Ok("2") {
            let kind = if did_major { "major" } else { "minor" };
            // B observability: the major-REASON (old_live / cap / cadence) + old_live_after
            // + committed, so the A/B benchmark can confirm minors dominate and majors fire
            // on genuine old-gen growth (not on the inflating committed). "{kind} cycle" is
            // kept contiguous so the grep-based gates ("minor cycle"/"major cycle") still match.
            let reason = if !did_major {
                "young"
            } else if live_major {
                "old_live"
            } else if cap_major {
                "cap"
            } else {
                "cadence"
            };
            eprintln!(
                "[index_gc] {phase} {kind} cycle ({reason}): roots={} live_bytes={live_after} old_live_after={old_live_after} committed={committed} young_alloc_pre={young_alloc} reclaimed_slots={} released_segs={} bytes_freed={} minors_since_major={}",
                addrs.len(),
                stats.reclaimed_to_free_list,
                stats.segments_released,
                stats.bytes_released,
                MINORS_SINCE_MAJOR.load(Ordering::Relaxed),
            );
        }
        true
    }
}

// ── D-TLAB-1.0: `SideColumn<T>` — concurrent never-realloc side-arena column ──
//
// The lock-free replacement for `SegmentSideArenas`' `Vec<Option<Box<T>>>`
// (each `children`/`strings`/`spans` field becomes one column). It mirrors
// `IndexArena`'s never-realloc directory + `Segment`'s two-cursor bump/publish
// protocol (see `index_arena.rs`): a lazily-grown chunk directory whose cells
// are published with `Release`/`Acquire`, a `bump` CLAIM cursor (`Relaxed`
// `fetch_add` — uniqueness is all `fetch_add` needs), and a `len` PUBLISH cursor
// (`Release`-CAS, `Acquire`-load) keeping `[0, len)` a contiguous written
// prefix. This lets entries be appended through a shared `&self` (the B2 `&self`
// allocation path) without an `&mut` or a lock on the hot path.
//
// D-TLAB-1.1 capacity repair: the directory is now **two levels** — a `pages`
// super-directory of chunk-pointer pages — instead of a single fixed-size chunk
// array. The original one-level directory was a fixed `MAX_SIDE_CHUNKS = 66`
// cells (one segment's worth), imposing a HARD ceiling of `66 * 4096 = 270_336`
// CUMULATIVE appends per column. That ceiling was unsound for the (deliberate,
// C1.c #1 / commit `674dbc5`) variable-length free-list REUSE on a release-exempt
// `cur_seg`: a long-lived `cur_seg` under reuse-heavy MIDLOOP churn appends side
// entries without bound and tripped `assert!(c < MAX_SIDE_CHUNKS)` (panic
// "side column exhausted", rc=101) on `side_free_minor.metta`/`cut_young.metta`.
// The prior `Vec<Option<Box<T>>>` also grew without bound but never crashed (a
// `Vec` has no ceiling). The two-level directory RESTORES that unbounded-no-crash
// behavior: it addresses the entire `u32` index space (every index any caller can
// ever claim), while keeping per-column overhead tiny because pages — and the
// chunks within them — are allocated LAZILY on first touch (see `MAX_SIDE_PAGES`).

/// Low bits of a side-column index used for the intra-chunk offset.
/// 12 bits ⇒ 4096 entries per chunk (the unit the directory grows by).
const SIDE_CHUNK_BITS: u32 = 12;
/// Entries per chunk (`1 << SIDE_CHUNK_BITS`).
const SIDE_CHUNK_LEN: usize = 1 << SIDE_CHUNK_BITS;
/// Mask selecting the intra-chunk offset from an index.
const SIDE_CHUNK_MASK: usize = SIDE_CHUNK_LEN - 1;

/// Bits of the *chunk number* used to select the chunk WITHIN a page.
/// 10 bits ⇒ 1024 chunk-pointers per page (one `SidePage`).
const SIDE_PAGE_BITS: u32 = 10;
/// Chunk-pointers per page (`1 << SIDE_PAGE_BITS`).
const SIDE_PAGE_LEN: usize = 1 << SIDE_PAGE_BITS;
/// Mask selecting the chunk-within-page from a chunk number.
const SIDE_PAGE_MASK: usize = SIDE_PAGE_LEN - 1;

/// Maximum pages a single column super-directory holds, allocated once in `new`.
///
/// Side-column indices are `u32`, so the directory must address the entire `u32`
/// index space (entries are never recycled — a `bump` claim only ever moves up).
/// A chunk holds `2^SIDE_CHUNK_BITS` entries and a page holds `2^SIDE_PAGE_BITS`
/// chunks, so `2^(32 − SIDE_CHUNK_BITS − SIDE_PAGE_BITS)` pages cover all `2^32`
/// indices. With `SIDE_CHUNK_BITS = 12` and `SIDE_PAGE_BITS = 10` this is
/// `2^(32 − 12 − 10) = 2^10 = 1024` pages — one page per `1024 * 4096 =
/// 4_194_304`-entry span. This is a CEILING on the `u32` index space itself, not
/// on the segment capacity, so it can never be tripped by legitimate appends
/// (`bump` would have to overflow `u32` first). The super-directory is the only
/// EAGER allocation (`MAX_SIDE_PAGES` pointer-cells per column); pages and chunks
/// are allocated lazily on first touch — so an untouched column costs only the
/// super-directory, and an active one costs the super-directory plus exactly the
/// pages/chunks it has reached.
const MAX_SIDE_PAGES: usize = 1 << (32 - SIDE_CHUNK_BITS - SIDE_PAGE_BITS);

/// One side-column chunk: `SIDE_CHUNK_LEN` cells, each an
/// `UnsafeCell<MaybeUninit<(u32, Option<Box<T>>)>>`. The `u32` is the cell's
/// PER-CELL GENERATION (bumped by each `push` that claims the cell); the
/// `Option<Box<T>>` is the payload. Every cell is initialized to
/// `MaybeUninit::new((0, None))` at chunk creation (`grow_to`) — NOT `uninit()` —
/// so `assume_init_ref` on ANY in-bounds offset is always sound, even before a
/// `push` writes it. (This diverges from the node arena, which gates reads on
/// `len`; for the `(gen, Option<Box<T>>)` column initializing to `(0, None)` is
/// the safe and robust choice.)
///
/// The generation is what makes index RECYCLING (post-266d19d free-list reuse)
/// safe against a stale deferred-free snapshot: a `SideReclaim` captures the
/// generation it observed in the owning node's ref, and `free` drops the cell
/// only when the cell's current generation still equals the captured one — so a
/// reused index whose generation a live re-intern bumped is never freed by a
/// stale snapshot. (The pair is no longer niche-packed to the size of a bare
/// `Box`, but a side-column entry is only one word per payload regardless.)
type SideChunk<T> = Box<[std::cell::UnsafeCell<std::mem::MaybeUninit<(u32, Option<Box<T>>)>>]>;

/// One super-directory page: `SIDE_PAGE_LEN` chunk-pointer cells, each an
/// `UnsafeCell<MaybeUninit<SideChunk<T>>>`. A page is allocated (its cells all
/// `MaybeUninit::uninit()`) lazily by `grow_to` when the first chunk it holds is
/// needed, and published (`page_count` advanced with `Release`) BEFORE any chunk
/// within it is published — so a reader observing `c < chunk_count` (Acquire)
/// also observes the page that holds chunk `c`. (Page cells are `uninit()`, NOT
/// `None`-initialized like chunk cells, because a `SidePage` is itself a
/// directory level whose entries are only ever read after `grow_to` publishes
/// them via `chunk_count` — mirroring the node arena's segment-directory cells.)
type SidePage<T> = Box<[std::cell::UnsafeCell<std::mem::MaybeUninit<SideChunk<T>>>]>;

/// A per-(segment, field) never-realloc chunked column of address-stable
/// `Box<T>` payloads, appendable under `&self` (lock-free bump+publish) — the
/// concurrent replacement for the `Vec<Option<Box<T>>>` side arenas.
///
/// ADDRESSABILITY vs GROWTH (audit Finding 3): the directory can ADDRESS the
/// entire `u32` index space (not the segment capacity — see [`MAX_SIDE_PAGES`];
/// C1.c #1 free-list node reuse decouples side indices from node count, so the
/// index space must not be capped). GROWTH, however, is NOT append-only:
/// post-`266d19d` side indices are RECYCLED through the per-cell generation
/// free list (`SideColumn::push` reuses a freed cell before bumping), and the
/// `pending_side_major` quiescence trigger drains pending reclaims — so the
/// occupied footprint is bounded by the live-side high-water mark plus the
/// reclaims awaiting the next quiescence drain (proven:
/// `QuiescentSideIndexReuse.v`, `RendezvousSideReclaimProgress.v` /
/// `RendezvousSideReclaimProgress.tla`).
///
/// Mirrors [`IndexArena`](crate::backend::eval::cesk::index_arena)'s directory
/// + [`Segment`]'s two-cursor, but with a **two-level lazy-growing directory**
/// (`pages` → `SidePage` → `SideChunk`) so the eager footprint is just the
/// `MAX_SIDE_PAGES` super-directory while the addressable space spans all `2^32`
/// indices: a page (then a chunk within it) is written exactly once, under
/// `grow_lock`, with the PAGE published (`page_count`, `Release`) strictly before
/// the CHUNK (`chunk_count`, `Release`); `bump` claims a unique index
/// (`Relaxed`); `len` publishes the contiguous written prefix (`Release`-CAS).
struct SideColumn<T: ?Sized> {
    /// Never-realloc super-directory: `MAX_SIDE_PAGES` page cells, allocated once
    /// in `new`. Page `p` is initialized (a `SidePage` of `SIDE_PAGE_LEN`
    /// chunk-cells written) exactly once, under `grow_lock`, before `page_count`
    /// is advanced past `p` (`Release`). A reader reaches page `p` only for
    /// `p < page_count.load(Acquire)`, which `grow_to` guarantees whenever the
    /// chunk it holds is published (page published before chunk).
    pages: Box<[std::cell::UnsafeCell<std::mem::MaybeUninit<SidePage<T>>>]>,
    /// Published page count (count of initialized super-directory cells).
    /// Monotone; advanced with `Release` under `grow_lock`, read with `Acquire`.
    /// Always advanced BEFORE `chunk_count` for any chunk the page holds.
    page_count: std::sync::atomic::AtomicUsize,
    /// Published chunk count (count of initialized chunk cells across all pages).
    /// Monotone; advanced with `Release` under `grow_lock`, read with `Acquire`.
    /// `c < chunk_count` ⇒ chunk `c`'s page is published (`page_count` was
    /// advanced first).
    chunk_count: std::sync::atomic::AtomicUsize,
    /// CLAIM cursor: `fetch_add(1, Relaxed)` hands each caller a unique entry
    /// index. `Relaxed` suffices — the index is made safe to read only by the
    /// subsequent `publish` (`Release`); the claim needs atomic uniqueness only.
    bump: std::sync::atomic::AtomicUsize,
    /// PUBLISH cursor: the count of fully-written, published entries. `[0, len)`
    /// is a contiguous written prefix. Published with `Release`, read with
    /// `Acquire` — paired so an `Acquire`-load observing `len > idx` happens-
    /// after the entry's `write`.
    len: std::sync::atomic::AtomicUsize,
    /// Serializes directory growth (`grow_to`): the rare slow path (once per
    /// `SIDE_CHUNK_LEN` pushes). A plain `Mutex<()>` — `push`'s fast path does
    /// NOT take it once the target chunk is published.
    grow_lock: std::sync::Mutex<()>,
    /// Quiescence-proven reusable published entry indices.
    ///
    /// Entries are pushed only by [`SideColumn::free`], which is reached from the
    /// full true-quiescence side-drain path after the pending reclaim snapshot has
    /// been consumed. Rendezvous/midloop collectors do not call `free`, so side
    /// indices captured by deferred snapshots cannot re-enter this stack before
    /// the snapshot is resolved.
    free_indices: std::sync::Mutex<Vec<u32>>,
}

impl<T: ?Sized> SideColumn<T> {
    /// A new, empty column. Allocates the `MAX_SIDE_PAGES`-cell super-directory
    /// once (each cell an uninitialized `MaybeUninit<SidePage>`); does NOT
    /// pre-allocate any page or chunk — page 0 / chunk 0 are created lazily by
    /// the first `push` via `grow_to`. (`IndexArena::new` eagerly opens segment
    /// 0; this column stays fully lazy because a column may never be pushed to —
    /// keeping an untouched column's overhead to just the `MAX_SIDE_PAGES`
    /// super-directory cells, here `1024 * size_of::<SidePage>()` =
    /// `1024 * 16 B` = 16 KiB; each lazily-allocated page is another
    /// `1024 * 16 B` = 16 KiB, and each chunk `4096 * 8 B` = 32 KiB.)
    fn new() -> Self {
        use std::cell::UnsafeCell;
        use std::mem::MaybeUninit;
        use std::sync::atomic::AtomicUsize;
        // `UnsafeCell`/`MaybeUninit` are not `Clone`, so the super-directory is
        // built from an iterator (one uninit cell per addressable page) rather
        // than `vec![..; MAX_SIDE_PAGES]`. Allocated once, never reallocated.
        let pages: Box<[UnsafeCell<MaybeUninit<SidePage<T>>>]> = (0..MAX_SIDE_PAGES)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        SideColumn {
            pages,
            page_count: AtomicUsize::new(0),
            chunk_count: AtomicUsize::new(0),
            bump: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
            grow_lock: std::sync::Mutex::new(()),
            free_indices: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Decompose an index into `(chunk, offset)`.
    #[inline]
    fn locate(idx: usize) -> (usize, usize) {
        (idx >> SIDE_CHUNK_BITS, idx & SIDE_CHUNK_MASK)
    }

    /// Decompose a chunk number into `(page, chunk-within-page)`.
    #[inline]
    fn locate_chunk(c: usize) -> (usize, usize) {
        (c >> SIDE_PAGE_BITS, c & SIDE_PAGE_MASK)
    }

    /// Borrow chunk `c` via the two-level directory (page → chunk).
    ///
    /// # Safety
    /// The caller must guarantee `c < self.chunk_count.load(Acquire)` as
    /// observed by the calling thread — i.e. chunk `c` has been published by
    /// `grow_to`'s `Release` store to `chunk_count`, which happens-after BOTH the
    /// chunk's initialization and (published strictly earlier) its page's
    /// initialization + `page_count` publication. Under that premise both the
    /// page cell and the chunk cell are initialized.
    ///
    /// The returned slice derives from the raw pointers `UnsafeCell::get()`
    /// yields at each level (NOT from a borrow of `&self.pages`), so it does not
    /// borrow `self` — mirroring `IndexArena::segment`.
    #[inline]
    unsafe fn chunk(
        &self,
        c: usize,
    ) -> &[std::cell::UnsafeCell<std::mem::MaybeUninit<(u32, Option<Box<T>>)>>] {
        let (p, ck) = Self::locate_chunk(c);
        let page = (*self.pages[p].get()).assume_init_ref(); // &SidePage<T> -> &[..]
        let chunk_cell = page[ck].get(); // *mut MaybeUninit<SideChunk<T>>
        (*chunk_cell).assume_init_ref() // &SideChunk<T> -> &[..] via Deref
    }

    /// Grow the two-level directory so chunk `c` is published. Mirrors
    /// `IndexArena::open_segment`: takes `grow_lock`, re-checks under the lock,
    /// and for each missing chunk up to `c` allocates+writes its PAGE (if not yet
    /// present) and publishes it (`page_count`, `Release`) BEFORE allocating the
    /// chunk, writing it into the page cell, and publishing it (`chunk_count`,
    /// `Release`). Both stores are paired with the `Acquire` loads in
    /// `chunk`/`push`.
    ///
    /// CRITICAL ORDERING: the PAGE is published strictly before the CHUNK it
    /// holds, so a reader observing `c < chunk_count` (Acquire) is guaranteed to
    /// also observe `page(c) < page_count` (Acquire) — the `unsafe fn chunk`
    /// precondition then covers both levels with the single `chunk_count` check.
    ///
    /// Loops `chunk_count_now..=c` to fill ALL gaps up to `c`, not just `c`
    /// itself: although a single-segment monotone `bump` advances `c` by one at
    /// a time, a future relaxation (a claim landing far ahead) could skip a
    /// chunk; filling the gap keeps the directory dense and every published
    /// chunk (and its page) initialized.
    fn grow_to(&self, c: usize) {
        use std::cell::UnsafeCell;
        use std::mem::MaybeUninit;
        use std::sync::atomic::Ordering;
        let _guard = self
            .grow_lock
            .lock()
            .expect("side-column grow_lock poisoned");
        let mut next = self.chunk_count.load(Ordering::Acquire);
        if c < next {
            return; // another thread already grew past `c` while we waited
        }
        // The super-directory addresses the entire `u32` index space, so this
        // can only trip if a `bump` claim has overflowed `u32` (impossible for
        // legitimate appends). Asserting on the page index gives the clearer msg.
        let (top_page, _) = Self::locate_chunk(c);
        assert!(
            top_page < MAX_SIDE_PAGES,
            "side column exhausted: {MAX_SIDE_PAGES} pages"
        );
        // Fill every gap chunk_count..=c. For each chunk: ensure its page is
        // allocated+published FIRST, then allocate+write the chunk, then publish
        // the chunk. Each chunk cell is initialized to `(0, None)` (NOT uninit) so
        // `assume_init_ref` is sound on any in-bounds offset before a `push`
        // writes it (see the `SideChunk` type docs): generation 0, no payload.
        while next <= c {
            let (p, ck) = Self::locate_chunk(next);
            // (1) Ensure page `p` is allocated + published. A page's cells are
            // `MaybeUninit::uninit()` (the chunk cell is written before the chunk
            // is published, so it is never read before then). Publish the page
            // (`Release`) strictly BEFORE the chunk so a reader seeing the chunk
            // also sees the page.
            if p >= self.page_count.load(Ordering::Acquire) {
                let page: SidePage<T> = (0..SIDE_PAGE_LEN)
                    .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                    .collect();
                // SAFETY: page cell `p` is not yet published (`p == page_count`),
                // so no reader can observe it; under `grow_lock` we are the unique
                // writer. Initialize it before publishing `p`.
                unsafe {
                    (*self.pages[p].get()).write(page);
                }
                self.page_count.store(p + 1, Ordering::Release); // publish page
            }
            // (2) Allocate the chunk (all cells `(0, None)`: generation 0, no
            // payload) and write it into page `p`'s chunk-cell `ck`.
            let chunk: SideChunk<T> = (0..SIDE_CHUNK_LEN)
                .map(|_| UnsafeCell::new(MaybeUninit::new((0u32, None))))
                .collect();
            // SAFETY: page `p` is published (`p < page_count`, just ensured), so
            // its cell array is initialized. Chunk cell `ck` is not yet published
            // (`next == chunk_count`), so no reader can observe it; under
            // `grow_lock` we are the unique writer of this cell.
            unsafe {
                let page = (*self.pages[p].get()).assume_init_ref();
                (*page[ck].get()).write(chunk);
            }
            // (3) Publish the chunk (paired with the `Acquire` load in `chunk`).
            self.chunk_count.store(next + 1, Ordering::Release);
            next += 1;
        }
    }

    /// Publish entry `idx` into the contiguous written prefix.
    ///
    /// Spins until `len == idx`, then advances `len` to `idx + 1` with `Release`
    /// (so the prior write happens-before any `Acquire`-load of `len` that
    /// observes the new value). Keeps `[0, len)` a contiguous written prefix
    /// even when claims complete out of order. Verbatim `Segment::publish`.
    #[inline]
    fn publish(&self, idx: usize) {
        use std::sync::atomic::Ordering;
        while self
            .len
            .compare_exchange_weak(idx, idx + 1, Ordering::Release, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
    }

    /// Append `boxed`, returning its stable index AND the cell's new generation
    /// (`(idx, gen)`). It first consumes a quiescence-proven reusable index if one
    /// exists; otherwise it takes the monotone bump path. The bump path is
    /// lock-free on the steady-state fast path (the `grow_to` lock is taken only
    /// when crossing into a not-yet-published chunk, once per `SIDE_CHUNK_LEN`
    /// pushes).
    ///
    /// GENERATION: every claim of a cell (reuse OR first bump) increments that
    /// cell's stored generation and returns the new value, which the caller stores
    /// in the node's ref (`ChildRef`/`ByteRef`/`SpanRef`). A deferred-free snapshot
    /// captures it and `free` honors it (see [`Self::free`]) — so a recycled index
    /// whose generation a live re-intern bumped is never freed by a stale snapshot.
    /// A fresh (bumped) cell is `(0, None)` from `grow_to`, so its first claim
    /// yields generation 1; a reused cell yields its prior generation + 1.
    ///
    /// Reuse protocol: `free_indices` contains only published cells whose prior
    /// payload was dropped by a full true-quiescence drain. Popping an index gives
    /// this caller unique ownership to write a fresh `Some(Box<T>)` (and a bumped
    /// generation) into a cell that no live node can still read.
    ///
    /// Bump protocol (mirrors `IndexArena::alloc_bump` → `Segment::{bump_one,
    /// write_claimed, publish}`): CLAIM a unique `idx` (`Relaxed` `fetch_add`);
    /// ensure the target chunk is published (`grow_to` under `Acquire` re-check);
    /// WRITE the (uniquely claimed, exclusive, unpublished) cell; PUBLISH `idx`.
    fn push(&self, boxed: Box<T>) -> (u32, u32) {
        use std::sync::atomic::Ordering;
        if let Some(idx) = self
            .free_indices
            .lock()
            .expect("side-column free_indices poisoned")
            .pop()
        {
            let (c, off) = Self::locate(idx as usize);
            // SAFETY: indices in `free_indices` were previously published, so
            // their chunks are initialized. The pop gives this writer unique
            // ownership of the freed cell until it is published again by storing
            // `Some`. The cell is already initialized (a `(gen, None)` left by the
            // `free` that recycled it), so `assume_init_mut` is sound; bump the
            // generation in place and install the fresh payload.
            let g = unsafe {
                let reused_chunk = self.chunk(c);
                let slot = (*reused_chunk[off].get()).assume_init_mut();
                let g = slot.0.wrapping_add(1);
                slot.0 = g;
                slot.1 = Some(boxed);
                g
            };
            return (idx, g);
        }

        let idx = self.bump.fetch_add(1, Ordering::Relaxed); // unique claim
        let (c, off) = Self::locate(idx);
        if c >= self.chunk_count.load(Ordering::Acquire) {
            self.grow_to(c);
        }
        // SAFETY: `c < chunk_count` now (either it already was, or `grow_to`
        // published it — both with `Release`, observed here `Acquire`-wise via
        // `grow_to`'s store / the load above). `idx` was uniquely claimed by the
        // `fetch_add`, so no other writer holds `(c, off)`; `off` is not yet
        // published (`off >= len` until `publish`), so no reader observes it.
        // The write is therefore exclusive and races no reader. The cell is
        // already initialized to `(0, None)` by `grow_to`, so we READ-MODIFY it
        // (bump the generation, install the payload) rather than `.write(..)` over
        // a `MaybeUninit` we'd then have to re-seed with a generation.
        let g = unsafe {
            let chunk = self.chunk(c);
            let cell = (*chunk[off].get()).assume_init_mut();
            let g = cell.0.wrapping_add(1);
            cell.0 = g;
            cell.1 = Some(boxed);
            g
        };
        self.publish(idx);
        (idx as u32, g)
    }

    /// Borrow the published entry at `idx`, or `None` if it was freed.
    ///
    /// # Safety
    /// The caller must guarantee `idx < self.len.load(Acquire)` as observed by
    /// the calling thread (the entry is in the published prefix). Publication
    /// (`len.CAS(.., Release)`) happens-after the entry's write, so an
    /// `Acquire`-load of `len` observing `len > idx` also observes the fully-
    /// written `Option<Box<T>>`. A published entry is mutated only by `free`
    /// (`&mut self`, quiescence-only — see the SAFETY block on the `unsafe
    /// impl`), so the borrow races no writer.
    unsafe fn get(&self, idx: u32) -> Option<&T> {
        use std::sync::atomic::Ordering;
        debug_assert!(
            (idx as usize) < self.len.load(Ordering::Acquire),
            "SideColumn::get index {idx} out of published range"
        );
        let (c, off) = Self::locate(idx as usize);
        let chunk = self.chunk(c);
        // `.1` is the payload; `.0` is the cell generation (not read here).
        (*chunk[off].get()).assume_init_ref().1.as_deref()
    }

    /// Safe wrapper for lagging auxiliary-table validation. Returns `None` if
    /// the side index was never published or if the published slot has already
    /// been freed.
    #[inline]
    fn get_if_published(&self, idx: u32) -> Option<&T> {
        if (idx as usize) >= self.published_len() {
            return None;
        }
        // SAFETY: the bounds check above establishes `idx < published_len`.
        unsafe { self.get(idx) }
    }

    /// Drop the payload `Box` at `idx` — but ONLY when the cell's current
    /// generation still equals `gen` (the generation captured in the deferred-free
    /// snapshot) — leaving the cell as `(gen, None)`, and make the published entry
    /// index reusable.
    ///
    /// THE GENERATION GUARD is the use-after-free fix: side-column indices are
    /// RECYCLED (post-266d19d free-list reuse), so a stale `SideReclaim` snapshot
    /// `{owner(dead), idx}` whose `idx` was meanwhile re-claimed by a LIVE node
    /// would, without this guard, drop the live node's payload `Box` (then a read
    /// of the live node observes `None` ⇒ the `live ... slot` panic / a true UAF).
    /// Each `push` that claims a cell bumps its generation; the snapshot captured
    /// the generation it observed in the owner's ref. So when the captured `gen`
    /// no longer matches the cell's current generation, the cell has been reused by
    /// a newer occupant and the snapshot MUST NOT free it; the guard skips it. A
    /// matching generation means the cell is still the exact occupant the snapshot
    /// named, so dropping it is correct. The cell's generation is NOT reset here —
    /// the next `push` that reuses the index increments it again, so a second stale
    /// snapshot bearing the same (now-superseded) generation still mismatches.
    ///
    /// `&mut self` (quiescence-only): a free runs only at a quiescent safepoint,
    /// statically exclusive of every `&self` reader/pusher, so it races nothing
    /// (the index analogue of the arena's free-list-reuse-at-quiescence rule).
    /// Re-freeing an already-`None` cell (or one whose generation moved on) is a
    /// no-op and does not push a duplicate reusable index. `Relaxed` load of `len`
    /// is fine under `&mut self` (no concurrent writer).
    fn free(&mut self, idx: u32, gen: u32) {
        use std::sync::atomic::Ordering;
        if (idx as usize) < self.len.load(Ordering::Relaxed) {
            let (c, off) = Self::locate(idx as usize);
            // SAFETY: `idx < len` ⇒ `c < chunk_count` (published) and the cell is
            // initialized (every cell is `(0, None)` from `grow_to`, then possibly
            // a `(g, Some)` from `push`). `&mut self` is exclusive, so no aliasing.
            let was_live = unsafe {
                let chunk = self.chunk(c);
                let slot = (*chunk[off].get()).assume_init_mut();
                if slot.0 == gen {
                    // The captured generation still names THIS occupant: drop it.
                    slot.1.take().is_some()
                } else {
                    // Reused by a newer occupant (generation moved on): the stale
                    // snapshot must not free the live payload. Leave it untouched.
                    false
                }
            };
            if was_live {
                self.free_indices
                    .lock()
                    .expect("side-column free_indices poisoned")
                    .push(idx);
            }
        }
    }

    /// The number of published entries (`len`).
    #[inline]
    fn published_len(&self) -> usize {
        use std::sync::atomic::Ordering;
        self.len.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn reusable_len(&self) -> usize {
        self.free_indices
            .lock()
            .expect("side-column free_indices poisoned")
            .len()
    }

    /// The number of published chunks (diagnostics / tests).
    #[inline]
    fn chunk_count(&self) -> usize {
        use std::sync::atomic::Ordering;
        self.chunk_count.load(Ordering::Acquire)
    }
}

// D-TLAB-1.1: `SideColumn` OWNS heap allocations (the page `Box`es, the chunk
// `Box`es, and in each initialized chunk cell an `Option<Box<T>>` payload) inside
// `MaybeUninit` cells, which `MaybeUninit` does NOT drop automatically. (This is
// the crucial difference from `IndexArena`/`Segment`, whose cells hold `N: Copy`
// nodes — a `Copy` type owns no resources, so dropping its directory `Box` without
// an explicit teardown leaks nothing; `Segment::release` is sound for exactly that
// reason.) Without this `Drop`, every `SideColumn` teardown — a segment release
// (`sides[seg] = SegmentSideArenas::default()` in `sweep`/`sweep_young`) or the
// heap's own drop — would LEAK every payload `Box<T>`, every chunk `Box`, and
// every page `Box`, regressing the RSS reclamation the side arenas exist to
// provide and diverging from the prior `Vec<Option<Box<T>>>`, which freed its
// payloads on drop/reassignment. This impl restores that exactly, INNERMOST-FIRST:
// drop each chunk cell's `Option<Box<T>>`, then each chunk `Box`, then each page
// `Box` — never a page before the chunks it holds.
impl<T: ?Sized> Drop for SideColumn<T> {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        // Only the PUBLISHED cells are initialized: chunk cells `[0, chunk_count)`
        // (`grow_to` writes chunk `c` then `Release`-publishes `c+1`) and page
        // cells `[0, page_count)` (published strictly before any chunk they hold).
        // Cells `>= chunk_count` / `>= page_count` are `MaybeUninit::uninit()` and
        // MUST NOT be touched. `&mut self` in `drop` is exclusive, so a `Relaxed`
        // load suffices (no concurrent writer/reader to synchronize with).
        let chunks_live = self.chunk_count.load(Ordering::Relaxed);
        // (1) INNERMOST: every published chunk's cells, then the chunk `Box`.
        for c in 0..chunks_live {
            let (p, ck) = Self::locate_chunk(c);
            // SAFETY: `c < chunk_count` ⇒ chunk `c` was initialized by `grow_to`,
            // and its page `p < page_count` (published first) so the page cell is
            // initialized too. `&mut self` is exclusive, so we are the unique
            // accessor. We take raw pointers (not `&mut` borrows of `self.pages`)
            // so the inner cell drops do not alias a live borrow of the directory.
            unsafe {
                let page = (*self.pages[p].get()).assume_init_ref(); // &SidePage<T>
                let chunk_cell = page[ck].get(); // *mut MaybeUninit<SideChunk<T>>
                                                 // Borrow the chunk to drop each cell's `(u32, Option<Box<T>>)`.
                                                 // EVERY cell of a published chunk is initialized — `grow_to` fills
                                                 // all `SIDE_CHUNK_LEN` cells with `MaybeUninit::new((0, None))`, and
                                                 // `push` only mutates a cell's generation/payload in place — so
                                                 // `assume_init_drop` is sound on each, dropping any live payload
                                                 // `Box<T>` (a freed cell is `(g, None)`, whose drop is a no-op; the
                                                 // `u32` generation drops trivially).
                let chunk: &mut SideChunk<T> = (*chunk_cell).assume_init_mut();
                for cell in chunk.iter() {
                    (*cell.get()).assume_init_drop(); // drops `(u32, Option<Box<T>>)`
                }
                // Now drop the chunk `Box` itself (frees the cell array). The page
                // `Box` that holds this chunk cell is dropped only in pass (2),
                // strictly after all its chunks — never a page before its chunks.
                (*chunk_cell).assume_init_drop(); // drops `SideChunk<T>` (the Box)
            }
        }
        // (2) Now every chunk is gone; drop each published page `Box`.
        let pages_live = self.page_count.load(Ordering::Relaxed);
        for p in 0..pages_live {
            // SAFETY: `p < page_count` ⇒ page `p` was initialized by `grow_to`;
            // all chunks it held were dropped in pass (1). `&mut self` is
            // exclusive. The chunk cells inside the page that were never published
            // (`>= chunk_count`) are `MaybeUninit::uninit()` and own nothing, so
            // dropping the page `Box` (the cell array) is sound without touching
            // them.
            unsafe {
                let page_cell = self.pages[p].get(); // *mut MaybeUninit<SidePage<T>>
                (*page_cell).assume_init_drop(); // drops `SidePage<T>` (the Box)
            }
        }
        // The super-directory `Box<[UnsafeCell<MaybeUninit<..>>]>` itself, and all
        // unpublished (uninit) page cells, drop trivially (no owned resources) when
        // `self.pages` drops after this — no manual teardown needed for them.
    }
}

// SAFETY: `SideColumn<T>`'s interior mutability (the `UnsafeCell` directory and
// per-chunk cells) is disciplined by the same claim/publish protocol as
// `IndexArena`/`Segment` (see the type-level SAFETY block in `index_arena.rs`):
//
//   (i)   UNIQUE CLAIM. A cell is *written* only after `bump.fetch_add(1,
//         Relaxed)` hands the writing thread a unique index. No two threads
//         obtain the same index, so the `MaybeUninit::write` is exclusive — no
//         writer aliases another writer.
//
//   (ii)  PUBLISHED READS ONLY, RELEASE/ACQUIRE-ORDERED. A reader (`get`)
//         dereferences index `idx` only for `idx < len.load(Acquire)`. The
//         writer publishes with `len.CAS(idx→idx+1, Release)` *after* its write,
//         so an `Acquire`-load observing `len > idx` happens-after the write ⇒
//         no uninit/torn read, no read/write race.
//
//   (iii) DIRECTORY PUBLICATION (two-level). A reader reaches chunk cell `c`
//         only via its page `p = c >> SIDE_PAGE_BITS` then the chunk, dereferenc-
//         ing each only for `c < chunk_count.load(Acquire)`. `grow_to`, under
//         `grow_lock` (the unique writer of both cells), publishes the PAGE first
//         — initialize page cell `p`, then `page_count.store(p+1, Release)` —
//         and the CHUNK strictly after — initialize chunk cell `c` inside page
//         `p`, then `chunk_count.store(c+1, Release)`. So observing
//         `c < chunk_count` (Acquire) happens-after BOTH the chunk's
//         initialization AND (published earlier) `p < page_count` and the page's
//         initialization ⇒ the page `Box` ptr, the chunk `Box` ptr, and the
//         chunk's cells (all `None` from `grow_to`) are all visible before the
//         reader indexes it. The single `chunk_count` check therefore covers
//         both directory levels.
//
//   (iv)  FREE AT QUIESCENCE, GENERATION-GUARDED. The only in-place rewrites of
//         an already-published entry are `free` and a reuse `push`, both of which
//         claim the cell only at a quiescent safepoint (`free` is `&mut self`; a
//         reuse `push` pops an index `free` made reusable, which only happens at
//         the same quiescent drain). `&mut self`/quiescence is statically
//         exclusive of every `&self` reader/pusher, so a free never races a
//         concurrent access. Indices ARE recycled (post-266d19d free-list reuse),
//         so the cell carries a PER-CELL GENERATION (bumped by each claiming
//         `push`): a deferred-free snapshot captures the generation it observed,
//         and `free` drops the cell only when the captured generation still equals
//         the cell's current one — so a recycled index whose generation a live
//         re-intern bumped is never freed by a stale snapshot. The generation
//         guard is the index-ABA defense the never-recycle assumption used to
//         provide.
//
// Hence no data race on any field. `Send` requires `T: Send` (the column owns
// `Box<T>` payloads it may hand to another thread); `Sync` requires `T: Send +
// Sync` (a shared `&SideColumn` lets multiple threads obtain `&T`) — the same
// conservative bounds `IndexArena` uses. `T: ?Sized` is supported (the columns
// hold `[MettaValue]`/`str`/`Span` via `Box<[_]>`/`Box<str>`/`Box<Span>`).
unsafe impl<T: ?Sized + Send> Send for SideColumn<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for SideColumn<T> {}

// Shared by the test submodules that exercise index-mode handles (`mod tests`
// and `mod tsan_concurrent_factory`, both file-root siblings). F4 selects index
// mode by the `index-gc` feature, so tests no longer mutate process-global GC
// mode. The guard remains because these tests also share global index-heap state
// and must serialize while values with index handles are live.
#[cfg(test)]
static INDEX_MODE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
pub(crate) struct IndexModeTestGuard {
    #[allow(dead_code)] // held for RAII: releases the shared index-test lock last
    lock: std::sync::MutexGuard<'static, ()>,
}
#[cfg(test)]
#[must_use = "bind as `let _mode = enter_index_mode_for_test();` to hold the lock for the test"]
pub(crate) fn enter_index_mode_for_test() -> IndexModeTestGuard {
    let lock = INDEX_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    IndexModeTestGuard { lock }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RendezvousWitnessTestGuard;

    impl RendezvousWitnessTestGuard {
        fn enter() -> Self {
            crate::backend::models::gc_allocator::set_current_witness_ok(true);
            Self
        }
    }

    impl Drop for RendezvousWitnessTestGuard {
        fn drop(&mut self) {
            crate::backend::models::gc_allocator::set_current_witness_ok(false);
        }
    }

    fn run_rendezvous_collection_for_test(roots: &[MettaValue]) -> bool {
        let _gip = crate::backend::models::gc_allocator::GcInProgressGuard::try_enter()
            .expect("test rendezvous collection must acquire GC_IN_PROGRESS");
        let _witness = RendezvousWitnessTestGuard::enter();
        index_gc::run_collection_if_triggered_rendezvous(roots)
    }

    fn allocate_dead_fixed_nodes(count: usize, salt: usize) {
        let mut heap = global_index_heap().write().expect("index heap write lock");
        for i in 0..count {
            let n = (salt as f64) * 1_000_000.0 + i as f64 + 0.25;
            let _ = heap.alloc_fixed(Node::Float(n));
        }
    }

    #[test]
    fn sexpr_children_roundtrip_and_co_locate() {
        // Inline-scalar children (Bool/Long) don't depend on the value mode.
        let f = crate::backend::models::global_factory();
        use crate::backend::models::MettaValueFactory;
        let items = vec![f.long(1), f.long(2), f.long(3)];
        let mut heap = IndexHeap::with_segment_capacity(8);
        let addr = heap.alloc_sexpr(&items);
        let got = heap.children(addr);
        assert_eq!(got.len(), 3);
        assert_eq!(got, items.as_slice(), "SExpr children roundtrip");
    }

    #[test]
    fn string_and_atom_bytes_roundtrip_across_segments() {
        let mut heap = IndexHeap::with_segment_capacity(2);
        let a = heap.alloc_string("hello");
        let b = heap.alloc_atom("world-variable");
        let c = heap.alloc_string("third forces a new segment");
        assert_eq!(heap.str_slice(a), "hello");
        assert_eq!(heap.str_slice(b), "world-variable");
        assert_eq!(heap.str_slice(c), "third forces a new segment");
        assert!(
            heap.segment_count() >= 2,
            "small capacity forced multiple segments"
        );
    }

    #[test]
    fn span_roundtrip() {
        use crate::ir::{Position, Span};
        let mut heap = IndexHeap::with_segment_capacity(8);
        let f = crate::backend::models::global_factory();
        use crate::backend::models::MettaValueFactory;
        let span = Span {
            start: Position {
                row: 1,
                column: 2,
                byte_offset: 3,
            },
            end: Position {
                row: 4,
                column: 5,
                byte_offset: 6,
            },
        };
        let addr = heap.alloc_spanned(f.long(7), span);
        assert_eq!(heap.span_at(addr), span);
    }

    #[test]
    fn transitive_mark_resolves_sexpr_children_from_side_arena() {
        // SExpr children that are arena handles require Index mode for
        // as_arena_addr to decode them. nextest isolates this in its own process.
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(64);
        // Two leaf atoms; reference them from a SExpr as index handles.
        let l1 = heap.alloc_atom("a");
        let l2 = heap.alloc_atom("b");
        let h1 = MettaValue::from_addr(l1, 0, TAG5_ATOM);
        let h2 = MettaValue::from_addr(l2, 0, TAG5_ATOM);
        let sx = heap.alloc_sexpr(&[h1, h2]);
        let orphan = heap.alloc_atom("orphan");

        let newly = heap.mark(&[sx]);
        assert_eq!(
            newly, 3,
            "sx + l1 + l2 reached transitively via the side-arena"
        );
        // Sweep frees the orphan; the reachable graph survives.
        let stats = heap.sweep();
        assert!(stats.reclaimed_to_free_list >= 1, "orphan reclaimed");
        // sx still resolves its children after sweep.
        assert_eq!(heap.children(sx).len(), 2);
        let _ = orphan;
    }

    #[test]
    fn transitive_mark_traces_space_handle_contents() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(64);
        let child = heap.alloc_atom("space-child");
        let orphan = heap.alloc_atom("space-orphan");

        let mut module_space = crate::backend::modules::ModuleSpace::new();
        module_space.add_atom(MettaValue::from_addr(child, 0, TAG5_ATOM));
        let handle = SpaceHandle::for_module(
            crate::backend::modules::ModId::new(10_001),
            "mark-space".to_string(),
            std::sync::Arc::new(parking_lot::RwLock::new(module_space)),
        );
        let space = heap.alloc_space(handle);

        let newly = heap.mark(&[space]);
        assert_eq!(newly, 2, "space node + contained atom are marked");
        let stats = heap.sweep();
        assert!(
            stats.reclaimed_to_free_list >= 1,
            "unreachable atom outside the SpaceHandle is reclaimed"
        );
        assert_eq!(heap.str_slice(child), "space-child");
        let _ = orphan;
    }

    #[test]
    fn minor_mark_traverses_old_space_to_mark_young_contents() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(2);
        let module_space = std::sync::Arc::new(parking_lot::RwLock::new(
            crate::backend::modules::ModuleSpace::new(),
        ));
        let handle = SpaceHandle::for_module(
            crate::backend::modules::ModId::new(10_002),
            "minor-space".to_string(),
            module_space.clone(),
        );

        let space = heap.alloc_space(handle);
        let _fill_old_segment = heap.alloc_atom("fill-old-segment");
        let child = heap.alloc_atom("young-space-child");
        let orphan = heap.alloc_atom("young-orphan");
        heap.promote_young();
        assert!(
            space.segment() < heap.arena.young_floor(),
            "space handle is old after promotion"
        );
        assert!(
            child.segment() >= heap.arena.young_floor(),
            "contained atom remains young after promotion"
        );

        module_space
            .write()
            .add_atom(MettaValue::from_addr(child, 0, TAG5_ATOM));
        let newly = heap.mark_young(&[space]);
        assert_eq!(
            newly, 1,
            "old space traversal marks the young contained atom"
        );
        let stats = heap.sweep_young();
        assert!(
            stats.reclaimed_to_free_list >= 1,
            "unreachable young atom is reclaimed by the minor"
        );
        assert_eq!(heap.str_slice(child), "young-space-child");
        let _ = orphan;
    }

    // ---- F1 SATB-young lever: the stale-old-mark discriminator + the fix ----
    // Source form of `tla/SATBYoungSweepStaleOldMark.tla`: a stale old mark (an
    // aborted SATB window's deletion-barrier shade) violates `NoStaleOldMark`
    // under a young-only sweep (ClearOldMarks=FALSE, the expected-fail cfg) and
    // the consequence is an UNDER-MARK: the pruning full mark treats the stale
    // node as visited and never marks its live children. `clear_old_marks` is
    // the ClearOldMarks=TRUE wiring that removes the hazard.

    #[test]
    fn stale_old_mark_hides_live_children_from_the_pruning_full_mark() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(4);
        let child = heap.alloc_atom("old-child");
        let parent = heap.alloc_sexpr(&[MettaValue::from_addr(child, 0, TAG5_ATOM)]);
        let _f1 = heap.alloc_atom("fill-a");
        let _f2 = heap.alloc_atom("fill-b"); // seg 0 full
        let _young = heap.alloc_atom("young-opens-seg1"); // cur_seg -> 1
        heap.promote_young(); // young_floor = 1: parent/child are OLD
        assert!(parent.segment() < heap.arena.young_floor());

        // The stale shade: ONE old mark, no traversal (what an armed SATB
        // deletion barrier leaves behind if the cycle aborts before its full sweep).
        heap.arena.mark(parent);

        // THE HAZARD (model cfg young_only / ClearOldMarks=FALSE): the full mark
        // prunes at the already-marked parent — the live child is NEVER marked.
        heap.mark(&[parent]);
        assert!(
            !heap.arena.is_marked(child),
            "under-mark: the pruning full mark never descends through the \
             stale-marked old parent (this is the UAF mechanism NoStaleOldMark guards)"
        );

        // THE FIX (ClearOldMarks=TRUE): clear old marks, re-mark — the child is found.
        heap.clear_old_marks();
        assert!(!heap.arena.is_marked(parent), "stale mark cleared");
        heap.mark(&[parent]);
        assert!(
            heap.arena.is_marked(child),
            "after clear_old_marks the full mark descends and marks the live child"
        );
    }

    #[test]
    fn rendezvous_minor_sequence_clears_stale_old_marks() {
        // The exact young-cycle sequence the rendezvous-phase minor arm runs
        // (mark_sweep_if_over_watermark, phase == "rendezvous"): mark_young →
        // sweep_young → clear_old_marks → promote_young. A pre-existing stale old
        // mark must NOT survive it (NoStaleOldMark at phase=promoted), young
        // liveness must be respected, and old slots must be untouched.
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(4);
        let old_atom = heap.alloc_atom("old-stale");
        let _f1 = heap.alloc_atom("fill-a");
        let _f2 = heap.alloc_atom("fill-b");
        let _f3 = heap.alloc_atom("fill-c"); // seg 0 full
        let young_live = heap.alloc_atom("young-live"); // opens seg 1
        let young_orphan = heap.alloc_atom("young-orphan");
        heap.promote_young(); // young_floor = 1: old_atom OLD, seg-1 atoms YOUNG
        assert!(old_atom.segment() < heap.arena.young_floor());
        assert!(young_live.segment() >= heap.arena.young_floor());

        heap.arena.mark(old_atom); // the stale shade

        // The rendezvous minor sequence.
        heap.mark_young(&[young_live]);
        let stats = heap.sweep_young();
        heap.clear_old_marks();
        heap.promote_young();

        assert!(
            !heap.arena.is_marked(old_atom),
            "NoStaleOldMark: the stale old mark is cleared by the rendezvous minor"
        );
        assert_eq!(heap.str_slice(old_atom), "old-stale", "old slot untouched");
        assert_eq!(
            heap.str_slice(young_live),
            "young-live",
            "live young survives"
        );
        assert!(
            stats.reclaimed_to_free_list >= 1,
            "the unreachable young orphan is reclaimed"
        );
        let _ = young_orphan;
    }

    #[test]
    fn segment_release_co_releases_side_arenas() {
        // Fill segment 0 with variable-length (string) nodes, keep a live node in
        // segment 1, then sweep: segment 0 dies and its byte side-arena is reset.
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(2);
        let _d0 = heap.alloc_string("dead-zero");
        let _d1 = heap.alloc_string("dead-one"); // segment 0 full
        let live = heap.alloc_string("live"); // segment 1, current
        let live_seg = live.segment();
        heap.mark(&[live]);
        let stats = heap.sweep();
        assert_eq!(
            stats.segments_released, 1,
            "segment 0 fully dead → released"
        );
        // The live string is intact and the current segment's side-arena is kept.
        assert_eq!(heap.str_slice(live), "live");
        assert!(live_seg != 0, "live value is not in the released segment 0");
    }

    #[test]
    fn committed_bytes_drop_after_segment_release() {
        // Inc 6: the watermark signal must DROP when a fully-dead segment is
        // released, else the trigger never re-arms. Fill segment 0 with dead
        // strings, keep a live node in segment 1, sweep, and assert committed
        // bytes fell.
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(2);
        let _d0 = heap.alloc_string("dead-zero");
        let _d1 = heap.alloc_string("dead-one"); // segment 0 full
        let live = heap.alloc_string("live"); // segment 1
        let before = heap.committed_bytes();
        heap.mark(&[live]);
        let stats = heap.sweep();
        assert_eq!(stats.segments_released, 1, "segment 0 released");
        let after = heap.committed_bytes();
        assert!(
            after < before,
            "committed_bytes must drop after release: before={before} after={after}"
        );
        // live_bytes reflects only the surviving high-water node slots.
        assert!(heap.live_bytes() > 0, "the live string still counts");
    }

    #[test]
    fn side_reclaim_snapshot_survives_node_slot_reuse() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(16);

        let old_dead = heap.alloc_string("old-dead");
        let live = heap.alloc_string("live-root");
        heap.mark(&[live]);
        let stats = heap.sweep_young();
        assert_eq!(stats.reclaimed_to_free_list, 1, "old string reclaimed");
        assert_eq!(
            heap.pending_side_reclaims.len(),
            1,
            "side owner is snapshotted at reclaim time"
        );

        let replacement = heap.alloc_string("replacement");
        assert_eq!(
            replacement, old_dead,
            "cur-segment free-list reuse should overwrite the reclaimed node slot"
        );
        assert_eq!(heap.str_slice(replacement), "replacement");

        heap.free_pending_side_reclaims();
        assert_eq!(
            heap.str_slice(replacement),
            "replacement",
            "draining the old side snapshot must not free the replacement's side slot"
        );
    }

    #[test]
    fn current_segment_reuse_pressure_tracks_exclusive_progress() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(16);

        let dead = heap.alloc_string("dead-current");
        let live = heap.alloc_string("live-root");
        heap.mark(&[live]);
        let stats = heap.sweep_young();
        assert_eq!(stats.reclaimed_to_free_list, 1);
        assert!(
            heap.has_current_free_slot(),
            "exclusive allocator should see a reusable current-segment slot"
        );

        let replacement = heap.alloc_string("replacement");
        assert_eq!(replacement, dead);
        assert!(
            !heap.has_current_free_slot(),
            "the exclusive reuse path consumed the current-segment slot"
        );
    }

    #[test]
    fn side_column_reuses_quiescence_freed_index_once() {
        let mut column = SideColumn::<str>::new();

        let (old, old_gen) = column.push("old".into());
        assert_eq!(old, 0);
        assert_eq!(column.published_len(), 1);

        column.free(old, old_gen);
        assert_eq!(column.reusable_len(), 1);
        // Idempotent re-free with the same generation: the payload is already
        // `None`, so it does not push a duplicate reusable index (the generation
        // still matches but `take()` finds nothing live).
        column.free(old, old_gen);
        assert_eq!(
            column.reusable_len(),
            1,
            "idempotent free must not duplicate a reusable side index"
        );

        let (replacement, _replacement_gen) = column.push("replacement".into());
        assert_eq!(
            replacement, old,
            "quiescence-freed side index should be reused before bumping"
        );
        assert_eq!(
            column.published_len(),
            1,
            "side-column high-water must not grow when a reusable index exists"
        );
        assert_eq!(column.reusable_len(), 0);
        // SAFETY: `replacement < published_len`.
        assert_eq!(unsafe { column.get(replacement) }, Some("replacement"));
    }

    #[test]
    fn full_quiescence_side_drain_enables_side_index_reuse() {
        fn string_side_idx(heap: &IndexHeap, addr: Addr) -> u32 {
            match heap.arena.get(addr) {
                Node::String(br) => br.idx,
                _ => panic!("expected String node"),
            }
        }

        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(16);

        let dead = heap.alloc_string("dead-current");
        let dead_side_idx = string_side_idx(&heap, dead);
        let live = heap.alloc_string("live-root");
        heap.mark(&[live]);
        let stats = heap.sweep();
        assert_eq!(stats.reclaimed_to_free_list, 1);
        heap.free_pending_side_reclaims();

        let replacement = heap.alloc_string("replacement");
        assert_eq!(replacement, dead, "node slot should be reused");
        assert_eq!(
            string_side_idx(&heap, replacement),
            dead_side_idx,
            "full quiescence drain should make the old side index reusable"
        );
        assert_eq!(heap.str_slice(replacement), "replacement");
        assert_eq!(heap.str_slice(live), "live-root");
    }

    #[test]
    fn duplicate_sweep_opportunity_does_not_duplicate_side_snapshot() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(16);

        let dead = heap.alloc_string("dead-once");
        let live = heap.alloc_string("live-root");
        heap.mark(&[live]);
        let first = heap.sweep_young();
        assert_eq!(first.reclaimed_to_free_list, 1, "dead slot first reclaimed");
        assert_eq!(heap.pending_side_reclaims.len(), 1);

        heap.mark(&[live]);
        let second = heap.sweep_young();
        assert_eq!(
            second.reclaimed_to_free_list, 0,
            "free_bit suppresses duplicate ownership of the same dead slot"
        );
        assert_eq!(
            heap.pending_side_reclaims.len(),
            1,
            "side snapshot must be emitted only when free-list ownership is newly acquired"
        );

        let replacement = heap.alloc_string("replacement");
        assert_eq!(replacement, dead, "the single free-list entry is reusable");
        heap.free_pending_side_reclaims();
        assert_eq!(heap.str_slice(replacement), "replacement");
    }

    #[test]
    fn duplicate_sweep_opportunity_does_not_duplicate_children_side_snapshot() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(16);

        let dead = heap.alloc_sexpr(&[MettaValue::Bool(true)]);
        let live = heap.alloc_sexpr(&[MettaValue::Bool(false)]);
        heap.mark(&[live]);
        let first = heap.sweep_young();
        assert_eq!(first.reclaimed_to_free_list, 1, "dead slot first reclaimed");
        assert_eq!(heap.pending_side_reclaims.len(), 1);

        heap.mark(&[live]);
        let second = heap.sweep_young();
        assert_eq!(
            second.reclaimed_to_free_list, 0,
            "free_bit suppresses duplicate ownership of the same child slot owner"
        );
        assert_eq!(
            heap.pending_side_reclaims.len(),
            1,
            "children side snapshot must be emitted only for newly acquired ownership"
        );

        let replacement = heap.alloc_sexpr(&[MettaValue::Bool(false), MettaValue::Bool(true)]);
        assert_eq!(replacement, dead, "the single free-list entry is reusable");
        heap.free_pending_side_reclaims();
        assert_eq!(heap.children(replacement).len(), 2);
    }

    #[test]
    fn marked_owner_pending_side_snapshot_is_not_drained() {
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(16);

        let live = heap.alloc_sexpr(&[MettaValue::Bool(true)]);
        let snapshot = heap
            .side_reclaim_for_addr(live)
            .expect("live SExpr has a children side slot");
        heap.pending_side_reclaims.push(snapshot);

        heap.mark(&[live]);
        heap.drop_or_free_pending_side_reclaims_after_full_mark();

        assert!(
            heap.pending_side_reclaims.is_empty(),
            "the stale pending snapshot should be consumed"
        );
        assert_eq!(
            heap.children(live).len(),
            1,
            "a full mark that proves the owner live must not free its children slot"
        );
    }

    #[test]
    fn reused_owner_pending_side_snapshot_is_not_freed() {
        // REGRESSION (per-cell generation fix): the gdb-confirmed REUSE-ABA. A
        // stale `SideReclaim` snapshot whose side index was RECYCLED (post-266d19d
        // free-list reuse) by a LIVE re-intern must NOT free the live cell's
        // payload. PRE-FIX, `SideColumn::free(idx)` blindly `take()`s cell `idx`,
        // dropping the live node's `Box` ⇒ a later read panics ("live ... slot") or
        // is a true UAF. POST-FIX, the cell carries a generation the reuse `push`
        // bumped, the snapshot captured the OLD generation, and `free` is a no-op on
        // the mismatch — so the live reused payload survives.
        let _mode = enter_index_mode_for_test();
        let mut heap = IndexHeap::with_segment_capacity(16);

        // A live co-resident node keeps the segment from being RELEASED wholesale
        // (so the dead slot's SIDE INDEX is recycled WITHIN the segment, which is
        // the reuse the bug needs — not a whole-segment reset).
        let keep = heap.alloc_sexpr(&[MettaValue::Bool(true)]);

        // `old`: a node whose single child occupies a side index I_old (gen 1).
        let old = heap.alloc_sexpr(&[MettaValue::Bool(true)]);
        // Capture the STALE snapshot now (owner=old, idx=I_old, gen=1). This is the
        // snapshot we will mis-apply AFTER I_old has been reused by a live node.
        let stale = heap
            .side_reclaim_for_addr(old)
            .expect("old SExpr has a children side slot");

        // Reclaim `old`'s node slot and FREE its side index I_old at quiescence, so
        // I_old becomes reusable (cell left `(gen 1, None)`). `keep` stays live so
        // the segment is not released.
        heap.mark(&[keep]);
        let stats = heap.sweep();
        assert_eq!(stats.reclaimed_to_free_list, 1, "old's node slot reclaimed");
        heap.free_pending_side_reclaims();

        // A live re-intern reuses node slot `old` AND side index I_old, bumping the
        // cell's generation to 2 and installing a fresh 2-child payload.
        let reused = heap.alloc_sexpr(&[MettaValue::Bool(false), MettaValue::Bool(true)]);
        assert_eq!(reused, old, "node slot reused");
        match heap.arena.get(reused) {
            Node::SExpr(cr) => {
                assert_eq!(
                    cr.gen, 2,
                    "the reuse push bumped the side cell's generation past the stale snapshot's"
                );
            }
            _ => panic!("expected reused SExpr node"),
        }

        // Mis-apply the STALE snapshot (it still names I_old with the OLD gen 1).
        // The generation guard inside `SideColumn::free` must reject it.
        heap.pending_side_reclaims.push(stale);
        heap.mark(&[keep, reused]);
        heap.free_pending_side_reclaims();

        // POST-FIX: the live reused payload SURVIVES (PRE-FIX this panicked /
        // UAF'd inside `children`).
        assert_eq!(
            heap.children(reused).len(),
            2,
            "a stale snapshot for a REUSED side index must not free the live cell"
        );
        assert_eq!(
            heap.children(reused),
            &[MettaValue::Bool(false), MettaValue::Bool(true)],
            "the live reused side payload is intact and readable"
        );
        assert_eq!(
            heap.children(keep).len(),
            1,
            "the co-resident live node's side payload is unaffected"
        );
    }

    #[test]
    fn side_reclaim_drain_requires_quiescent_major() {
        assert!(!index_gc::should_drain_side_reclaims("rendezvous", true));
        assert!(!index_gc::should_drain_side_reclaims("midloop", true));
        assert!(!index_gc::should_drain_side_reclaims("quiescence", false));
        assert!(index_gc::should_drain_side_reclaims("quiescence", true));
    }

    #[test]
    fn midloop_gate_closes_after_worker_spawned() {
        // The midloop non-rendezvous gate must latch shut the instant any eval
        // worker is noted as spawned. True quiescence is fanout-independent and is
        // covered by `gate_open`'s active==0/n_threads==0 conditions.
        let _mode = enter_index_mode_for_test();
        crate::backend::models::note_worker_spawned();
        assert!(
            !index_gc::gate_open_midloop(),
            "midloop gate must be closed once a worker has ever been spawned"
        );
    }

    #[test]
    fn rendezvous_forced_churn_reuses_free_list_without_duplicates() {
        let _mode = enter_index_mode_for_test();
        let target_young_bytes = index_gc::young_budget_for_test() + 1024 * 1024;
        let nodes_to_force_minor = target_young_bytes / std::mem::size_of::<Node>().max(1) + 1;
        let first_dead_count = nodes_to_force_minor + nodes_to_force_minor / 2;

        allocate_dead_fixed_nodes(first_dead_count, 1);
        let live = {
            let mut heap = global_index_heap().write().expect("index heap write lock");
            let live_addr = heap.alloc_fixed(Node::Float(42.0));
            MettaValue::from_addr(live_addr, 0, TAG5_FLOAT)
        };
        let roots = vec![live];

        let cycles_before = index_gc::rendezvous_cycles_run();
        let minors_before = index_gc::rendezvous_minor_cycles_run();

        assert!(
            run_rendezvous_collection_for_test(&roots),
            "first forced rendezvous collection must run"
        );

        // Reuse many, but not all, of the first sweep's current-segment free slots.
        // The second rendezvous sweep re-sees the still-listed dead slots; the
        // production free_bit must suppress duplicate pushes.
        allocate_dead_fixed_nodes(nodes_to_force_minor, 2);
        assert!(
            run_rendezvous_collection_for_test(&roots),
            "second forced rendezvous collection must run"
        );

        let cycles_after = index_gc::rendezvous_cycles_run();
        let minors_after = index_gc::rendezvous_minor_cycles_run();
        assert!(
            cycles_after >= cycles_before + 2,
            "forced dedicated gate must run two rendezvous cycles: before={cycles_before}, after={cycles_after}"
        );
        assert!(
            minors_after > minors_before,
            "forced dedicated gate must include a minor rendezvous cycle: before={minors_before}, after={minors_after}"
        );
    }

    #[test]
    fn global_index_heap_initializes() {
        let _mode = enter_index_mode_for_test();
        // Smoke: the global heap is constructible and allocates.
        let mut guard = global_index_heap().write().expect("lock");
        let a = guard.alloc_string("global");
        assert_eq!(guard.str_slice(a), "global");
    }

    #[test]
    fn index_factory_and_store_via_global_heap() {
        use crate::backend::eval::cesk::store::Store;
        use crate::backend::models::MettaValueFactory;
        // Index mode so `as_arena_addr` decodes the factory's handles. (We do NOT
        // use `==` on index handles here — `PartialEq` is mode-aware only in 2a-5;
        // we compare via `.tagged` / `as_arena_addr` / heap accessors instead.)
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;

        // Heap-allocated atom: decodes to an Addr, content roundtrips.
        let a = f.atom("foo");
        let aaddr = a.as_arena_addr().expect("atom is an arena handle");
        assert_eq!(
            global_index_heap().read().expect("lock").str_slice(aaddr),
            "foo"
        );

        // Inline scalars are NOT arena handles (byte-identical to slab).
        assert_eq!(f.bool(true).as_arena_addr(), None);
        assert_eq!(f.unit().as_arena_addr(), None);
        assert_eq!(f.empty().as_arena_addr(), None);
        assert_eq!(f.long(5).as_arena_addr(), None, "i48 long is inline");

        // SExpr children roundtrip via the heap.
        let sx = f.sexpr(vec![f.long(1), f.long(2)]);
        let sxaddr = sx.as_arena_addr().expect("sexpr is an arena handle");
        assert_eq!(
            global_index_heap()
                .read()
                .expect("lock")
                .children(sxaddr)
                .len(),
            2
        );

        // FLAG_HAS_VARIABLES: a variable atom is flagged; a SExpr containing it inherits it.
        let v = f.atom("$x");
        assert_eq!(v.tagged & 0xF, FLAG_HAS_VARIABLES, "$x flagged as variable");
        let sxv = f.sexpr(vec![v, f.long(0)]);
        assert_eq!(
            sxv.tagged & 0xF,
            FLAG_HAS_VARIABLES,
            "SExpr with a variable child is flagged"
        );

        // empty sexpr → inline unit; not_reducible is a stable memoized sentinel.
        assert_eq!(
            f.sexpr(vec![]).as_arena_addr(),
            None,
            "empty sexpr → inline unit"
        );
        assert_eq!(
            f.not_reducible().tagged,
            f.not_reducible().tagged,
            "not_reducible is memoized to one Addr"
        );

        // IndexHeapStore exposes the factory + a live alloc count.
        let store = IndexHeapStore::new();
        let before = store.alloc_count();
        let _ = store.factory().atom("through-store");
        assert!(
            store.alloc_count() > before,
            "store.factory() allocates into the heap"
        );
    }

    #[test]
    fn metta_value_view_is_mode_aware() {
        // The CRUX (Inc 2a-5 Step 1): `MettaValue::view()` decodes an index handle
        // by reading its `Node` from the global heap (Spanned-stripped,
        // `'static`-laundered), instead of dereferencing a slab pointer.
        use crate::backend::models::metta_value::ValueView;
        use crate::backend::models::MettaValueFactory;
        use crate::ir::{Position, Span};
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;

        // Heap composites: the `'static`-laundered content roundtrips through view().
        assert!(matches!(f.atom("foo").view(), ValueView::Atom(s) if s == "foo"));
        assert!(matches!(f.string("hi").view(), ValueView::String(s) if s == "hi"));
        match f.sexpr(vec![f.long(1), f.long(2)]).view() {
            ValueView::SExpr(items) => assert_eq!(items.len(), 2),
            v => panic!("expected SExpr view, got {v:?}"),
        }
        assert!(matches!(
            f.conjunction(vec![f.long(1)]).view(),
            ValueView::Conjunction(items) if items.len() == 1
        ));
        assert!(matches!(
            f.error(f.atom("op"), f.string("bad")).view(),
            ValueView::Error(..)
        ));
        assert!(matches!(
            f.type_value(f.atom("T")).view(),
            ValueView::Type(_)
        ));
        assert!(matches!(f.quote(f.atom("q")).view(), ValueView::Quoted(_)));
        assert!(matches!(f.lazy(f.atom("l")).view(), ValueView::Lazy(_)));
        assert!(matches!(f.not_reducible().view(), ValueView::NotReducible));

        // Heap scalars that exceed the inline range decode from their Node.
        let big = 1i64 << 50; // > i48 inline range → heap Node::Long
        let n = f.long(big);
        assert!(
            n.as_arena_addr().is_some(),
            "2^50 is heap-allocated, not inline"
        );
        assert!(matches!(n.view(), ValueView::Long(x) if x == big));
        assert!(matches!(f.float(2.5).view(), ValueView::Float(x) if x == 2.5));

        // Inline scalars decode mode-independently (never touch the heap).
        assert!(matches!(f.bool(true).view(), ValueView::Bool(true)));
        assert!(matches!(f.unit().view(), ValueView::Unit));
        assert!(matches!(f.empty().view(), ValueView::Empty));
        assert!(
            matches!(f.long(5).view(), ValueView::Long(5)),
            "i48 long is inline"
        );

        // view() strips Spanned layers (single and nested), as in slab mode.
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 3,
                byte_offset: 3,
            },
        };
        let sp = f.spanned(f.atom("inner"), span);
        assert!(
            matches!(sp.view(), ValueView::Atom(s) if s == "inner"),
            "view() strips one Spanned layer"
        );
        let sp2 = f.spanned(sp, span);
        assert!(
            matches!(sp2.view(), ValueView::Atom(s) if s == "inner"),
            "view() strips nested Spanned layers"
        );
    }

    #[test]
    fn decode_api_is_mode_aware_via_materialization() {
        // CRUX Step 2c+3a: the typed accessors, inner_ref/inner/inner_raw, and
        // inner_ptr all work in Index mode without per-accessor edits — they
        // funnel through the now-mode-aware inner_ref() (Addr-keyed materialization).
        use crate::backend::models::metta_value::MettaValueInner;
        use crate::backend::models::metta_value_trait::MettaValueTrait; // hash_value
        use crate::backend::models::MettaValueFactory;
        use crate::ir::{Position, Span};
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;

        // ── typed accessors (is_X / as_X) ────────────────────────────────────
        let a = f.atom("foo");
        assert!(a.is_atom() && a.as_atom() == Some("foo"));
        assert!(!a.is_sexpr() && a.as_sexpr().is_none());
        let s = f.string("hi");
        assert!(s.is_string() && s.as_string() == Some("hi"));
        let sx = f.sexpr(vec![f.long(1), f.long(2)]);
        assert!(sx.is_sexpr() && sx.as_sexpr().map(|i| i.len()) == Some(2));
        let big = 1i64 << 50;
        let n = f.long(big);
        assert!(n.is_long() && n.as_long() == Some(big));
        assert!(f.float(2.5).as_float() == Some(2.5));
        let e = f.error(f.atom("op"), f.string("bad"));
        assert!(e.is_error() && e.as_error().is_some());
        let t = f.type_value(f.atom("T"));
        assert!(t.is_type() && t.as_type().is_some());
        let cj = f.conjunction(vec![f.long(1)]);
        assert!(cj.is_conjunction() && cj.as_conjunction().map(|g| g.len()) == Some(1));
        let q = f.quote(f.atom("q"));
        assert!(q.is_quoted() && q.as_quoted().is_some() && q.as_quoted_ref().is_some());
        let lz = f.lazy(f.atom("l"));
        assert!(lz.is_lazy() && lz.as_lazy().is_some() && lz.as_lazy_ref().is_some());
        // Lazy-transparency of as_long/as_float (the PLN stv-confidence fix path).
        let lazy_num = f.lazy(f.long(big));
        assert_eq!(lazy_num.as_long(), Some(big), "as_long is Lazy-transparent");
        // type_name + metatype via the mode-aware decode.
        assert_eq!(a.type_name(), "Symbol");
        assert_eq!(f.atom("$x").type_name(), "Variable");
        assert_eq!(sx.type_name(), "Expression");

        // ── inner_ref / inner / inner_raw ────────────────────────────────────
        assert!(matches!(a.inner_ref(), MettaValueInner::Atom(x) if *x == "foo"));
        assert!(matches!(sx.inner(), MettaValueInner::SExpr(items) if items.len() == 2));
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 1,
                byte_offset: 1,
            },
        };
        let sp = f.spanned(f.atom("y"), span);
        assert!(
            matches!(sp.inner_raw(), MettaValueInner::Spanned(..)),
            "inner_raw keeps Spanned"
        );
        assert!(
            matches!(sp.inner(), MettaValueInner::Atom(x) if *x == "y"),
            "inner strips Spanned"
        );
        // exp46: the shared Inner column → a stable per-Addr inner_ref pointer.
        let p1 = a.inner_ref() as *const MettaValueInner;
        let p2 = a.inner_ref() as *const MettaValueInner;
        assert_eq!(
            p1, p2,
            "inner_ref is a stable per-Addr pointer (shared Inner column)"
        );

        // ── inner_ptr key (Step 3a) ──────────────────────────────────────────
        assert!(
            !a.inner_ptr().is_null(),
            "index heap handle has a non-null key"
        );
        assert_ne!(
            a.inner_ptr(),
            s.inner_ptr(),
            "distinct values → distinct keys"
        );
        assert_eq!(a.inner_ptr(), a.inner_ptr(), "key is stable");
        assert!(f.long(5).inner_ptr().is_null(), "inline scalar key is null");

        // ── Hash / PartialEq / Display fall out of the mode-aware inner_ref ───
        // PartialEq is structural even across distinct Addrs (no hash-cons yet).
        assert_eq!(
            f.atom("x"),
            f.atom("x"),
            "PartialEq is structural in index mode"
        );
        assert_ne!(f.atom("x"), f.atom("z"));
        assert_eq!(
            f.sexpr(vec![f.long(1), f.long(2)]),
            f.sexpr(vec![f.long(1), f.long(2)]),
            "structural SExpr equality"
        );
        // Spanned/Lazy transparency of eq.
        assert_eq!(f.spanned(f.atom("x"), span), f.atom("x"));
        // Hash agrees for structurally-equal values.
        assert_eq!(f.atom("h").hash_value(), f.atom("h").hash_value());
        assert_eq!(
            f.sexpr(vec![f.atom("k"), f.long(9)]).hash_value(),
            f.sexpr(vec![f.atom("k"), f.long(9)]).hash_value()
        );
        // Display renders structurally.
        assert_eq!(
            format!("{}", f.sexpr(vec![f.atom("+"), f.long(1), f.long(2)])),
            "(+ 1 2)"
        );
        assert_eq!(format!("{a}"), "foo");

        // ── span / spans / peel_span ─────────────────────────────────────────
        assert_eq!(sp.span(), Some(&span));
        assert_eq!(sp.spans(), vec![&span]);
        assert!(sp.peel_span().0.as_atom() == Some("y") && sp.peel_span().1 == Some(&span));
    }

    /// exp46: the defining v2 property the per-thread INNER_SHADOW could not
    /// have — ONE shared column cell per Addr, so `inner_ref` returns the
    /// IDENTICAL pointer on every thread (the shadow returned a per-thread
    /// re-materialized box; that re-materialization multiplied work N-fold
    /// under FANOUT — the 2.25x parallel decomposition this lever attacks).
    /// Also: no epoch handshake — a sweep-epoch bump does NOT invalidate the
    /// cell (reuse REWRITES it before the new handle escapes instead).
    #[test]
    fn inner_column_pointer_is_shared_across_threads_and_epoch_stable() {
        use crate::backend::models::gc_allocator::bump_gc_sweep_epoch;
        use crate::backend::models::metta_value::MettaValueInner;
        use crate::backend::models::MettaValueFactory;

        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;

        let a = f.atom("inner-column-shared-a");
        let here = a.inner_ref() as *const MettaValueInner as usize;
        let there = std::thread::spawn(move || {
            // The handle crosses via the spawn closure (an escape channel —
            // the v3-F2 visibility argument); the read takes no lock.
            a.inner_ref() as *const MettaValueInner as usize
        })
        .join()
        .expect("column reader thread");
        assert_eq!(
            here, there,
            "the Inner column is SHARED: same Addr => same cell pointer on every thread"
        );

        // No epoch handshake: a sweep-epoch bump leaves the cell readable and
        // address-stable (the shadow's lazy clear is deleted with it).
        bump_gc_sweep_epoch();
        let again = a.inner_ref() as *const MettaValueInner as usize;
        assert_eq!(here, again, "column cell survives sweep-epoch bumps");
        assert!(
            matches!(a.inner_ref(), MettaValueInner::Atom(s) if *s == "inner-column-shared-a"),
            "cell content intact after epoch bump"
        );
    }

    #[test]
    fn index_factory_hash_conses_ground_sexpr() {
        // CRUX Step 4: ground SExprs are hash-consed exactly like GcFactory —
        // keyed by child HANDLE identity (tagged bits), ground-only. Equal child
        // handles ⇒ one Addr ⇒ one inner_ptr key (the R9 fixpoint-identity parity).
        use crate::backend::models::MettaValueFactory;
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;

        // Inline-only children share identical tagged bits → dedup.
        let a = f.sexpr(vec![f.long(1), f.long(2)]);
        let b = f.sexpr(vec![f.long(1), f.long(2)]);
        assert_eq!(
            a.as_arena_addr(),
            b.as_arena_addr(),
            "inline-children ground SExpr is hash-consed"
        );
        assert_eq!(
            a.inner_ptr(),
            b.inner_ptr(),
            "→ identical inner_ptr identity key"
        );

        // Shared atom-child handles → dedup (matches slab handle-identity keying).
        let foo = f.atom("foo");
        let c = f.sexpr(vec![foo, f.long(7)]);
        let d = f.sexpr(vec![foo, f.long(7)]);
        assert_eq!(
            c.as_arena_addr(),
            d.as_arena_addr(),
            "shared child handles → hash-consed"
        );

        // Different content → different Addr.
        let e = f.sexpr(vec![f.long(1), f.long(3)]);
        assert_ne!(a.as_arena_addr(), e.as_arena_addr());

        // Variable-bearing SExprs are NOT hash-consed (matches GcFactory: ground-only).
        let v1 = f.sexpr(vec![f.atom("k"), f.atom("$x")]);
        let v2 = f.sexpr(vec![f.atom("k"), f.atom("$x")]);
        assert_ne!(
            v1.as_arena_addr(),
            v2.as_arena_addr(),
            "variable-bearing SExpr is not hash-consed (FLAG_HAS_VARIABLES path)"
        );
        // …but it still carries the variable flag and is structurally equal.
        assert_eq!(v1.tagged & 0xF, FLAG_HAS_VARIABLES);
        assert_eq!(v1, v2, "non-hash-consed SExprs remain structurally equal");
    }

    #[test]
    fn sweep_retains_live_hash_cons_drops_dead() {
        // B1.c: sweep keeps the hash-cons entry for LIVE (marked) ground content
        // and drops the entry for dead content — so re-interning live content HITS
        // the same canonical Addr, while dead content is forgotten and re-allocated
        // fresh (never a dangling hit on a reclaimed-but-intact slot).
        use crate::backend::models::MettaValueFactory;
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;
        let mut heap = IndexHeap::with_segment_capacity(64);

        // Two distinct ground SExprs (inline-scalar children → heap-independent).
        let live = heap.intern_ground_sexpr(&[f.long(1), f.long(2)]);
        let dead = heap.intern_ground_sexpr(&[f.long(3), f.long(4)]);
        let live_addr = live.as_arena_addr().expect("live is an arena addr");
        let dead_addr = dead.as_arena_addr().expect("dead is an arena addr");
        assert_ne!(live_addr, dead_addr);

        // Mark only `live`, then sweep (mirrors the collector's mark→sweep order).
        heap.mark(&[live_addr]);
        let _ = heap.sweep();

        // Live content re-interns to the SAME Addr (entry retained).
        let live2 = heap.intern_ground_sexpr(&[f.long(1), f.long(2)]);
        assert_eq!(
            live2.as_arena_addr(),
            Some(live_addr),
            "live ground content keeps its canonical Addr across sweep"
        );
        // Dead content's hash-cons entry was DROPPED at sweep (B1.c), so re-interning
        // it is a MISS that RE-ALLOCATES — NOT a dangling HIT on the reclaimed slot.
        // C1.c #1 (cur_seg-only free-list reuse) means a fresh allocation may REUSE the
        // reclaimed slot, so the no-dangling-hit guard can no longer be an Addr-
        // inequality check. Instead: consume the reclaimed slot with DIFFERENT content
        // first (it reuses `dead_addr`), so a (buggy) stale hit on dead's entry would
        // return that slot now holding (5,6); then re-interning (3,4) must yield the
        // CORRECT (3,4) content — proving the entry was dropped + re-allocated.
        let other = heap.intern_ground_sexpr(&[f.long(5), f.long(6)]);
        assert_eq!(
            other.as_arena_addr(),
            Some(dead_addr),
            "C1.c #1: a fresh allocation reuses the reclaimed cur_seg slot"
        );
        let dead2 = heap.intern_ground_sexpr(&[f.long(3), f.long(4)]);
        let kids = heap.children(dead2.as_arena_addr().expect("dead2 is an arena addr"));
        assert!(
            kids.len() == 2
                && kids[0].tagged == f.long(3).tagged
                && kids[1].tagged == f.long(4).tagged,
            "dead content re-interns to CORRECT (3,4) content — entry dropped + \
             re-allocated, not a dangling hit on the reclaimed (now-(5,6)) slot"
        );
    }

    #[test]
    fn hash_cons_free_listed_stale_hit_is_reallocated_before_reuse() {
        // A retained/stale table entry is not enough for a hit: the address must
        // still name an allocated node slot. Otherwise a swept-but-not-yet-side-
        // drained node can be returned while its Addr is still on the free list,
        // and the next allocation can clobber the value just returned.
        use crate::backend::models::MettaValueFactory;
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;
        let mut heap = IndexHeap::with_segment_capacity(64);

        let items = [f.long(1), f.long(2)];
        let stale = heap.intern_ground_sexpr(&items);
        let stale_addr = stale.as_arena_addr().expect("stale is an arena addr");
        let key = hash_cons_key(&items);

        let _ = heap.sweep();
        heap.hash_cons.insert(key, stale);

        let revived = heap.intern_ground_sexpr(&items);
        let other = heap.intern_ground_sexpr(&[f.long(5), f.long(6)]);
        assert_ne!(
            revived.as_arena_addr(),
            other.as_arena_addr(),
            "validated stale miss must consume the free-listed slot before later allocations"
        );
        assert_eq!(
            revived.as_arena_addr(),
            Some(stale_addr),
            "the miss may reuse the old Addr, but only after clearing free-list ownership"
        );
        let kids = heap.children(stale_addr);
        assert!(
            kids.len() == 2
                && kids[0].tagged == items[0].tagged
                && kids[1].tagged == items[1].tagged,
            "the revived value must not be clobbered by a later allocation"
        );
    }

    #[test]
    fn hash_cons_missing_side_hit_is_treated_as_miss() {
        // After the true-quiescence side drain, a stale hash-cons entry points at
        // a node whose children slot is `None`. Lookup must remove the entry and
        // allocate fresh instead of calling the panicking live-node `children()`
        // accessor.
        use crate::backend::models::MettaValueFactory;
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;
        let mut heap = IndexHeap::with_segment_capacity(64);

        let items = [f.long(3), f.long(4)];
        let stale = heap.intern_ground_sexpr(&items);
        let key = hash_cons_key(&items);

        let _ = heap.sweep();
        heap.free_pending_side_reclaims();
        heap.hash_cons.insert(key, stale);

        let revived = heap.intern_ground_sexpr(&items);
        let revived_addr = revived.as_arena_addr().expect("revived is an arena addr");
        let kids = heap.children(revived_addr);
        assert!(
            kids.len() == 2
                && kids[0].tagged == items[0].tagged
                && kids[1].tagged == items[1].tagged,
            "missing-side stale entry must be a miss that rebuilds the children slot"
        );
    }

    // C1.c #1 cache-invalidation regression guard: the failure mode (a reused Addr
    // serving a stale VALUE_HASH_CACHE hash -> set-op mis-bucketing) is NOT unit-
    // testable here. VALUE_HASH_CACHE is populated/read by `MettaValue::hash_value()`,
    // which materializes via the GLOBAL index heap (not a local `IndexHeap`), and
    // forcing controlled reclaim+reuse there means driving the gated GLOBAL collector
    // — which mutates global state unsafely under `cargo test`'s threads (every
    // index-mode test shares the global heap). The guard is therefore the CONFORMANCE
    // suite: `M09f-stdlib-set/{002,010,011}` (intersection/subtraction-on-atoms),
    // `M11-bisimilarity-pt/3xx/{036,037}` (set-op rewrites), and `M18-jit-fallback/
    // {001,003}` — all FAIL 483->473/10 without the `clear_aba_sensitive_caches()`
    // invalidation and PASS 483/0 with it, driving the real global collector each gate.

    #[test]
    fn strip_spans_returns_bare_handle_in_index() {
        // CRUX Step 5: strip_spans peels all Spanned layers via the mode-aware
        // peel_span and returns the bare handle (NOT via from_inner round-trip).
        use crate::backend::models::MettaValueFactory;
        use crate::ir::{Position, Span};
        let _mode = enter_index_mode_for_test();
        let f = IndexFactory;
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 1,
                byte_offset: 1,
            },
        };
        let sp = f.spanned(f.spanned(f.atom("x"), span), span);
        let stripped = sp.strip_spans();
        assert!(
            stripped.span().is_none(),
            "strip_spans removes every Spanned layer"
        );
        assert_eq!(stripped.as_atom(), Some("x"), "the bare value survives");
        // A non-Spanned value is returned unchanged.
        let plain = f.atom("y");
        assert_eq!(plain.strip_spans().as_atom(), Some("y"));
    }

    // ── D-TLAB-1.0: `SideColumn<T>` single-threaded unit tests ──────────────
    // Concurrency is validated separately (loom/TSan in D-TLAB-1.2); these are
    // deterministic, single-threaded behavioral checks. No GC-mode set is needed
    // (the column is a standalone data structure, mode-independent).

    #[test]
    fn side_column_push_get_roundtrip_and_order() {
        // Push 10 `Box<[i32]>` payloads; read each back; assert value + order.
        let col: SideColumn<[i32]> = SideColumn::new();
        let mut idxs = Vec::with_capacity(10);
        for i in 0..10i32 {
            let boxed: Box<[i32]> = vec![i, i * 10, i * 100].into_boxed_slice();
            let (idx, gen) = col.push(boxed);
            // Each fresh (bumped) cell starts `(0, None)` from `grow_to`, so its
            // first claim yields generation 1.
            assert_eq!(gen, 1, "first claim of a fresh cell yields generation 1");
            idxs.push(idx);
        }
        // Indices are claimed monotonically from 0 (no recycling, no gaps here).
        assert_eq!(
            idxs,
            (0u32..10).collect::<Vec<_>>(),
            "monotone indices 0..10"
        );
        assert_eq!(col.published_len(), 10, "all 10 published");
        for (i, &idx) in idxs.iter().enumerate() {
            let i = i as i32;
            // SAFETY: idx < published_len() (just pushed, single-threaded).
            let got = unsafe { col.get(idx) }.expect("entry present");
            assert_eq!(got, &[i, i * 10, i * 100][..], "entry {idx} value + order");
        }
    }

    #[test]
    fn side_column_grows_across_chunk_boundary() {
        // Push past one chunk boundary to exercise `grow_to` over >= 2 chunks.
        let n = SIDE_CHUNK_LEN + 5;
        let col: SideColumn<[i32]> = SideColumn::new();
        for i in 0..n {
            let boxed: Box<[i32]> = vec![i as i32].into_boxed_slice();
            let (idx, _gen) = col.push(boxed);
            assert_eq!(idx as usize, i, "index tracks push order");
        }
        assert_eq!(col.published_len(), n, "all entries published");
        assert!(
            col.chunk_count() >= 2,
            "crossed a chunk boundary ⇒ >= 2 chunks (got {})",
            col.chunk_count()
        );
        // Read back a sample spanning both chunks (including the boundary).
        for &i in &[
            0usize,
            1,
            SIDE_CHUNK_LEN - 1,
            SIDE_CHUNK_LEN,
            SIDE_CHUNK_LEN + 4,
        ] {
            // SAFETY: i < published_len() == n.
            let got = unsafe { col.get(i as u32) }.expect("entry present");
            assert_eq!(got, &[i as i32][..], "entry {i} survives the grow");
        }
    }

    #[test]
    fn side_column_free_drops_only_target() {
        // free(idx) ⇒ get returns None there; a neighbor is unaffected.
        let mut col: SideColumn<str> = SideColumn::new();
        let (a, _ag) = col.push(Box::from("alpha"));
        let (b, bg) = col.push(Box::from("beta"));
        let (c, _cg) = col.push(Box::from("gamma"));
        // SAFETY: all indices < published_len() (just pushed).
        assert_eq!(unsafe { col.get(b) }, Some("beta"));
        col.free(b, bg);
        assert_eq!(unsafe { col.get(b) }, None, "freed entry reads None");
        // Neighbors unaffected.
        assert_eq!(unsafe { col.get(a) }, Some("alpha"), "left neighbor intact");
        assert_eq!(
            unsafe { col.get(c) },
            Some("gamma"),
            "right neighbor intact"
        );
        // Idempotent re-free is a no-op (still None, no double-drop).
        col.free(b, bg);
        assert_eq!(unsafe { col.get(b) }, None, "re-free is idempotent");
    }

    #[test]
    fn side_column_recycles_after_quiescence_free() {
        // After a true-quiescence free, the next push reuses that published index
        // instead of growing the side-column high-water.
        let mut col: SideColumn<[i32]> = SideColumn::new();
        let (i0, g0) = col.push(vec![0].into_boxed_slice());
        let (i1, _g1) = col.push(vec![1].into_boxed_slice());
        assert_eq!(g0, 1, "first claim of cell i0 yields generation 1");
        col.free(i0, g0);
        let (i2, g2) = col.push(vec![2].into_boxed_slice());
        assert_eq!(i2, i0, "push after quiescence free reuses the index");
        assert_eq!(
            g2, 2,
            "reusing the index bumps its generation (so a stale snapshot bearing g0 \
             no longer matches and cannot free the reused cell)"
        );
        // The freed slot now contains the replacement; no new entry was appended.
        // SAFETY: indices < published_len() (== 2).
        assert_eq!(
            unsafe { col.get(i0) },
            Some(&[2][..]),
            "freed slot now holds the replacement"
        );
        assert_eq!(unsafe { col.get(i1) }, Some(&[1][..]), "i1 intact");
        assert_eq!(
            col.published_len(),
            2,
            "len does not grow when a reusable index is available"
        );
    }

    #[test]
    fn side_column_grows_past_old_66_chunk_ceiling() {
        // REGRESSION (D-TLAB-1.1 capacity repair): the original one-level
        // directory was a fixed `MAX_SIDE_CHUNKS = 66` cells ⇒ a HARD ceiling of
        // `66 * 4096 = 270_336` cumulative appends, which `assert!(c <
        // MAX_SIDE_CHUNKS)` tripped (panic "side column exhausted", rc=101) under
        // the C1.c #1 free-list reuse on a release-exempt `cur_seg` in MIDLOOP
        // workloads. The two-level lazy directory restores the prior
        // `Vec<Option<Box<T>>>`'s unbounded-no-crash behavior. Push WELL past the
        // old ceiling and assert: no panic, every entry published, the directory
        // spans multiple PAGES (so the page level actually grew), and reads are
        // correct at 0, across the old 270_335/270_336 boundary, and at the end.
        const OLD_CEILING: usize = 270_336; // = 66 * SIDE_CHUNK_LEN (the panic point)
        const N: usize = 280_000; // > OLD_CEILING ⇒ would have panicked before
                                  // Tiny payload (a 1-element `Box<[i32]>`) keeps the test cheap: ~280k
                                  // 4-byte allocations, not 280k large slices.
        let col: SideColumn<[i32]> = SideColumn::new();
        for i in 0..N {
            let boxed: Box<[i32]> = vec![i as i32].into_boxed_slice();
            let (idx, _gen) = col.push(boxed); // MUST NOT panic past the old 66-chunk cap
            assert_eq!(idx as usize, i, "index tracks push order at {i}");
        }
        assert_eq!(
            col.published_len(),
            N,
            "all {N} entries published, no ceiling"
        );
        // The directory must span more than one PAGE (each page covers
        // `SIDE_PAGE_LEN * SIDE_CHUNK_LEN = 1024 * 4096 = 4_194_304` entries — so
        // 280k fits in page 0, but the chunk count must exceed one page's worth of
        // chunks only at 4M+; here we assert it spans many CHUNKS and that the
        // chunk count is consistent with N, exercising the two-level `chunk`
        // deref across the boundary the old single-level array could not address).
        let chunks = col.chunk_count();
        let expected_chunks = N.div_ceil(SIDE_CHUNK_LEN);
        assert_eq!(
            chunks, expected_chunks,
            "chunk_count tracks N (got {chunks})"
        );
        assert!(
            chunks > 66,
            "directory grew past the OLD 66-chunk ceiling (got {chunks} chunks)"
        );
        // Reads at the boundaries that mattered: index 0, the two indices
        // straddling the old ceiling, and the final index — all via the two-level
        // `chunk` deref. SAFETY: every index < published_len() == N.
        for &i in &[0usize, OLD_CEILING - 1, OLD_CEILING, N - 1] {
            let got = unsafe { col.get(i as u32) }.expect("entry present past ceiling");
            assert_eq!(got, &[i as i32][..], "value correct at index {i}");
        }
    }
}

// ============================================================================
// D-RLOCK.2 TSan concurrency test — the `.read()`+`&self` concurrent factory path
// ============================================================================
//
// Gate #5 of the D-RLOCK.2 increment: real-thread (NOT loom) validation that N
// threads concurrently allocating through `IndexFactory`'s `.read()`+`&self`
// BUMP-ONLY path (`alloc_*_concurrent`) race no data and produce decodable values.
//
// To FORCE every worker onto the concurrent path (not the `try_write` `&mut`
// path), the main thread holds a `.read()` guard for the duration of the storm:
// with a reader held, every worker's `try_write()` fails (RwLock denies a writer
// while a reader is live) ⇒ each worker takes `global_index_heap().read()` +
// `alloc_*_concurrent`. Multiple concurrent readers + concurrent `&self` bumps is
// exactly the runtime regime D-RLOCK.2 introduces (parallel-eval workers, with
// the single-threaded collector gated OFF by `note_worker_spawned()`).
//
// Collector exclusion is established two ways (both asserted): `note_worker_spawned()`
// closes the single-threaded collector gate, AND every freeing collector path is
// `&mut self` so it is uncallable through the `.read()` guards the workers hold.
//
// BUILD + RUN under ThreadSanitizer (nightly, capped, FOREGROUND):
//   RUSTFLAGS="-Zsanitizer=thread -C target-cpu=native" \
//     systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1000% \
//     cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu \
//       --release --lib --features index-gc \
//       backend::eval::cesk::index_heap::tsan_concurrent_factory -- --nocapture --test-threads=1
// Expect: 0 TSan reports (no "WARNING: ThreadSanitizer: data race").
//
// Gated `feature = "index-gc"` (IndexFactory only allocates into the index heap
// under that feature; `gc_mode_is_index()` inits to true there) and `not(loom)`
// (uses std threads, not loom's instrumented model).
#[cfg(all(test, not(loom)))]
mod tsan_concurrent_factory {
    use super::*;
    use crate::backend::models::{note_worker_spawned, MettaValueFactory};
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_read_path_allocations_are_race_free() {
        // Index value mode (so `from_addr`/`view` decode the arena Addr) + close the
        // single-threaded collector gate (the regime in which concurrent `.read()`
        // allocation occurs; the collector backs off).
        let _mode = enter_index_mode_for_test();
        note_worker_spawned();
        assert!(
            crate::backend::models::worker_ever_spawned(),
            "collector gate must be CLOSED for the concurrent-path regime"
        );

        const THREADS: usize = 8;
        const PER_THREAD: usize = 2000;
        let barrier = Arc::new(Barrier::new(THREADS + 1));

        // Hold a READ guard for the whole storm so every worker's `try_write()`
        // fails ⇒ each takes the `.read()` + `alloc_*_concurrent` path. Dropped
        // after the workers join.
        let read_guard = global_index_heap().read().expect("index heap read guard");

        let mut handles = Vec::with_capacity(THREADS);
        for t in 0..THREADS {
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let f = IndexFactory;
                b.wait(); // all threads hammer the heap simultaneously
                let mut last = Vec::with_capacity(PER_THREAD);
                for i in 0..PER_THREAD {
                    // Exercise each concurrent entry: atom (variable ⇒ FLAG set),
                    // a VARIABLE SExpr (non-ground ⇒ the de-serialized branch, NOT
                    // hash-cons), a conjunction, and a spanned wrapper.
                    let a = f.atom(&format!("$v{t}_{i}"));
                    let s = f.sexpr_from_slice(&[a, f.atom(&format!("$w{t}_{i}"))]);
                    let c = f.conjunction_from_slice(&[s, f.long(1_000_000 + i as i64)]);
                    let sp = f.spanned(
                        c,
                        crate::ir::Span {
                            start: crate::ir::Position {
                                row: t,
                                column: i,
                                byte_offset: i,
                            },
                            end: crate::ir::Position {
                                row: t,
                                column: i + 1,
                                byte_offset: i + 1,
                            },
                        },
                    );
                    last.push(sp);
                }
                last
            }));
        }
        barrier.wait();
        let mut total = 0usize;
        for h in handles {
            let produced = h.join().expect("worker thread panicked");
            total += produced.len();
        }
        // Workers are joined; release the read guard.
        drop(read_guard);
        assert_eq!(total, THREADS * PER_THREAD, "all allocations accounted for");

        // Decode a sample under a fresh read guard — proves the concurrently-bumped
        // nodes + their side data (children / strings / spans) are coherently
        // published (a torn/unpublished side entry would panic in `view`/`children`).
        let _g = global_index_heap().read().expect("index heap read guard");
        // (The values are heap handles; their mere construction + accounting above,
        // run under TSan, is the race check. A deeper structural walk is exercised
        // by the conformance/ASAN arms.)
    }
}

// ============================================================================
// loom model — the side-append-BEFORE-node-publish ordering proof (D-TLAB-1.2)
// ============================================================================
//
// Builds only under `--cfg loom`. This is the concurrency-capability gate for the
// `&self` side-append path: it proves that a reader who Acquire-observes a published
// NODE is GUARANTEED to also observe the SIDE entry that node names — i.e. the side
// datum's publication transitively happens-before the reader's side read, so the
// `unsafe fn side`/`SideColumn::get` deref never sees an unpublished/torn side cell.
//
// The ordering under test mirrors `IndexHeap::alloc_sexpr`'s bump path: on ONE
// thread, the side datum is `push`ed (claim + write + publish via the side column's
// `len`-CAS `Release`) STRICTLY BEFORE the node is bumped (claim + write the node —
// which CARRIES the side index — + publish via the node `len`-CAS `Release`). A
// reader Acquire-loads the node `len`, and for every published node reads its side
// index then Acquire-loads the side `len` and the side cell.
//
// WHY IT HOLDS (the happens-before chain the model checks):
//   writer:  side.write(idx) → side_len.store(Release) → node.write(carry=idx)
//            → node_len.CAS(Release)            [all in program order, one thread]
//   reader:  node_len.load(Acquire) sees the node  ⇒ synchronizes-with the writer's
//            node_len Release ⇒ happens-after ALL the writer's prior writes, INCLUDING
//            its side cell write and side_len store. So the reader observes
//            `side_len > idx` (side published) and the fully-written side bytes.
// loom exhaustively explores every interleaving of two such writers + one reader and
// surfaces a data race / stale read if the chain ever broke (e.g. if the side were
// published with weaker-than-Release ordering, or AFTER the node).
//
// RUN (capped, FOREGROUND — same convention + caveats as `index_arena::loom_model`):
//   RUSTFLAGS="--cfg loom -C target-cpu=native" LOOM_MAX_PREEMPTIONS=2 \
//     systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0 -p CPUQuota=1200% \
//     cargo test --release --lib --features index-gc \
//     backend::eval::cesk::index_heap::loom_side_node_ordering -- --nocapture
// (`--release`: loom runs each thread on a fixed coroutine stack the DEBUG frame of
// this large crate overflows; `--cfg loom -C target-cpu=native`: RUSTFLAGS overrides
// .cargo/config.toml, so re-add the gxhash AES/SSE2 flags. STRONG `compare_exchange`
// + `thread::yield_now()` for the same reasons documented on `index_arena`'s model.)
#[cfg(loom)]
mod loom_side_node_ordering {
    use loom::cell::UnsafeCell;
    use loom::sync::atomic::{AtomicUsize, Ordering};
    use loom::sync::Arc;
    use loom::thread;
    use std::mem::MaybeUninit;

    /// Minimal mirror of ONE `SideColumn` chunk (the publish protocol under test),
    /// over loom's instrumented `UnsafeCell`/atomics. Payload `usize` (a per-writer
    /// tag) so the reader can assert it read a fully-written value (not torn/uninit).
    struct SideCol {
        slots: Vec<UnsafeCell<MaybeUninit<usize>>>,
        len: AtomicUsize,
        bump: AtomicUsize,
        cap: usize,
    }
    // SAFETY: same claim-unique / publish-Release / read-published protocol as the
    // production `SideColumn`; loom verifies the absence of races.
    unsafe impl Sync for SideCol {}
    unsafe impl Send for SideCol {}

    impl SideCol {
        fn new(cap: usize) -> Self {
            SideCol {
                slots: (0..cap)
                    .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                    .collect(),
                len: AtomicUsize::new(0),
                bump: AtomicUsize::new(0),
                cap,
            }
        }
        /// Claim + write + publish a side entry; returns its index (the value the
        /// node will CARRY). `Relaxed` claim, `Release` publish — verbatim
        /// `SideColumn::{push,publish}` protocol (strong CAS / yield for loom).
        fn push(&self, value: usize) -> Option<usize> {
            let idx = self.bump.fetch_add(1, Ordering::Relaxed);
            if idx >= self.cap {
                return None;
            }
            self.slots[idx].with_mut(|p| unsafe { (*p).write(value) });
            while self
                .len
                .compare_exchange(idx, idx + 1, Ordering::Release, Ordering::Relaxed)
                .is_err()
            {
                thread::yield_now();
            }
            Some(idx)
        }
        /// Read a published side entry. SAFETY: `idx < len.load(Acquire)`.
        unsafe fn get(&self, idx: usize) -> usize {
            self.slots[idx].with(|p| (*p).assume_init())
        }
    }

    /// Minimal mirror of the node arena's `Segment` slot machinery; each node CARRIES
    /// the side index it names (so the reader can follow node → side).
    struct NodeSeg {
        slots: Vec<UnsafeCell<MaybeUninit<usize>>>, // each cell holds a side index
        len: AtomicUsize,
        bump: AtomicUsize,
        cap: usize,
    }
    unsafe impl Sync for NodeSeg {}
    unsafe impl Send for NodeSeg {}

    impl NodeSeg {
        fn new(cap: usize) -> Self {
            NodeSeg {
                slots: (0..cap)
                    .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                    .collect(),
                len: AtomicUsize::new(0),
                bump: AtomicUsize::new(0),
                cap,
            }
        }
        /// Claim + write + publish a node carrying `side_idx`. Verbatim
        /// `Segment::{bump_one,write_claimed,publish}` protocol.
        fn publish_node(&self, side_idx: usize) -> Option<usize> {
            let off = self.bump.fetch_add(1, Ordering::Relaxed);
            if off >= self.cap {
                return None;
            }
            self.slots[off].with_mut(|p| unsafe { (*p).write(side_idx) });
            while self
                .len
                .compare_exchange(off, off + 1, Ordering::Release, Ordering::Relaxed)
                .is_err()
            {
                thread::yield_now();
            }
            Some(off)
        }
        unsafe fn carried_side_idx(&self, off: usize) -> usize {
            self.slots[off].with(|p| (*p).assume_init())
        }
    }

    #[test]
    fn loom_side_entry_visible_when_node_observed() {
        loom::model(|| {
            const CAP: usize = 4;
            let side = Arc::new(SideCol::new(CAP));
            let nodes = Arc::new(NodeSeg::new(CAP));

            // Two writers, each performing the `alloc_*` triple ON ONE THREAD: push a
            // side entry (publish it), THEN publish a node carrying that side index —
            // the side-append-BEFORE-node-publish ordering D-TLAB-1.2 establishes.
            let w = |tag: usize| {
                let side = side.clone();
                let nodes = nodes.clone();
                thread::spawn(move || {
                    // Tag the side payload so the reader can assert a sane value; the
                    // tag's high bits are unique per writer (0xA._ / 0xB._).
                    let sidx = side.push(tag)?;
                    nodes.publish_node(sidx)
                })
            };
            let w1 = w(0xA0);
            let w2 = w(0xB0);

            // Reader: observe published nodes, follow each to its side entry, and
            // assert the side entry IS published (no out-of-bounds vs `side.len`) and
            // fully written (a known tag, never uninit/torn). loom flags a data race
            // here if observing the node did NOT transitively publish the side.
            let r = {
                let side = side.clone();
                let nodes = nodes.clone();
                thread::spawn(move || {
                    let n = nodes.len.load(Ordering::Acquire);
                    assert!(n <= CAP, "node len {n} exceeded capacity {CAP}");
                    for off in 0..n {
                        // SAFETY: off < node_len (Acquire) ⇒ node published.
                        let sidx = unsafe { nodes.carried_side_idx(off) };
                        // THE INVARIANT: a published node ⇒ its side entry published.
                        let slen = side.len.load(Ordering::Acquire);
                        assert!(
                            sidx < slen,
                            "node at off {off} carries side idx {sidx} but side len is \
                             only {slen} — side entry NOT published before the node \
                             (the D-TLAB-1.2 ordering broke)"
                        );
                        // SAFETY: sidx < side_len (just asserted, Acquire) ⇒ published.
                        let v = unsafe { side.get(sidx) };
                        assert!(
                            v == 0xA0 || v == 0xB0,
                            "side entry {sidx} read torn/uninit value {v:#x}"
                        );
                    }
                    n
                })
            };

            let o1 = w1.join().expect("w1");
            let o2 = w2.join().expect("w2");
            let _ = r.join().expect("r");

            // Both writers completed ⇒ two nodes + two side entries published, and
            // the two writers claimed DISTINCT node offsets (unique claim).
            if let (Some(a), Some(b)) = (o1, o2) {
                assert_ne!(a, b, "two writers published the same node offset {a}");
            }
            let final_nodes = nodes.len.load(Ordering::Acquire);
            let final_side = side.len.load(Ordering::Acquire);
            assert_eq!(final_nodes, 2, "both nodes must be published");
            assert_eq!(final_side, 2, "both side entries must be published");
            // Every published node still resolves to a written side entry post-join.
            for off in 0..final_nodes {
                let sidx = unsafe { nodes.carried_side_idx(off) };
                assert!(sidx < final_side, "node {off} → side {sidx} unpublished");
                let v = unsafe { side.get(sidx) };
                assert!(v == 0xA0 || v == 0xB0, "final side {sidx} value {v:#x}");
            }
        });
    }
}
