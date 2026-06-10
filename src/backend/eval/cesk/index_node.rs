//! Arena value node — Increment 2 of the clean-room GC migration.
//!
//! [`Node`] is a fixed-size, `Copy`, `Drop`-free mirror of
//! [`MettaValueInner`](crate::backend::models::MettaValueInner)'s 18 variants,
//! stored in an [`IndexArena`](crate::backend::eval::cesk::index_arena::IndexArena).
//! It is the value representation the non-moving collector reclaims.
//!
//! Representation rules (so `Node` stays small and `Copy`):
//! - **Children** are `MettaValue` handles. In index mode a heap child's payload
//!   is an [`Addr`]; [`MettaValue::as_arena_addr`] decodes it. Inline scalars
//!   (`Bool`/i48 `Long`/`Unit`/`Empty`) are carried by value in the handle and
//!   yield no `Addr`.
//! - **Variable-length data** (SExpr/Conjunction children, Atom/String bytes,
//!   Spanned spans) lives in **per-segment side-arenas** owned by `IndexHeap`
//!   (Inc 2a-3); `Node` holds only a segment-relative index
//!   ([`ChildRef`]/[`ByteRef`]/[`SpanRef`]) into a per-segment `Vec<Box<_>>`
//!   whose `Box` pointees are address-stable for the segment's life. A bare
//!   `Node` therefore cannot reach SExpr/Conjunction children — those edges are
//!   enumerated by `IndexHeap` via [`IndexArena::mark_from_roots_with`].
//! - **Non-`Copy` handles** (`SpaceHandle`/`MemoHandle`) are represented by their
//!   `u64` id; the live `Arc`-backed handle is kept in an `IndexHeap` side table
//!   keyed by that id (matching how `view`/eq/hash already use only the id).
//!
//! This node representation is wired into the active `index-gc` value model via
//! `IndexHeap` and the index factory. The module keeps `#![allow(dead_code)]`
//! because some total-mirror variants and verification helpers are intentionally
//! present across slab/index build modes.

#![allow(dead_code)]

use crate::backend::eval::cesk::index_arena::{Addr, ArenaNode};
use crate::backend::models::MettaValue;

/// Index into a segment's child side-arena (`Vec<Box<[MettaValue]>>`): the
/// children of a `SExpr`/`Conjunction` are the `idx`-th boxed slice. Segment-
/// relative — the owning node's [`Addr`] names the segment — and co-released with
/// it. A *boxed* slice (not a shared growable `Vec`) so its address is stable for
/// the segment's life: the per-segment `Vec<Box<_>>` may reallocate as more
/// values are interned, but the `Box` pointees never move, so a `&[MettaValue]`
/// laundered from one stays valid until the segment is released (avoids the
/// reallocation use-after-free a shared `Vec<MettaValue>` would suffer, since a
/// single SExpr can hold unbounded children).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildRef {
    pub idx: u32,
    /// Per-cell generation stamped by `SideColumn::push` when this `idx` was
    /// claimed. Side-column indices are RECYCLED (post-266d19d free-list reuse),
    /// so an index alone no longer identifies a payload across reuse; the
    /// generation does. A deferred `SideReclaim` snapshot captures THIS value and
    /// `SideColumn::free` drops the cell only when its current generation still
    /// equals the captured one — so a stale snapshot whose `idx` was reused by a
    /// live re-intern (which bumped the cell's generation) never frees the live
    /// payload. See [`SideColumn`](crate::backend::eval::cesk::index_heap).
    ///
    /// `u32` (not `u64`) is sufficient and deliberate: a stale snapshot could only
    /// ALIAS a reused cell after `2^32` reuses of THIS one cell BETWEEN the
    /// snapshot's capture and its drain — but a snapshot is drained at the very next
    /// true-quiescence collection (`pending_side_major` forces a major the moment a
    /// quiescence point is reached with pending reclaims > 0), i.e. within a handful
    /// of reuses, so the wrap is unreachable by a factor of ~`2^32`. Widening to
    /// `u64` would grow each `{idx, gen}` ref 8→16 bytes (still under the `Node`
    /// 32-byte budget) for zero benefit. The injective-generation abstraction is
    /// proven in `formal/rocq/gc/QuiescentSideIndexReuse.v` (gen_injective) +
    /// model-checked in `tla/SideReclaimGeneration.tla`.
    pub gen: u32,
}

/// Index into a segment's byte side-arena (`Vec<Box<str>>`) for `Atom`/`String`
/// (boxed str → stable address, same rationale as [`ChildRef`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRef {
    pub idx: u32,
    /// Per-cell generation; see [`ChildRef::gen`].
    pub gen: u32,
}

/// Index into a segment's span side-arena (`Vec<Box<Span>>`) for `Spanned`
/// (boxed → stable address; keeps the 48-byte `Span` out of `Node`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanRef {
    pub idx: u32,
    /// Per-cell generation; see [`ChildRef::gen`].
    pub gen: u32,
}

