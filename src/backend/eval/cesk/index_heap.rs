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
//! bump the node slot *and* their side data into the *same* segment and never
//! take a free-list slot.
//!
//! Inc 2 is in progress: this module is complete + unit-tested in isolation and
//! not yet wired to the value model (`#![allow(dead_code)]`). A single global
//! `RwLock<IndexHeap>` backs the future `IndexHeapStore` (Inc 2a-4); the
//! lock-free per-thread TLAB refinement is Inc 5.

#![allow(dead_code)]

use std::sync::{OnceLock, RwLock};

use crate::backend::eval::cesk::index_arena::{Addr, ArenaNode, IndexArena, SweepStats};
use crate::backend::eval::cesk::index_node::{ByteRef, ChildRef, Node, SpanRef};
use crate::backend::eval::cesk::store::Store;
use crate::backend::models::gc_allocator::hash_cons_key;
use crate::backend::models::metta_value::{
    is_variable_str, MettaValueInner, ValueView, FLAG_HAS_VARIABLES,
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
/// `Box` DATA is freed promptly; only the `Vec` SPINE grows (one `Option` ptr per
/// ever-interned datum), recovered wholesale at segment release. (A safe index
/// recycler would need an occupancy bitmap — a future refinement.)
#[derive(Default)]
struct SegmentSideArenas {
    children: Vec<Option<Box<[MettaValue]>>>,
    strings: Vec<Option<Box<str>>>,
    spans: Vec<Option<Box<Span>>>,
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

/// The index-arena value heap.
pub struct IndexHeap {
    arena: IndexArena<Node>,
    /// `sides[i]` holds segment `i`'s variable-length data (kept parallel with
    /// `arena.segments` via [`sync_sides`](Self::sync_sides)).
    sides: Vec<SegmentSideArenas>,
    /// Increment A: the slots the most recent `sweep`/`sweep_young` reclaimed, stashed for
    /// the driver to free their side `Box`es via [`free_reclaimed_side_slots`] — but ONLY
    /// at quiescence (a midloop sweep leaves this for the next quiescence sweep, for
    /// launder soundness). `std::mem::take`-n by the driver after each collection.
    last_reclaimed: Vec<Addr>,
    /// `Arc`-backed `Space` handles, indexed by the `u64` id stored in
    /// `Node::Space(id)`. Append-only and never swept (handles are env-rooted).
    space_table: Vec<SpaceHandle>,
    /// `Arc`-backed `Memo` handles, indexed by `Node::Memo(id)`.
    memo_table: Vec<MemoHandle>,
    /// Ground-SExpr hash-cons table (CRUX Step 4): content hash of children's
    /// `tagged` bits → the canonical interned handle. Mirrors the slab's
    /// thread-local table (`gc_allocator.rs`) exactly — ground-SExpr-only, the
    /// shared [`hash_cons_key`], one entry per key (best-effort; a hash collision
    /// overwrites), capped at 8192 — so equal ground content yields the same
    /// `Addr` ⇒ the same `inner_ptr` key, matching Slab's fixpoint/cycle/dedup
    /// identity sites for the Inc-3 A/B differential (R9). Cleared on
    /// [`sweep`](Self::sweep) (Inc 6 wires the live sweep; until then it is a
    /// bounded monotone intern table, like the slab table between safepoints).
    hash_cons: std::collections::HashMap<u64, MettaValue>,
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
        let mut h = IndexHeap {
            arena,
            sides: Vec::new(),
            last_reclaimed: Vec::new(),
            space_table: Vec::new(),
            memo_table: Vec::new(),
            hash_cons: std::collections::HashMap::new(),
        };
        h.sync_sides();
        h
    }

    /// Grow `sides` so it stays index-parallel with the arena's segments (the
    /// arena may open segments internally during allocation).
    #[inline]
    fn sync_sides(&mut self) {
        while self.sides.len() < self.arena.segment_count() {
            self.sides.push(SegmentSideArenas::default());
        }
    }

    // ── Allocation ───────────────────────────────────────────────────────

    /// Allocate a fixed-size node (no side-arena data). Free-list reuse is fine.
    pub fn alloc_fixed(&mut self, node: Node) -> Addr {
        let a = self.arena.alloc(node);
        self.sync_sides();
        a
    }

    /// Register a `Space` handle and allocate its node (id → side table).
    pub fn alloc_space(&mut self, handle: SpaceHandle) -> Addr {
        let id = self.space_table.len() as u64;
        self.space_table.push(handle);
        self.alloc_fixed(Node::Space(id))
    }

    /// Register a `Memo` handle and allocate its node.
    pub fn alloc_memo(&mut self, handle: MemoHandle) -> Addr {
        let id = self.memo_table.len() as u64;
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
            let cs = self.intern_children(items);
            let seg = self.arena.ensure_bump_room();
            self.arena.bump_in(seg, Node::SExpr(cs))
        }
    }

    /// Allocate a `Conjunction`, co-locating its goals (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_conjunction(&mut self, goals: &[MettaValue]) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let cr = self.intern_children_in(addr.segment(), goals);
            self.arena.write_reused(addr, Node::Conjunction(cr));
            addr
        } else {
            let cs = self.intern_children(goals);
            let seg = self.arena.ensure_bump_room();
            self.arena.bump_in(seg, Node::Conjunction(cs))
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
        if let Some(&existing) = self.hash_cons.get(&key) {
            if let Some(addr) = existing.as_arena_addr() {
                let kids = self.children(addr);
                if kids.len() == items.len()
                    && kids.iter().zip(items).all(|(a, b)| a.tagged == b.tagged)
                {
                    return existing;
                }
            }
        }
        // Miss (or hash collision → overwrite, matching the slab table's
        // last-writer-wins-per-key best-effort behavior).
        let addr = self.alloc_sexpr(items);
        let v = MettaValue::from_addr(addr, 0); // ground ⇒ no FLAG_HAS_VARIABLES
        if self.hash_cons.len() < 8192 {
            self.hash_cons.insert(key, v);
        }
        v
    }

    /// Allocate an `Atom`, co-locating its bytes (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_atom(&mut self, s: &str) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let br = self.intern_bytes_in(addr.segment(), s);
            self.arena.write_reused(addr, Node::Atom(br));
            addr
        } else {
            let bs = self.intern_bytes(s);
            let seg = self.arena.ensure_bump_room();
            self.arena.bump_in(seg, Node::Atom(bs))
        }
    }

    /// Allocate a `String`, co-locating its bytes (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_string(&mut self, s: &str) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let br = self.intern_bytes_in(addr.segment(), s);
            self.arena.write_reused(addr, Node::String(br));
            addr
        } else {
            let bs = self.intern_bytes(s);
            let seg = self.arena.ensure_bump_room();
            self.arena.bump_in(seg, Node::String(bs))
        }
    }

    /// Allocate a `Spanned`, co-locating the (boxed) span (reuse-or-bump, as `alloc_sexpr`).
    pub fn alloc_spanned(&mut self, inner: MettaValue, span: Span) -> Addr {
        if let Some(addr) = self.arena.pop_young_free_slot() {
            let sr = self.intern_span_in(addr.segment(), span);
            self.arena.write_reused(addr, Node::Spanned(inner, sr));
            addr
        } else {
            let seg = self.arena.ensure_bump_room();
            let sr = self.intern_span_in(seg, span);
            self.arena.bump_in(seg, Node::Spanned(inner, sr))
        }
    }

    /// C1.c #1: box `items` into segment `seg`'s child side-arena, REUSING a freed
    /// index (a swept-dead slot, from `free_children`) if available else APPENDING,
    /// returning the segment-relative index. The caller co-locates the owning node
    /// in the SAME `seg` (so node + children co-release, and `children(addr)` reads
    /// `sides[addr.segment()]`). The reuse path passes the REUSED node-slot's
    /// segment so the side data lands WITH the node (not a possibly-advanced
    /// `cur_seg`, which would break co-location + bump order). Reuse keeps the side
    /// `Vec` bounded by live+free entries, not total-ever — so a minor's reclaim is
    /// not wasted.
    fn intern_children_in(&mut self, seg: usize, items: &[MettaValue]) -> ChildRef {
        self.sync_sides();
        let side = &mut self.sides[seg];
        let idx = side.children.len() as u32;
        side.children.push(Some(items.to_vec().into_boxed_slice()));
        ChildRef { idx }
    }

    fn intern_bytes_in(&mut self, seg: usize, s: &str) -> ByteRef {
        self.sync_sides();
        let side = &mut self.sides[seg];
        let idx = side.strings.len() as u32;
        side.strings.push(Some(s.to_string().into_boxed_str()));
        ByteRef { idx }
    }

    fn intern_span_in(&mut self, seg: usize, span: Span) -> SpanRef {
        self.sync_sides();
        let side = &mut self.sides[seg];
        let idx = side.spans.len() as u32;
        side.spans.push(Some(Box::new(span)));
        SpanRef { idx }
    }

    /// Bump-path interning: into the current bump segment (`ensure_bump_room`).
    fn intern_children(&mut self, items: &[MettaValue]) -> ChildRef {
        let seg = self.arena.ensure_bump_room();
        self.intern_children_in(seg, items)
    }

    fn intern_bytes(&mut self, s: &str) -> ByteRef {
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
            Node::SExpr(cr) | Node::Conjunction(cr) => self.sides[addr.segment()].children
                [cr.idx as usize]
                .as_deref()
                .expect("live SExpr/Conjunction children slot"),
            _ => panic!("children() on a non-SExpr/Conjunction node"),
        }
    }

    /// The string of an `Atom`/`String` at `addr`.
    pub fn str_slice(&self, addr: Addr) -> &str {
        match self.arena.get(addr) {
            Node::Atom(br) | Node::String(br) => self.sides[addr.segment()].strings
                [br.idx as usize]
                .as_deref()
                .expect("live Atom/String slot"),
            _ => panic!("str_slice() on a non-Atom/String node"),
        }
    }

    /// The `Span` of a `Spanned` at `addr`.
    pub fn span_at(&self, addr: Addr) -> Span {
        match self.arena.get(addr) {
            Node::Spanned(_, sr) => *self.sides[addr.segment()].spans[sr.idx as usize]
                .as_deref()
                .expect("live Spanned slot"),
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
            Node::Atom(_) => ValueView::Atom(unsafe { launder(self.str_slice(a)) }),
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
            Node::Atom(_) => MettaValueInner::Atom(unsafe { launder(self.str_slice(addr)) }),
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
                let span: &'static Span = unsafe {
                    launder(
                        self.sides[addr.segment()].spans[sr.idx as usize]
                            .as_deref()
                            .expect("live Spanned slot"),
                    )
                };
                MettaValueInner::Spanned(*inner, span)
            }
        }
    }

    // ── Collection ─────────────────────────────────────────────────────────

    /// Transitively mark every node reachable from `roots`, resolving
    /// `SExpr`/`Conjunction` children from the child side-arena. Returns the
    /// count newly marked. Stack-safe (delegates to `mark_from_roots_with`).
    pub fn mark(&self, roots: &[Addr]) -> usize {
        let arena = &self.arena;
        let sides = &self.sides;
        arena.mark_from_roots_with(roots, |addr, out| {
            let node = arena.get(addr);
            node.child_addrs(out); // Error/Type/Quoted/Lazy/Spanned inline handles
            if let Node::SExpr(cr) | Node::Conjunction(cr) = node {
                let kids = sides[addr.segment()].children[cr.idx as usize]
                    .as_deref()
                    .expect("live SExpr/Conjunction children slot");
                for c in kids.iter() {
                    if let Some(a) = c.as_arena_addr() {
                        out.push(a);
                    }
                }
            }
        })
    }

    /// C1.c: YOUNG-ONLY mark — the MINOR's mark (see
    /// [`IndexArena::mark_young_from_roots_with`] for the soundness theorem). Same
    /// child resolution as [`mark`] (inline handles + SExpr/Conjunction side-arena
    /// children) but marks/descends only young nodes (`seg >= young_floor`), making
    /// the minor O(young reachable) instead of O(total live). The child resolver is
    /// invoked only on young nodes (the worklist holds only young addrs), so old
    /// segments are never even read.
    pub fn mark_young(&self, roots: &[Addr]) -> usize {
        let arena = &self.arena;
        let sides = &self.sides;
        arena.mark_young_from_roots_with(roots, |addr, out| {
            let node = arena.get(addr);
            node.child_addrs(out); // Error/Type/Quoted/Lazy/Spanned inline handles
            if let Node::SExpr(cr) | Node::Conjunction(cr) = node {
                let kids = sides[addr.segment()].children[cr.idx as usize]
                    .as_deref()
                    .expect("live SExpr/Conjunction children slot");
                for c in kids.iter() {
                    if let Some(a) = c.as_arena_addr() {
                        out.push(a);
                    }
                }
            }
        })
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
        let stats = {
            let sides = &mut self.sides;
            self.arena.sweep_with(
                |seg| {
                    if seg < sides.len() {
                        sides[seg] = SegmentSideArenas::default();
                    }
                },
                &mut reclaimed,
            )
        };
        // C1.c #1 (Increment A): stash the reclaimed (partial-segment) dead slots; the
        // driver frees their payload `Box`es via `free_reclaimed_side_slots` ONLY at
        // quiescence (launder soundness). Released segments were reset wholesale above.
        self.last_reclaimed = reclaimed;
        stats
    }

    /// C1.c #1: free the co-located side-arena entry (children/string/span `Box`) of
    /// each reclaimed (swept-dead, partial-segment) node — bounding side-arena growth
    /// under variable-length node-slot reuse, since a slot reused by `alloc_sexpr`/etc.
    /// orphans the prior occupant's side `Box` (a leak otherwise bounded only by
    /// segment release, which resets the whole `sides[seg]`). `reclaimed` is the slots
    /// `sweep`/`sweep_young` reclaimed (NOT released-segment slots). The node bytes are
    /// still intact (the sweep frees a slot WITHOUT clobbering it), so `arena.get(addr)`
    /// reads the dead node's side index; the slot is `None`d so its `Box` drops — and
    /// the index is NOT recycled (intern only ever APPENDS fresh indices), so `None`-ing
    /// index `i` touches no live node and is idempotent (a re-reclaimed still-free slot
    /// just `None`s the same already-`None` index again). Recycling — which would hand
    /// `i` to a live node — is the unsound path an earlier gate's "live … slot" panic
    /// caught, hence the no-recycle design.
    ///
    /// QUIESCENCE-ONLY (Phase C Increment A — the soundness gate). The driver calls this
    /// ONLY for `phase == "quiescence"` (`active_evaluator_count() == 0`). Freeing a side
    /// `Box` at sweep is a USE-AFTER-FREE under the MID-LOOP collector: a live
    /// `materialize_inner` result on the Rust stack can hold a launder'd `&'static` into
    /// that `Box` (the launder contract — see module docs; every external launder consumer
    /// runs DURING eval ⇒ `active >= 1`), which dropping the `Box` would dangle. At TRUE
    /// QUIESCENCE no trampoline/VM frame is on the Rust stack ⇒ no live stack launder'd
    /// ref exists; the only references into side `Box`es are this thread's `INNER_SHADOW`
    /// entries, which the driver's post-sweep `clear_inner_shadow()` discards before any
    /// next eval can deref one (the single-threaded collector thread is the sole
    /// populator of `INNER_SHADOW`). A MIDLOOP collection still reclaims the node SLOT
    /// (sound — the slot bytes stay intact; the launder'd ref is into the side `Box`, not
    /// the node) and DEFERS the side `Box` to the next quiescence sweep, where the same
    /// still-dead slot reappears in `reclaimed` (no-recycle idempotence makes the deferral
    /// correct). So midloop stays sound at the cost of the side-free's RSS win on that cycle.
    fn free_reclaimed_side_slots(&mut self, reclaimed: &[Addr]) {
        enum Side {
            Children(u32),
            Strings(u32),
            Spans(u32),
            None,
        }
        for &addr in reclaimed {
            let seg = addr.segment();
            // Extract the dead node's side index (Copy) — the `arena` borrow ends here,
            // before mutating the disjoint `sides` field. `reclaimed` holds exactly the
            // unmarked, non-released slots `sweep_range` reclaimed (its contract), so the
            // node bytes are intact and this reads the dead occupant's side index.
            let side = match self.arena.get(addr) {
                Node::SExpr(cr) | Node::Conjunction(cr) => Side::Children(cr.idx),
                Node::Atom(br) | Node::String(br) => Side::Strings(br.idx),
                Node::Spanned(_, sr) => Side::Spans(sr.idx),
                _ => Side::None, // fixed node (Bool/Long/Var/…): no side slot
            };
            if seg >= self.sides.len() {
                continue;
            }
            let s = &mut self.sides[seg];
            // Drop the dead node's `Box` (return the payload RSS); leave the index `None`
            // (NOT recycled — intern only APPENDS, so index `i` is permanently this dead
            // node's). The `i < len` guard keeps it bounds-safe AND idempotent: a
            // re-reclaimed still-free slot just `None`s an already-`None` index again.
            match side {
                Side::Children(i) => {
                    let i = i as usize;
                    if i < s.children.len() {
                        s.children[i] = None;
                    }
                }
                Side::Strings(i) => {
                    let i = i as usize;
                    if i < s.strings.len() {
                        s.strings[i] = None;
                    }
                }
                Side::Spans(i) => {
                    let i = i as usize;
                    if i < s.spans.len() {
                        s.spans[i] = None;
                    }
                }
                Side::None => {}
            }
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
        let stats = {
            let sides = &mut self.sides;
            self.arena.sweep_young_with(
                |seg| {
                    if seg < sides.len() {
                        sides[seg] = SegmentSideArenas::default();
                    }
                },
                &mut reclaimed,
            )
        };
        // C1.c #1 (Increment A): stash the reclaimed young slots; the driver frees their
        // payload `Box`es ONLY at quiescence (launder soundness — a midloop minor defers).
        self.last_reclaimed = reclaimed;
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
    /// word per interned child slice/str/span box pointer, i.e. the `Vec<Box<_>>`
    /// spine — the boxed payloads themselves are not separately tracked but the
    /// spine count tracks growth/release in lockstep with segments). This is the
    /// watermark signal for the Inc-6 single-threaded GC trigger; it does not
    /// need to be exact, only monotone-up between sweeps and to drop at sweep.
    #[inline]
    pub fn committed_bytes(&self) -> usize {
        let node_bytes = self.arena.committed_node_bytes();
        // Side-arena spine: pointer-sized entry per interned variable-length datum.
        let ptr = std::mem::size_of::<usize>();
        let mut side = 0usize;
        for s in &self.sides {
            side += (s.children.len() + s.strings.len() + s.spans.len()) * ptr;
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
}

/// The process-global index heap, mirroring the slab's `OnceLock<SlabAllocator>`.
/// Backs the future `IndexHeapStore`; `RwLock` is correct for Inc 2 (default-OFF,
/// single-threaded tests + sequential `--gc=index`), replaced by lock-free TLABs
/// in Inc 5.
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

impl MettaValueFactory<MettaValue> for IndexFactory {
    fn atom(&self, s: &str) -> MettaValue {
        let flags = flag_vars(is_variable_str(s));
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_atom(s);
        MettaValue::from_addr(addr, flags)
    }

    fn bool(&self, b: bool) -> MettaValue {
        MettaValue::inline_bool(b) // inline, mode-independent — byte-identical to slab
    }

    fn long(&self, n: i64) -> MettaValue {
        if let Some(v) = MettaValue::try_inline_long(n) {
            return v;
        }
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_fixed(Node::Long(n));
        MettaValue::from_addr(addr, 0)
    }

    fn float(&self, f: f64) -> MettaValue {
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_fixed(Node::Float(f));
        MettaValue::from_addr(addr, 0)
    }

    fn string(&self, s: &str) -> MettaValue {
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_string(s);
        MettaValue::from_addr(addr, 0)
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
            return global_index_heap()
                .write()
                .expect("index heap")
                .intern_ground_sexpr(items);
        }
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_sexpr(items);
        MettaValue::from_addr(addr, FLAG_HAS_VARIABLES)
    }

    fn error(&self, offending: MettaValue, detail: MettaValue) -> MettaValue {
        let flags = flag_vars(offending.has_variables_fast() || detail.has_variables_fast());
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_fixed(Node::Error(offending, detail));
        MettaValue::from_addr(addr, flags)
    }

    fn type_value(&self, inner: MettaValue) -> MettaValue {
        let flags = flag_vars(inner.has_variables_fast());
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_fixed(Node::Type(inner));
        MettaValue::from_addr(addr, flags)
    }

    fn conjunction(&self, goals: Vec<MettaValue>) -> MettaValue {
        self.conjunction_from_slice(&goals)
    }

    fn conjunction_from_slice(&self, goals: &[MettaValue]) -> MettaValue {
        let flags = flag_vars(goals.iter().any(|g| g.has_variables_fast()));
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_conjunction(goals);
        MettaValue::from_addr(addr, flags)
    }

    fn space(&self, handle: SpaceHandle) -> MettaValue {
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_space(handle);
        MettaValue::from_addr(addr, 0)
    }

    fn state(&self, id: u64) -> MettaValue {
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_fixed(Node::State(id));
        MettaValue::from_addr(addr, 0)
    }

    fn unit(&self) -> MettaValue {
        MettaValue::inline_unit()
    }

    fn memo(&self, handle: MemoHandle) -> MettaValue {
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_memo(handle);
        MettaValue::from_addr(addr, 0)
    }

    fn empty(&self) -> MettaValue {
        MettaValue::inline_empty()
    }

    fn not_reducible(&self) -> MettaValue {
        // Memoized NotReducible sentinel (mirrors GcFactory's interned singleton).
        // Overrides the trait default, which incorrectly returns atom("NotReducible").
        static INDEX_NOT_REDUCIBLE: OnceLock<MettaValue> = OnceLock::new();
        *INDEX_NOT_REDUCIBLE.get_or_init(|| {
            let addr = global_index_heap()
                .write()
                .expect("index heap")
                .alloc_fixed(Node::NotReducible);
            MettaValue::from_addr(addr, 0)
        })
    }

    fn quote(&self, inner: MettaValue) -> MettaValue {
        let flags = flag_vars(inner.has_variables_fast());
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_fixed(Node::Quoted(inner));
        MettaValue::from_addr(addr, flags)
    }

    fn lazy(&self, inner: MettaValue) -> MettaValue {
        // NOTE: GcFactory's `lazy` is idempotent (returns `inner` if it is already
        // Lazy). That check needs the mode-aware `is_lazy`/decode, so it is added
        // in Inc 2a-5; until then `lazy` always wraps (correct, just non-idempotent).
        let flags = flag_vars(inner.has_variables_fast());
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_fixed(Node::Lazy(inner));
        MettaValue::from_addr(addr, flags)
    }

    fn spanned(&self, value: MettaValue, span: crate::ir::Span) -> MettaValue {
        let flags = flag_vars(value.has_variables_fast());
        let addr = global_index_heap()
            .write()
            .expect("index heap")
            .alloc_spanned(value, span);
        MettaValue::from_addr(addr, flags)
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
// satisfied by the CALLER, which passes the live trampoline S/C/K + frame chain
// + caches (the trampoline's own `RootSet`) UNIONED with `collect_all_roots()`
// (env/tiers/output/dispatch RootProviders). See
// docs/cesk-gc/single-threaded-collector.md STEP 0.
// ============================================================================
pub mod index_gc {
    use super::global_index_heap;
    use crate::backend::models::metta_value::{clear_inner_shadow, gc_mode_is_index};
    use crate::backend::models::{active_evaluator_count, worker_ever_spawned, MettaValue};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

    /// C1.c: the nursery budget — a MINOR fires when `young_alloc_bytes` (bytes of
    /// young node-slab allocated since the last promotion) exceeds this. A principled
    /// CONSTANT (NOT an on/off switch — minors are ALWAYS on; this only sizes the
    /// nursery), ≈ 1/4 of a segment's node-slab capacity (a segment is
    /// `1<<18` slots ≈ 8 MiB). Rationale: (a) < `DEFAULT_MIN_THRESHOLD` (the major
    /// floor) so a minor fires BEFORE a major on multi-segment workloads
    /// (minor-primary); (b) < one segment so a minor's young generation stays within
    /// the active bump segment ⇒ its reclaimed slots stay young and are reused (no
    /// promotion-stranding); (c) tied to the substrate's segment granularity, not a
    /// magic number. Minors thus fire NATURALLY on any workload allocating more than
    /// ~1/4 segment of transient young between safepoints — exercised by tests with
    /// no force-switch.
    const YOUNG_BUDGET: usize = 2 * 1024 * 1024;

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
        let b = YOUNG_BUDGET;
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
        young_alloc > YOUNG_BUDGET
            || nursery_pending
            || old_live > WATERMARK.load(Ordering::Relaxed).max(min_threshold())
            || committed > max_bytes()
            || MINORS_SINCE_MAJOR.load(Ordering::Relaxed) >= MAJOR_CADENCE
    }

    /// The provable single-threaded-quiescence gate:
    ///
    /// ```text
    /// gc_mode_is_index() && !worker_ever_spawned() && active_evaluator_count() == 0
    /// ```
    ///
    /// The collector runs at TRUE quiescence — the point in `eval()` AFTER the
    /// `EvalGuard` has dropped, so `active_evaluator_count() == 0`: no trampoline
    /// loop and no bytecode VM is live on the Rust stack, so the only surviving
    /// values are the environment / promoted roots (`collect_all_roots()`) plus
    /// the about-to-be-returned result vector (which the caller passes in
    /// explicitly). This is exactly the slab GC's session-release reclaim point
    /// and exactly the proven `QuiescenceInvariant` (activeEvaluators empty).
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
    /// `!worker_ever_spawned()` keeps the single-threaded guarantee: if any eval
    /// worker has ever been spawned, a parked-resumable worker could exist, so
    /// the collector backs off entirely.
    #[inline]
    pub fn gate_open() -> bool {
        gc_mode_is_index() && !worker_ever_spawned() && active_evaluator_count() == 0 && !disabled()
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
    /// `!worker_ever_spawned()` this is still the trivially-true instance of the
    /// proven `QuiescenceInvariant`: no eval worker has ever been spawned, so no
    /// parked-resumable worker can exist, and the single calling thread at the
    /// safepoint is the SOLE thread that can touch σ. The collection runs under
    /// the heap write lock, mutually exclusive with allocation, so no `Addr` is
    /// minted mid-mark.
    ///
    /// **Root completeness (the UAF linchpin):** unlike the quiescence point,
    /// the live trampoline S/C/K AND every on-stack bytecode-VM frame's
    /// execution stacks ARE alive here — so the caller MUST pass the COMPLETE
    /// mid-execution root set: the trampoline's own `RootSet` (work items +
    /// continuations + frame chain + pointer-keyed caches + deferred envs)
    /// UNIONED with `collect_all_roots()`. The frame chain now carries every
    /// nested VM frame's `collect_roots_into` output (see
    /// `bytecode/vm/mod.rs::with_vm_roots_frame`), which is what makes the
    /// mid-loop set complete and the collection ASAN-clean. Marking from an
    /// incomplete set is a use-after-free.
    #[inline]
    pub fn gate_open_midloop() -> bool {
        // Mid-loop (mid-directive) collection is OPT-IN (`midloop_enabled()`)
        // until its ASAN root-completeness proof lands — see `midloop_enabled`.
        // Default-OFF means the shipped index-gc behavior is exactly the
        // ASAN-validated true-quiescence collector (Inc 6a); mid-loop fires only
        // when explicitly enabled for validation / once proven.
        gc_mode_is_index()
            && midloop_enabled()
            && !worker_ever_spawned()
            && active_evaluator_count() == 1
            && !disabled()
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
        young_alloc > YOUNG_BUDGET
            || nursery_pending
            || old_live > WATERMARK.load(Ordering::Relaxed).max(min_threshold())
            || committed > max_bytes()
            || MINORS_SINCE_MAJOR.load(Ordering::Relaxed) >= MAJOR_CADENCE
    }

    /// `METTATRON_INDEX_GC_DISABLE=1` forces the collector off (parsed once).
    /// Used by the RSS-reclamation validation to measure the no-collection
    /// baseline; default is enabled.
    fn disabled() -> bool {
        use std::sync::OnceLock;
        static OFF: OnceLock<bool> = OnceLock::new();
        *OFF.get_or_init(|| std::env::var("METTATRON_INDEX_GC_DISABLE").as_deref() == Ok("1"))
    }

    /// Mid-loop (mid-directive) collection is OPT-IN until its ASAN
    /// root-completeness proof lands: `METTATRON_INDEX_GC_MIDLOOP=1` enables it.
    /// Default OFF so the shipped behavior is the ASAN-validated true-quiescence
    /// collector (Inc 6a). The mid-loop path roots the live trampoline S/C/K plus
    /// every on-stack VM frame (`with_vm_roots_frame`); enabling it by default
    /// awaits the ASAN run that forces a mid-execution sweep and proves no
    /// freed-mid-execution use-after-free (parsed once).
    fn midloop_enabled() -> bool {
        use std::sync::OnceLock;
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var("METTATRON_INDEX_GC_MIDLOOP").as_deref() == Ok("1"))
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
        // Gate first — cheap, and keeps the default/slab build's call site dead
        // (gc_mode_is_index() const-folds to false when the feature is off).
        if !gate_open() {
            return false;
        }
        mark_sweep_if_over_watermark(roots, "quiescence")
    }

    /// Run a MID-LOOP (mid-directive) mark+sweep cycle IF the mid-loop safety
    /// gate ([`gate_open_midloop`]) is open AND the committed-bytes watermark is
    /// exceeded.
    ///
    /// `roots` MUST be the COMPLETE mid-execution root set — the live trampoline
    /// S/C/K (work items + continuations + frame chain, including every nested
    /// bytecode-VM frame's execution stacks via `with_vm_roots_frame`) and
    /// pointer-keyed caches and deferred envs, UNIONED with `collect_all_roots()`
    /// (env / tiers / promoted). Unlike the quiescence variant, live execution
    /// stacks ARE present here, so an incomplete set is a use-after-free; this is
    /// validated directly under ASAN (a missed root → freed-mid-execution →
    /// heap-use-after-free). Returns `true` iff a cycle ran.
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
        let (committed, young_alloc, old_live, nursery_pending) = {
            let heap = global_index_heap().read().expect("index heap");
            (
                heap.committed_bytes(),
                heap.young_alloc_bytes(),
                heap.old_live_bytes(),
                heap.nursery_full_pending(),
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
        let major_due = live_major || cap_major || cadence_major;
        // Increment C (CHANGE #2): the MINOR is `young_alloc` past the budget OR the
        // backpressure signal (`nursery_pending` — a segment opened since the last promotion).
        let minor_due = young_alloc > YOUNG_BUDGET || nursery_pending;
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
        let do_major =
            major_due && !(level == 3 && minor_due && !cap_major && !cadence_major);

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
        //   MINOR: the cheap YOUNG-ONLY `mark_young` (O(young reachable)) + young
        //          `sweep_young` + promote. SOUND because `alloc` reuses young slots
        //          only ⇒ no old→young σ edge ⇒ no live young node is reachable only
        //          through an old node (Phase C1.c §A; the young-only mark theorem).
        // `promote_young` reclassifies the swept young segments as old (so only the
        // active + future segments stay young) AND resets the young-alloc odometer.
        let (live_after, old_live_after, stats, did_major) = {
            let mut heap = global_index_heap().write().expect("index heap");
            let (stats, did_major) = if do_major {
                heap.mark(&addrs); // FULL mark
                (heap.sweep(), true)
            } else {
                // Minor: a pure minor (only minor_due) OR a level-3-DEFERRED major (a minor
                // ran instead this cycle; the live_major re-fires next cycle / within cadence).
                heap.mark_young(&addrs); // YOUNG-ONLY mark (cheap — the minor's win)
                (heap.sweep_young(), false)
            };
            // Increment A (the RSS half of CHANGE #1): free the swept-dead nodes' payload
            // `Box`es — ONLY at quiescence. `phase == "quiescence"` ⇒ `gate_open()` ⇒
            // `active_evaluator_count() == 0` ⇒ no trampoline/VM frame on the Rust stack ⇒
            // no live launder'd `&'static` into a side `Box`; the only refs into side
            // `Box`es are this thread's `INNER_SHADOW` entries, dropped by
            // `clear_inner_shadow()` below before any next eval can deref one. A MIDLOOP
            // collection reclaimed the node slots but DEFERS the side `Box`es to the next
            // quiescence sweep (the still-dead slots reappear in `reclaimed`; no-recycle
            // idempotence makes the deferral correct).
            if phase == "quiescence" {
                let reclaimed = std::mem::take(&mut heap.last_reclaimed);
                heap.free_reclaimed_side_slots(&reclaimed);
            }
            heap.promote_young();
            // B.4: measure old_live AFTER promote (the just-swept survivors are now old),
            // so the rearm metric == the trigger metric (both old_live) ⇒ geometric, no thrash.
            (heap.live_bytes(), heap.old_live_bytes(), stats, did_major)
        };

        // Drop this thread's stale `MettaValueInner` materialization cache: a swept
        // (young or whole-heap) segment's `Addr`s are now invalid/reusable, so any
        // cached inner keyed by such an Addr must go — required after BOTH a minor
        // (a reused young Addr) and a major. This thread is the only one with a
        // populated INNER_SHADOW in the single-threaded regime.
        // C1.c #1: when this collection reused an `Addr` for new content, every
        // Addr-keyed / content-hash-keyed cache that could still hold the PRIOR
        // occupant's entry must be invalidated — else a later lookup (set-op hashing,
        // eval memo, match, operator, MORK) serves a stale result. Invalidate EXACTLY
        // the set the SLAB collector invalidates at its safepoints, via the proven,
        // already-wired `clear_aba_sensitive_caches()` — restoring slab parity:
        //   VALUE_HASH_CACHE (Addr-keyed in index mode — the PRIMARY cause of the
        //   set-op divergences), the MORK bytes + ground-fragment caches, and the
        //   operator cache. (The slab bumps `gc_sweep_epoch`, which VALUE_HASH_CACHE /
        //   MORK self-read to lazily self-clear; the index collector cannot call the
        //   `pub(super)` epoch bump, so it uses the same public clear path the slab
        //   path uses — minimal surface, exact parity.)
        crate::backend::eval::trampoline::eval_loop::clear_aba_sensitive_caches();
        // Index-only: the laundered-`MettaValueInner` shadow keyed by `Addr` (a reused
        // Addr invalidates any cached inner). Not part of the slab ABA set.
        clear_inner_shadow();
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
            MINORS_SINCE_MAJOR.fetch_add(1, Ordering::Relaxed);
        }

        GC_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
        if phase == "midloop" {
            MIDLOOP_CYCLES_RUN.fetch_add(1, Ordering::Relaxed);
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
// protocol EXACTLY (see `index_arena.rs`): a once-allocated chunk directory
// (`chunks`) whose cells are published with `Release`/`Acquire`, a `bump` CLAIM
// cursor (`Relaxed` `fetch_add` — uniqueness is all `fetch_add` needs), and a
// `len` PUBLISH cursor (`Release`-CAS, `Acquire`-load) keeping `[0, len)` a
// contiguous written prefix. This lets entries be appended through a shared
// `&self` (the B2 `&self` allocation path) without an `&mut` or a lock on the
// hot path. INERT in D-TLAB-1.0: declared and unit-tested, but no other code
// references it yet (D-TLAB-1.1 swaps `SegmentSideArenas` over to it).

/// Low bits of a side-column index used for the intra-chunk offset.
/// 12 bits ⇒ 4096 entries per chunk (the unit the directory grows by).
const SIDE_CHUNK_BITS: u32 = 12;
/// Entries per chunk (`1 << SIDE_CHUNK_BITS`).
const SIDE_CHUNK_LEN: usize = 1 << SIDE_CHUNK_BITS;
/// Mask selecting the intra-chunk offset from an index.
const SIDE_CHUNK_MASK: usize = SIDE_CHUNK_LEN - 1;
/// Maximum chunks a single column directory holds, allocated once in `new`.
///
/// Worst case: every node slot in one segment is a variable-length allocation
/// contributing exactly one side entry, and entries are NEVER recycled, so a
/// column for a single segment must hold at least
/// [`DEFAULT_SEGMENT_CAPACITY`](crate::backend::eval::cesk::index_arena::DEFAULT_SEGMENT_CAPACITY)
/// entries. `ceil(DEFAULT_SEGMENT_CAPACITY / SIDE_CHUNK_LEN)` chunks cover that,
/// plus a small `+ 2` margin. With `DEFAULT_SEGMENT_CAPACITY = 262_144` and
/// `SIDE_CHUNK_LEN = 4096` this is `262_144 / 4096 + 2 = 64 + 2 = 66`.
const MAX_SIDE_CHUNKS: usize = {
    use crate::backend::eval::cesk::index_arena::DEFAULT_SEGMENT_CAPACITY;
    (DEFAULT_SEGMENT_CAPACITY + SIDE_CHUNK_LEN - 1) / SIDE_CHUNK_LEN + 2
};

/// One side-column chunk: `SIDE_CHUNK_LEN` cells, each an
/// `UnsafeCell<MaybeUninit<Option<Box<T>>>>`. Every cell is initialized to
/// `MaybeUninit::new(None)` at chunk creation (`grow_to`) — NOT `uninit()` —
/// so `assume_init_ref` on ANY in-bounds offset is always sound, even before a
/// `push` writes it. (This diverges from the node arena, which gates reads on
/// `len`; for the `Option<Box<T>>` column initializing to `None` is the safe
/// and robust choice and costs nothing — the niche keeps `Option<Box<_>>` the
/// same size as the bare `Box`.)
type SideChunk<T> = Box<[std::cell::UnsafeCell<std::mem::MaybeUninit<Option<Box<T>>>>]>;

/// A per-(segment, field) never-realloc chunked column of address-stable
/// `Box<T>` payloads, appendable under `&self` (lock-free bump+publish) — the
/// concurrent replacement for the `Vec<Option<Box<T>>>` side arenas.
///
/// Mirrors [`IndexArena`](crate::backend::eval::cesk::index_arena)'s directory
/// + [`Segment`]'s two-cursor: `chunks` is a once-allocated directory of
/// `MAX_SIDE_CHUNKS` cells (a cell's chunk `Box` is written exactly once, under
/// `grow_lock`, before `chunk_count` advances past it); `bump` claims a unique
/// index (`Relaxed`); `len` publishes the contiguous written prefix
/// (`Release`-CAS). Inert in D-TLAB-1.0 (added but not yet referenced).
struct SideColumn<T: ?Sized> {
    /// Never-realloc directory: `MAX_SIDE_CHUNKS` cells, allocated once in
    /// `new`. Cell `c` is initialized (its chunk `Box` written) exactly once,
    /// under `grow_lock`, before `chunk_count` is advanced past `c` with a
    /// `Release` store. A reader dereferences cell `c` only for
    /// `c < chunk_count.load(Acquire)` (the `unsafe fn chunk` precondition).
    chunks: Box<[std::cell::UnsafeCell<std::mem::MaybeUninit<SideChunk<T>>>]>,
    /// Published chunk count (count of initialized directory cells). Monotone;
    /// advanced with `Release` under `grow_lock`, read with `Acquire`.
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
}

impl<T: ?Sized> SideColumn<T> {
    /// A new, empty column. Allocates the `MAX_SIDE_CHUNKS`-cell directory once
    /// (each cell an uninitialized `MaybeUninit<SideChunk>`); does NOT
    /// pre-allocate any chunk — chunk 0 is created lazily by the first `push`
    /// via `grow_to`. (`IndexArena::new` eagerly opens segment 0; this column
    /// stays fully lazy because a column may never be pushed to — keeping
    /// per-column overhead to just the `MAX_SIDE_CHUNKS` pointer slots, here
    /// `66 * 8 = 528` bytes.)
    fn new() -> Self {
        use std::cell::UnsafeCell;
        use std::mem::MaybeUninit;
        use std::sync::atomic::AtomicUsize;
        // `UnsafeCell`/`MaybeUninit` are not `Clone`, so the directory is built
        // from an iterator (one uninit cell per addressable chunk) rather than
        // `vec![..; MAX_SIDE_CHUNKS]`. Allocated once, never reallocated.
        let chunks: Box<[UnsafeCell<MaybeUninit<SideChunk<T>>>]> = (0..MAX_SIDE_CHUNKS)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        SideColumn {
            chunks,
            chunk_count: AtomicUsize::new(0),
            bump: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
            grow_lock: std::sync::Mutex::new(()),
        }
    }

    /// Decompose an index into `(chunk, offset)`.
    #[inline]
    fn locate(idx: usize) -> (usize, usize) {
        (idx >> SIDE_CHUNK_BITS, idx & SIDE_CHUNK_MASK)
    }

    /// Borrow chunk `c` of the published directory.
    ///
    /// # Safety
    /// The caller must guarantee `c < self.chunk_count.load(Acquire)` as
    /// observed by the calling thread — i.e. cell `c` has been published by
    /// `grow_to`'s `Release` store to `chunk_count`, which happens-after the
    /// cell's initialization. Under that premise the cell is initialized.
    ///
    /// The returned slice derives from the raw pointer `UnsafeCell::get()`
    /// yields (NOT from a borrow of `&self.chunks`), so it does not borrow
    /// `self` — mirroring `IndexArena::segment`.
    #[inline]
    unsafe fn chunk(&self, c: usize) -> &[std::cell::UnsafeCell<std::mem::MaybeUninit<Option<Box<T>>>>] {
        let cell = self.chunks[c].get(); // *mut MaybeUninit<SideChunk<T>>
        (*cell).assume_init_ref() // &SideChunk<T> -> &[..] via Deref
    }

    /// Grow the directory so chunk `c` is published. Mirrors
    /// `IndexArena::open_segment`: takes `grow_lock`, re-checks under the lock,
    /// allocates and writes any missing chunk cell, then publishes it with a
    /// `Release` store to `chunk_count` (paired with the `Acquire` load in
    /// `chunk`/`push`).
    ///
    /// Loops `chunk_count_now..=c` to fill ALL gaps up to `c`, not just `c`
    /// itself: although a single-segment monotone `bump` advances `c` by one at
    /// a time, a future relaxation (a claim landing far ahead) could skip a
    /// chunk; filling the gap keeps the directory dense and every published
    /// chunk initialized.
    fn grow_to(&self, c: usize) {
        use std::cell::UnsafeCell;
        use std::mem::MaybeUninit;
        use std::sync::atomic::Ordering;
        let _guard = self.grow_lock.lock().expect("side-column grow_lock poisoned");
        let mut next = self.chunk_count.load(Ordering::Acquire);
        if c < next {
            return; // another thread already grew past `c` while we waited
        }
        assert!(
            c < MAX_SIDE_CHUNKS,
            "side column exhausted: {MAX_SIDE_CHUNKS} chunks"
        );
        // Fill every gap chunk_count..=c. Each entry is initialized to `None`
        // (NOT uninit) so `assume_init_ref` is sound on any in-bounds offset
        // before a `push` writes it (see the `SideChunk` type docs).
        while next <= c {
            let chunk: SideChunk<T> = (0..SIDE_CHUNK_LEN)
                .map(|_| UnsafeCell::new(MaybeUninit::new(None)))
                .collect();
            // SAFETY: cell `next` is not yet published (`next == chunk_count`),
            // so no reader can observe it; under `grow_lock` we are the unique
            // writer of this cell. Initialize it before publishing `next`.
            unsafe {
                (*self.chunks[next].get()).write(chunk);
            }
            self.chunk_count.store(next + 1, Ordering::Release); // publish cell
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

    /// Append `boxed`, returning its stable index. Lock-free on the steady-state
    /// fast path (the `grow_to` lock is taken only when crossing into a not-yet-
    /// published chunk, once per `SIDE_CHUNK_LEN` pushes).
    ///
    /// Protocol (mirrors `IndexArena::alloc_bump` → `Segment::{bump_one,
    /// write_claimed, publish}`): CLAIM a unique `idx` (`Relaxed` `fetch_add`);
    /// ensure the target chunk is published (`grow_to` under `Acquire` re-check);
    /// WRITE the (uniquely claimed, exclusive, unpublished) cell; PUBLISH `idx`.
    fn push(&self, boxed: Box<T>) -> u32 {
        use std::sync::atomic::Ordering;
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
        // The write is therefore exclusive and races no reader.
        unsafe {
            let chunk = self.chunk(c);
            (*chunk[off].get()).write(Some(boxed));
        }
        self.publish(idx);
        idx as u32
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
        (*chunk[off].get()).assume_init_ref().as_deref()
    }

    /// Drop the payload `Box` at `idx`, leaving the cell as `None`. `&mut self`
    /// (quiescence-only): a free runs only at a quiescent safepoint, statically
    /// exclusive of every `&self` reader/pusher, so it races nothing (the index
    /// analogue of the arena's free-list-reuse-at-quiescence rule). Never
    /// decrements `bump`/`len` and never recycles the index — a later `push`
    /// APPENDS a fresh index (keeping the side-free idempotent: re-freeing an
    /// already-`None` cell is a no-op). `Relaxed` load of `len` is fine under
    /// `&mut self` (no concurrent writer).
    fn free(&mut self, idx: u32) {
        use std::sync::atomic::Ordering;
        if (idx as usize) < self.len.load(Ordering::Relaxed) {
            let (c, off) = Self::locate(idx as usize);
            // SAFETY: `idx < len` ⇒ `c < chunk_count` (published) and the cell is
            // initialized (every cell is `None` from `grow_to`, then possibly a
            // `Some` from `push`). `&mut self` is exclusive, so no aliasing.
            unsafe {
                let chunk = self.chunk(c);
                *(*chunk[off].get()).assume_init_mut() = None; // drops the Box
            }
        }
    }

    /// The number of published entries (`len`).
    #[inline]
    fn published_len(&self) -> usize {
        use std::sync::atomic::Ordering;
        self.len.load(Ordering::Acquire)
    }

    /// The number of published chunks (diagnostics / tests).
    #[inline]
    fn chunk_count(&self) -> usize {
        use std::sync::atomic::Ordering;
        self.chunk_count.load(Ordering::Acquire)
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
//   (iii) DIRECTORY PUBLICATION. A reader dereferences directory cell `c` only
//         for `c < chunk_count.load(Acquire)`. `grow_to` initializes cell `c`
//         then does `chunk_count.store(c+1, Release)` (under `grow_lock`, the
//         unique writer of cell `c`), so observing `c < chunk_count` happens-
//         after the cell's initialization ⇒ the chunk `Box` ptr (and its cells,
//         all `None` from `grow_to`) are visible before the reader indexes it.
//
//   (iv)  FREE AT QUIESCENCE. The only in-place rewrite of an already-published
//         entry is `free`, which is `&mut self` and runs only at a quiescent
//         safepoint. `&mut self` is statically exclusive of every `&self`
//         reader/pusher, so a free never races a concurrent access (and the
//         index is never recycled ⇒ no ABA).
//
// Hence no data race on any field. `Send` requires `T: Send` (the column owns
// `Box<T>` payloads it may hand to another thread); `Sync` requires `T: Send +
// Sync` (a shared `&SideColumn` lets multiple threads obtain `&T`) — the same
// conservative bounds `IndexArena` uses. `T: ?Sized` is supported (the columns
// hold `[MettaValue]`/`str`/`Span` via `Box<[_]>`/`Box<str>`/`Box<Span>`).
unsafe impl<T: ?Sized + Send> Send for SideColumn<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for SideColumn<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::metta_value::{reset_gc_mode_slab, set_gc_mode_index};

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
        set_gc_mode_index();
        let mut heap = IndexHeap::with_segment_capacity(64);
        // Two leaf atoms; reference them from a SExpr as index handles.
        let l1 = heap.alloc_atom("a");
        let l2 = heap.alloc_atom("b");
        let h1 = MettaValue::from_addr(l1, 0);
        let h2 = MettaValue::from_addr(l2, 0);
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
        reset_gc_mode_slab();
    }

    #[test]
    fn segment_release_co_releases_side_arenas() {
        // Fill segment 0 with variable-length (string) nodes, keep a live node in
        // segment 1, then sweep: segment 0 dies and its byte side-arena is reset.
        set_gc_mode_index();
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
        reset_gc_mode_slab();
    }

    #[test]
    fn committed_bytes_drop_after_segment_release() {
        // Inc 6: the watermark signal must DROP when a fully-dead segment is
        // released, else the trigger never re-arms. Fill segment 0 with dead
        // strings, keep a live node in segment 1, sweep, and assert committed
        // bytes fell.
        set_gc_mode_index();
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
        reset_gc_mode_slab();
    }

    #[test]
    fn gate_closes_after_worker_spawned() {
        // The single-threaded safety gate must latch shut the instant any eval
        // worker is noted as spawned. (Process-global flag; nextest isolates this
        // test in its own process so the latch does not leak.)
        set_gc_mode_index();
        // No worker yet and a single (this) caller → gate may open. We don't
        // assert it's open here (active_evaluator_count depends on guards), but we
        // DO assert that after note_worker_spawned() it is definitively closed.
        crate::backend::models::note_worker_spawned();
        assert!(
            !index_gc::gate_open(),
            "gate must be closed once a worker has ever been spawned"
        );
        reset_gc_mode_slab();
    }

    #[test]
    fn global_index_heap_initializes() {
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
        set_gc_mode_index();
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

        reset_gc_mode_slab();
    }

    #[test]
    fn metta_value_view_is_mode_aware() {
        // The CRUX (Inc 2a-5 Step 1): `MettaValue::view()` decodes an index handle
        // by reading its `Node` from the global heap (Spanned-stripped,
        // `'static`-laundered), instead of dereferencing a slab pointer.
        use crate::backend::models::metta_value::ValueView;
        use crate::backend::models::MettaValueFactory;
        use crate::ir::{Position, Span};
        set_gc_mode_index();
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

        reset_gc_mode_slab();
    }

    #[test]
    fn decode_api_is_mode_aware_via_materialization() {
        // CRUX Step 2c+3a: the typed accessors, inner_ref/inner/inner_raw, and
        // inner_ptr all work in Index mode without per-accessor edits — they
        // funnel through the now-mode-aware inner_ref() (Addr-keyed materialization).
        use crate::backend::models::metta_value::{
            clear_inner_shadow, inner_shadow_len, MettaValueInner,
        };
        use crate::backend::models::metta_value_trait::MettaValueTrait; // hash_value
        use crate::backend::models::MettaValueFactory;
        use crate::ir::{Position, Span};
        set_gc_mode_index();
        clear_inner_shadow();
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
        // Addr-keyed cache → a stable inner_ref pointer for the same handle.
        let p1 = a.inner_ref() as *const MettaValueInner;
        let p2 = a.inner_ref() as *const MettaValueInner;
        assert_eq!(
            p1, p2,
            "inner_ref is a stable per-Addr pointer (Addr-keyed cache)"
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

        // ── shadow cache is bounded + clearable ──────────────────────────────
        assert!(
            inner_shadow_len() > 0,
            "materialization populated the shadow cache"
        );
        clear_inner_shadow();
        assert_eq!(
            inner_shadow_len(),
            0,
            "clear_inner_shadow empties the cache"
        );

        reset_gc_mode_slab();
    }

    #[test]
    fn index_factory_hash_conses_ground_sexpr() {
        // CRUX Step 4: ground SExprs are hash-consed exactly like GcFactory —
        // keyed by child HANDLE identity (tagged bits), ground-only. Equal child
        // handles ⇒ one Addr ⇒ one inner_ptr key (the R9 fixpoint-identity parity).
        use crate::backend::models::MettaValueFactory;
        set_gc_mode_index();
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

        reset_gc_mode_slab();
    }

    #[test]
    fn sweep_retains_live_hash_cons_drops_dead() {
        // B1.c: sweep keeps the hash-cons entry for LIVE (marked) ground content
        // and drops the entry for dead content — so re-interning live content HITS
        // the same canonical Addr, while dead content is forgotten and re-allocated
        // fresh (never a dangling hit on a reclaimed-but-intact slot).
        use crate::backend::models::MettaValueFactory;
        set_gc_mode_index();
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

        reset_gc_mode_slab();
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
        set_gc_mode_index();
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
        reset_gc_mode_slab();
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
            idxs.push(col.push(boxed));
        }
        // Indices are claimed monotonically from 0 (no recycling, no gaps here).
        assert_eq!(idxs, (0u32..10).collect::<Vec<_>>(), "monotone indices 0..10");
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
            let idx = col.push(boxed);
            assert_eq!(idx as usize, i, "index tracks push order");
        }
        assert_eq!(col.published_len(), n, "all entries published");
        assert!(
            col.chunk_count() >= 2,
            "crossed a chunk boundary ⇒ >= 2 chunks (got {})",
            col.chunk_count()
        );
        // Read back a sample spanning both chunks (including the boundary).
        for &i in &[0usize, 1, SIDE_CHUNK_LEN - 1, SIDE_CHUNK_LEN, SIDE_CHUNK_LEN + 4] {
            // SAFETY: i < published_len() == n.
            let got = unsafe { col.get(i as u32) }.expect("entry present");
            assert_eq!(got, &[i as i32][..], "entry {i} survives the grow");
        }
    }

    #[test]
    fn side_column_free_drops_only_target() {
        // free(idx) ⇒ get returns None there; a neighbor is unaffected.
        let mut col: SideColumn<str> = SideColumn::new();
        let a = col.push(Box::from("alpha"));
        let b = col.push(Box::from("beta"));
        let c = col.push(Box::from("gamma"));
        // SAFETY: all indices < published_len() (just pushed).
        assert_eq!(unsafe { col.get(b) }, Some("beta"));
        col.free(b);
        assert_eq!(unsafe { col.get(b) }, None, "freed entry reads None");
        // Neighbors unaffected.
        assert_eq!(unsafe { col.get(a) }, Some("alpha"), "left neighbor intact");
        assert_eq!(unsafe { col.get(c) }, Some("gamma"), "right neighbor intact");
        // Idempotent re-free is a no-op (still None, no double-drop).
        col.free(b);
        assert_eq!(unsafe { col.get(b) }, None, "re-free is idempotent");
    }

    #[test]
    fn side_column_no_recycle_after_free() {
        // After freeing an index, the next push gets a NEW (higher) index — the
        // freed slot is never reused (no-recycle invariant).
        let mut col: SideColumn<[i32]> = SideColumn::new();
        let i0 = col.push(vec![0].into_boxed_slice());
        let i1 = col.push(vec![1].into_boxed_slice());
        col.free(i0);
        let i2 = col.push(vec![2].into_boxed_slice());
        assert_eq!(i2, 2, "push after free APPENDS a fresh index (no recycle)");
        assert_ne!(i2, i0, "freed index {i0} is not reused");
        // The freed slot stays freed; the new entry is the appended one.
        // SAFETY: indices < published_len() (== 3).
        assert_eq!(unsafe { col.get(i0) }, None, "freed slot still None");
        assert_eq!(unsafe { col.get(i1) }, Some(&[1][..]), "i1 intact");
        assert_eq!(unsafe { col.get(i2) }, Some(&[2][..]), "new entry present");
        assert_eq!(col.published_len(), 3, "len counts every push, freed or not");
    }
}