/// A fixed-size, `Copy` arena node mirroring all 18 `MettaValueInner` variants.
///
/// `Bool`/`Unit`/`Empty` and i48-range `Long` are normally carried inline in a
/// `MettaValue` and never allocated as a `Node`; the variants exist so `Node` is
/// a faithful, total mirror (the factory decides inline vs. node).
#[derive(Clone, Copy)]
pub enum Node {
    Atom(ByteRef),
    Bool(bool),
    Long(i64),
    Float(f64),
    String(ByteRef),
    SExpr(ChildRef),
    Error(MettaValue, MettaValue),
    Type(MettaValue),
    Conjunction(ChildRef),
    Space(u64),
    State(u64),
    Unit,
    Memo(u64),
    Quoted(MettaValue),
    Lazy(MettaValue),
    Empty,
    NotReducible,
    Spanned(MettaValue, SpanRef),
}

impl ArenaNode for Node {
    #[inline]
    fn child_addrs(&self, out: &mut Vec<Addr>) {
        #[inline]
        fn push(h: MettaValue, out: &mut Vec<Addr>) {
            if let Some(a) = h.as_arena_addr() {
                out.push(a);
            }
        }
        match *self {
            Node::Error(a, b) => {
                push(a, out);
                push(b, out);
            }
            Node::Type(a) | Node::Quoted(a) | Node::Lazy(a) => push(a, out),
            Node::Spanned(a, _) => push(a, out),
            // SExpr/Conjunction children live in the child side-arena; a bare
            // Node cannot reach them — IndexHeap enumerates those edges via
            // IndexArena::mark_from_roots_with (see module docs).
            Node::SExpr(_) | Node::Conjunction(_) => {}
            // Leaves: no child handles.
            Node::Atom(_)
            | Node::Bool(_)
            | Node::Long(_)
            | Node::Float(_)
            | Node::String(_)
            | Node::Space(_)
            | Node::State(_)
            | Node::Unit
            | Node::Memo(_)
            | Node::Empty
            | Node::NotReducible => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::cesk::index_arena::Addr;
    use crate::backend::models::metta_value::{reset_gc_mode_slab, set_gc_mode_index};

    #[test]
    fn node_is_copy_and_compact() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Node>();
        // The largest payload is Error(MettaValue, MettaValue) = 16 B; with the
        // discriminant Node must stay ≤ 32 B so a ≤8 MiB segment holds ≥256K
        // slots at DEFAULT_SEGMENT_CAPACITY. If this fails, retune the capacity.
        let sz = std::mem::size_of::<Node>();
        assert!(
            sz <= 32,
            "Node grew to {sz} B; retune DEFAULT_SEGMENT_CAPACITY"
        );
    }

    #[test]
    fn child_addrs_enumerates_inline_handle_children() {
        // `child_addrs` decodes child handles via `as_arena_addr`, which yields
        // an Addr only in Index mode. Flip the process-global mode for this test
        // (nextest isolates each test in its own process) and restore it after.
        set_gc_mode_index();
        let a = Addr::new(1, 2);
        let b = Addr::new(3, 4);
        let ha = MettaValue::from_addr(a, 0);
        let hb = MettaValue::from_addr(b, 0);

        let mut out = Vec::new();
        Node::Error(ha, hb).child_addrs(&mut out);
        assert_eq!(out, vec![a, b], "Error enumerates both child handles");

        out.clear();
        Node::Type(ha).child_addrs(&mut out);
        assert_eq!(out, vec![a]);

        out.clear();
        Node::Quoted(hb).child_addrs(&mut out);
        assert_eq!(out, vec![b]);

        out.clear();
        Node::Lazy(ha).child_addrs(&mut out);
        assert_eq!(out, vec![a]);

        out.clear();
        Node::Spanned(hb, SpanRef { idx: 7, gen: 0 }).child_addrs(&mut out);
        assert_eq!(
            out,
            vec![b],
            "Spanned enumerates its inner handle, not the span"
        );

        // Side-arena composites and leaves contribute nothing from a bare node.
        out.clear();
        Node::SExpr(ChildRef { idx: 0, gen: 0 }).child_addrs(&mut out);
        Node::Conjunction(ChildRef { idx: 1, gen: 0 }).child_addrs(&mut out);
        Node::Long(42).child_addrs(&mut out);
        Node::Atom(ByteRef { idx: 0, gen: 0 }).child_addrs(&mut out);
        Node::Space(9).child_addrs(&mut out);
        Node::Unit.child_addrs(&mut out);
        assert!(
            out.is_empty(),
            "SExpr/Conjunction (resolved by IndexHeap) and leaves push no addresses"
        );

        reset_gc_mode_slab();
    }
}
