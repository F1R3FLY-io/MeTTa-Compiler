//! Phase E4 — Serializable continuation slices (`#[cfg(feature = "index-gc")]`).
//!
//! A suspended CESK state `⟨C, E_local, a_k⟩` is serializable BY CONSTRUCTION in
//! the index store: every heap handle's payload is a `u32` arena [`Addr`] rather
//! than a raw pointer, so a captured slice is a flat, position-independent set of
//! `(opaque-old-Addr → SerNode)` entries plus the three seed root lists (control,
//! env-local, kont). This module captures, checkpoints (postcard), and restores
//! such slices, and is the runtime image of the
//! `SerializableContinuationSlice` Rocq theorem + the three TLC discriminators.
//!
//! ## Genuine-CESK closure (the load-bearing property)
//!
//! The captured slice IS `σ|_Reachable(⟨C, E_local, a_k⟩)`. The closure walk does
//! NOT roll its own edge relation: [`capture_slice`] seeds from the three root
//! lists and drives [`IndexHeap::reachable_closure`], which is itself a
//! non-bit-setting twin of the collector's transitive mark — it visits children
//! through the EXACT same [`IndexHeap::child_addrs_for_mark`] edge function that
//! [`IndexHeap::mark`] uses (`mark_from_roots_with`). Therefore the serialized
//! Addr set == the set the collector would keep live == `Reach(Seed)` in the
//! proof. This is `Hclosed` (the proof's "slice closed under `Edge`") satisfied
//! by reuse, not by a hand-written walk that could drift from the collector.
//!
//! ## Restore remaps to FRESH Addrs (never reuses source Addrs)
//!
//! [`Addr`] is `(segment << OFFSET_BITS) | offset`-encoded; a target arena's
//! `cur_seg`/free-list is independent, so reusing a raw source Addr would alias a
//! live node — exactly the use-after-free `restored_future_touch_not_freed`
//! forbids. [`restore_slice`] therefore re-interns every node children-before-
//! parents through the existing `IndexHeap::alloc_*` API, building a total
//! `remap: old_raw → new Addr`, and rebuilds C/E_local/K via
//! [`MettaValue::from_addr`]`(remap[old], flags)`. Capture stores raw Addrs as
//! OPAQUE keys; only restore binds them to a concrete arena.
//!
//! ## Discriminators (the #254 "reject without C/E/K closure" requirement)
//!
//! [`restore_slice`] rejects two malformed slices — the operational image of the
//! two negative TLC configs:
//! * [`SliceError::MissingKontRoot`] — a suspended K with frames but an
//!   empty/incomplete `kont` (== `MC_SerializableContinuationSlice_missing_kont`,
//!   `IncludeKont = FALSE`; a missing seed ⇒ `Hseed` fails).
//! * [`SliceError::MissingReachableChild`] — some serialized node names a child
//!   Addr absent from `nodes` (== `MC_SerializableContinuationSlice_missing_child`,
//!   `IncludeReachableChild = FALSE`; the runtime image of `Hclosed` failing).

#![allow(dead_code)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::backend::eval::cesk::index_arena::Addr;
use crate::backend::eval::cesk::index_heap::global_index_heap;
use crate::backend::eval::cesk::index_node::Node;
use crate::backend::models::MettaValue;

// ────────────────────────────────────────────────────────────────────────────
// Serializable mirrors of the inline-vs-heap value handle
// ────────────────────────────────────────────────────────────────────────────

/// A serializable inline scalar carried BY VALUE (no Addr): the `MettaValue`
/// NaN-box tags that never allocate a `Node`. Restoring one reproduces the exact
/// `tagged` bit pattern via the inline constructors, so no remap is involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SerInline {
    Bool(bool),
    /// 48-bit inline Long (the only Long form that is inline; wider Longs are
    /// heap `Node::Long` and therefore travel as a [`SlotRef::Heap`]).
    Long(i64),
    Unit,
    Empty,
}

/// A serialized reference to a `MettaValue`: either a heap handle (an opaque old
/// raw Addr plus the 4 handle flag bits, to be remapped on restore) or an inline
/// scalar carried by value.
///
/// The `flags` field is the low 4 bits of the source handle's `tagged`
/// (`FLAG_HAS_VARIABLES` etc.) so [`MettaValue::from_addr`]`(new_addr, flags)`
/// reconstructs the EXACT handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotRef {
    /// A heap handle: `raw` is the OLD packed `Addr::raw()` (an opaque key until
    /// restore binds it); `flags` are the low-4 handle flags.
    Heap { raw: u32, flags: u8 },
    /// An inline scalar carried by value (no Addr, no remap).
    Inline(SerInline),
}

impl SlotRef {
    /// Encode a live `MettaValue` into a [`SlotRef`]. Heap handles record their
    /// raw Addr + flags; inline scalars are carried by value.
    ///
    /// Requires the process to be in index mode (the E4 module is cfg-walled to
    /// `index-gc`, where that holds).
    pub fn from_value(v: MettaValue) -> Self {
        if let Some(addr) = v.as_arena_addr() {
            SlotRef::Heap {
                raw: addr.raw(),
                flags: (v.addr_flags() & 0xF) as u8,
            }
        } else {
            // Inline scalar — decode its NaN-box view and carry it by value.
            match v.view() {
                crate::backend::models::ValueView::Bool(b) => SlotRef::Inline(SerInline::Bool(b)),
                crate::backend::models::ValueView::Long(n) => SlotRef::Inline(SerInline::Long(n)),
                crate::backend::models::ValueView::Unit => SlotRef::Inline(SerInline::Unit),
                crate::backend::models::ValueView::Empty => SlotRef::Inline(SerInline::Empty),
                // Any other view at this point would be a heap handle that
                // `as_arena_addr` failed to decode — impossible in index mode for
                // a value that is not inline. Treat defensively as Empty so a
                // round-trip never panics; the closure/discriminator gates below
                // still protect store safety.
                _ => SlotRef::Inline(SerInline::Empty),
            }
        }
    }

    /// The old raw Addr this slot references, or `None` for an inline scalar.
    fn old_raw(&self) -> Option<u32> {
        match self {
            SlotRef::Heap { raw, .. } => Some(*raw),
            SlotRef::Inline(_) => None,
        }
    }

    /// Rebuild a live `MettaValue` for this slot, mapping a heap `raw` through
    /// `remap`. Returns `Err(MissingReachableChild)` if a heap slot's old raw is
    /// absent from `remap` (the runtime image of `Hclosed` failing).
    fn resolve(&self, remap: &HashMap<u32, (Addr, u8)>) -> Result<MettaValue, SliceError> {
        match self {
            SlotRef::Heap { raw, flags } => match remap.get(raw) {
                Some(&(new_addr, tag)) => Ok(MettaValue::from_addr(new_addr, *flags as usize, tag)),
                None => Err(SliceError::MissingReachableChild { old_raw: *raw }),
            },
            SlotRef::Inline(SerInline::Bool(b)) => Ok(MettaValue::inline_bool(*b)),
            // An inline Long is only ever encoded for a value that WAS inline at
            // capture (i.e. fits in 48 bits — `as_arena_addr` returned `None`), so
            // `try_inline_long` re-encodes it exactly.
            SlotRef::Inline(SerInline::Long(n)) => Ok(MettaValue::try_inline_long(*n)
                .expect("inline Long slot must fit the 48-bit inline encoding")),
            SlotRef::Inline(SerInline::Unit) => Ok(MettaValue::inline_unit()),
            SlotRef::Inline(SerInline::Empty) => Ok(MettaValue::inline_empty()),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Serializable span mirror (ir::Span is not Serialize)
// ────────────────────────────────────────────────────────────────────────────

/// Serde mirror of [`crate::ir::Position`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerPosition {
    pub row: usize,
    pub column: usize,
    pub byte_offset: usize,
}

/// Serde mirror of [`crate::ir::Span`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerSpan {
    pub start: SerPosition,
    pub end: SerPosition,
}

impl From<crate::ir::Span> for SerSpan {
    fn from(s: crate::ir::Span) -> Self {
        SerSpan {
            start: SerPosition {
                row: s.start.row,
                column: s.start.column,
                byte_offset: s.start.byte_offset,
            },
            end: SerPosition {
                row: s.end.row,
                column: s.end.column,
                byte_offset: s.end.byte_offset,
            },
        }
    }
}

impl From<SerSpan> for crate::ir::Span {
    fn from(s: SerSpan) -> Self {
        crate::ir::Span {
            start: crate::ir::Position {
                row: s.start.row,
                column: s.start.column,
                byte_offset: s.start.byte_offset,
            },
            end: crate::ir::Position {
                row: s.end.row,
                column: s.end.column,
                byte_offset: s.end.byte_offset,
            },
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Serializable mirror of the arena Node (18 variants)
// ────────────────────────────────────────────────────────────────────────────

/// A serde mirror of [`Node`] (index_node.rs) — all 18 variants. Variable-length
/// side data (SExpr/Conjunction children, Atom/String bytes, Spanned spans) is
/// flattened into DENSE indices into the slice's `children`/`bytes`/`spans`
/// vectors; child `MettaValue` handles travel as [`SlotRef`]. `Space`/`State`/
/// `Memo` carry their `u64` side-table id (the live `Arc`-backed handle stays in
/// the process-global heap's side table, which restore re-references in place).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SerNode {
    Atom { bytes_idx: u32 },
    Bool(bool),
    Long(i64),
    Float(f64),
    String { bytes_idx: u32 },
    SExpr { children_idx: u32 },
    Error(SlotRef, SlotRef),
    Type(SlotRef),
    Conjunction { children_idx: u32 },
    Space(u64),
    State(u64),
    Unit,
    Memo(u64),
    Quoted(SlotRef),
    Lazy(SlotRef),
    Empty,
    NotReducible,
    Spanned(SlotRef, u32),
}

impl SerNode {
    /// Experiment #18: TAG5 derive-at-restore. SerNode mirrors `Node` in
    /// IDENTICAL declaration order, and this table MUST equal
    /// `Node::variant_code()` (index_node.rs — the canonical mapping). The
    /// `inner_ref_index` materialization tripwire asserts handle-tag/node
    /// agreement on every materialization, so any divergence trips on the
    /// first restored handle the DEBUG oracle materializes.
    #[inline]
    fn variant_code(&self) -> u8 {
        match self {
            SerNode::Atom { .. } => 1,
            SerNode::Bool(_) => 2,
            SerNode::Long(_) => 3,
            SerNode::Float(_) => 4,
            SerNode::String { .. } => 5,
            SerNode::SExpr { .. } => 6,
            SerNode::Error(_, _) => 7,
            SerNode::Type(_) => 8,
            SerNode::Conjunction { .. } => 9,
            SerNode::Space(_) => 10,
            SerNode::State(_) => 11,
            SerNode::Unit => 12,
            SerNode::Memo(_) => 13,
            SerNode::Quoted(_) => 14,
            SerNode::Lazy(_) => 15,
            SerNode::Empty => 16,
            SerNode::NotReducible => 17,
            SerNode::Spanned(_, _) => 18,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The serialized slice
// ────────────────────────────────────────────────────────────────────────────

/// A serialized suspended CESK state: the three seed root lists plus the
/// store-closed set of nodes reachable from them.
///
/// `nodes` == `Reach(control ∪ env_local ∪ kont)` under the
/// `child_addrs_for_mark` edge relation (the proof's `Hclosed`), keyed by OLD raw
/// Addr (an opaque key; only [`restore_slice`] binds it to a concrete arena).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SerializedContinuationSlice {
    /// Control seed roots (`C` + pending control) — the proof's `ControlRoot`.
    pub control: Vec<SlotRef>,
    /// Env-local seed roots (`E_local`, the forked-env CoW-diverged Addrs) — the
    /// proof's `EnvRoot`.
    pub env_local: Vec<SlotRef>,
    /// Continuation seed roots (`a_k` / the K spine) — the proof's `KontRoot`.
    pub kont: Vec<SlotRef>,
    /// `true` when the suspended machine had a non-trivial continuation spine
    /// (frames present). When set, an EMPTY `kont` is the runtime image of
    /// `IncludeKont = FALSE` and is rejected with [`SliceError::MissingKontRoot`].
    pub kont_nonempty: bool,
    /// The store closure, keyed by OLD raw Addr (children before parents is NOT
    /// required for serialization — restore topo-sorts via the remap fixpoint).
    pub nodes: Vec<(u32, SerNode)>,
    /// Dense child-slice pool: `children[i]` is the children of any node whose
    /// `children_idx == i`.
    pub children: Vec<Vec<SlotRef>>,
    /// Dense byte pool: `bytes[i]` is the Atom/String text for `bytes_idx == i`.
    pub bytes: Vec<String>,
    /// Dense span pool: `spans[i]` is the Spanned span for the trailing index.
    pub spans: Vec<SerSpan>,
    /// Evaluation depth at suspension (carried verbatim into the restored state).
    pub depth: u32,
    /// Lifetime reduction count at suspension (carried verbatim).
    pub total_reductions: u64,
}

/// Errors a malformed slice can produce on restore. The two discriminator
/// variants are the runtime image of the two negative TLC configs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SliceError {
    /// A suspended K with frames but an empty/incomplete `kont` seed list
    /// (== `MC_SerializableContinuationSlice_missing_kont`).
    MissingKontRoot,
    /// A serialized node (or a seed) names a child Addr absent from `nodes`
    /// (== `MC_SerializableContinuationSlice_missing_child`; the image of
    /// `Hclosed` failing).
    MissingReachableChild { old_raw: u32 },
    /// Postcard (de)serialization failed.
    Codec(String),
}

impl std::fmt::Display for SliceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SliceError::MissingKontRoot => {
                write!(f, "restore rejected: suspended K spine has frames but the serialized kont seed list is empty (missing continuation root)")
            }
            SliceError::MissingReachableChild { old_raw } => write!(
                f,
                "restore rejected: reachable child Addr {old_raw:#x} is absent from the serialized slice (slice not store-closed)"
            ),
            SliceError::Codec(msg) => write!(f, "continuation-slice codec error: {msg}"),
        }
    }
}

impl std::error::Error for SliceError {}

/// A restored, resumable suspension over FRESH Addrs (the bridge produces a
/// [`crate::backend::eval::cesk::SuspendedEval`] from this — see
/// `reductions.rs::SuspendedEval::from_restored`).
#[derive(Clone, Debug)]
pub struct RestoredSuspension {
    /// Control roots rebuilt over fresh Addrs (the remapped `C` + pending control).
    pub control: Vec<MettaValue>,
    /// Env-local roots rebuilt over fresh Addrs (remapped `E_local`).
    pub env_local: Vec<MettaValue>,
    /// Kont roots rebuilt over fresh Addrs (remapped `a_k`).
    pub kont: Vec<MettaValue>,
    /// Evaluation depth carried from the slice.
    pub depth: u32,
    /// Lifetime reduction count carried from the slice.
    pub total_reductions: u64,
}

// ────────────────────────────────────────────────────────────────────────────
// Capture
// ────────────────────────────────────────────────────────────────────────────

/// Capture `σ|_Reachable(⟨control, env_local, kont⟩)` into a serializable slice.
///
/// 1. Each seed `MettaValue` is folded through [`MettaValue::as_arena_addr`]
///    (encoded as a [`SlotRef`]): heap handles record `(raw, flags)` AND seed the
///    closure; inline scalars are carried by value. This pins the proof's three
///    seed sets (`SerializedSeed` = control ∨ env ∨ kont).
/// 2. The closure walk reuses the GC reachability fold:
///    [`IndexHeap::reachable_closure`] over the heap seeds, visiting children
///    through [`IndexHeap::child_addrs_for_mark`] (the collector's exact edge
///    relation), so `nodes` == `Reach(Seed)` == the live set the GC keeps.
pub fn capture_slice(
    control: &[MettaValue],
    env_local: &[MettaValue],
    kont: &[MettaValue],
    depth: u32,
    total_reductions: u64,
) -> SerializedContinuationSlice {
    let control_refs: Vec<SlotRef> = control.iter().map(|v| SlotRef::from_value(*v)).collect();
    let env_local_refs: Vec<SlotRef> = env_local.iter().map(|v| SlotRef::from_value(*v)).collect();
    let kont_refs: Vec<SlotRef> = kont.iter().map(|v| SlotRef::from_value(*v)).collect();
    let kont_nonempty = !kont.is_empty();

    // Heap seed Addrs (the inputs to the GC reachability fold). Inline seeds are
    // already carried by value in the *_refs vectors and contribute no edges.
    let mut seeds: Vec<Addr> = Vec::with_capacity(control.len() + env_local.len() + kont.len());
    for r in control_refs
        .iter()
        .chain(env_local_refs.iter())
        .chain(kont_refs.iter())
    {
        if let Some(raw) = r.old_raw() {
            seeds.push(Addr::from_raw(raw));
        }
    }

    // Build the closure + emit each node, under the heap read lock.
    let heap = global_index_heap()
        .read()
        .expect("global index heap poisoned");
    // reachable_closure is the non-bit-setting twin of `mark`; it visits children
    // through the SAME `child_addrs_for_mark` edge function the collector uses.
    let closure: Vec<Addr> = heap.reachable_closure(&seeds);

    let mut nodes: Vec<(u32, SerNode)> = Vec::with_capacity(closure.len());
    let mut children: Vec<Vec<SlotRef>> = Vec::new();
    let mut bytes: Vec<String> = Vec::new();
    let mut spans: Vec<SerSpan> = Vec::new();
    for &addr in &closure {
        let ser = heap.emit_node(addr, &mut children, &mut bytes, &mut spans);
        nodes.push((addr.raw(), ser));
    }
    drop(heap);

    SerializedContinuationSlice {
        control: control_refs,
        env_local: env_local_refs,
        kont: kont_refs,
        kont_nonempty,
        nodes,
        children,
        bytes,
        spans,
        depth,
        total_reductions,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Restore
// ────────────────────────────────────────────────────────────────────────────

/// Restore a slice into the process-global store σ, re-interning every node to a
/// FRESH Addr (never reusing source Addrs) and rebuilding the three seed root
/// lists over the remap.
///
/// Rejections (the #254 "reject without C/E/K closure" gate, pinned in
/// source-coupling and mirrored by the two negative TLC configs):
/// * **D-KONT** — `kont_nonempty && kont.is_empty()` ⇒
///   [`SliceError::MissingKontRoot`].
/// * **D-CHILD** — any node's child Addr (or any seed's heap Addr) absent from
///   `nodes` ⇒ [`SliceError::MissingReachableChild`].
pub fn restore_slice(
    slice: &SerializedContinuationSlice,
) -> Result<RestoredSuspension, SliceError> {
    // D-KONT (missing-K-root): a non-trivial spine with no serialized kont seeds.
    if slice.kont_nonempty && slice.kont.is_empty() {
        return Err(SliceError::MissingKontRoot);
    }

    // D-CHILD (store-closure / Hclosed): every child Addr a node names, AND every
    // heap seed, must be present in `nodes`. Checking up-front makes the rejection
    // total and independent of re-intern order.
    let present: std::collections::HashSet<u32> = slice.nodes.iter().map(|(raw, _)| *raw).collect();
    let check_ref = |r: &SlotRef| -> Result<(), SliceError> {
        if let Some(raw) = r.old_raw() {
            if !present.contains(&raw) {
                return Err(SliceError::MissingReachableChild { old_raw: raw });
            }
        }
        Ok(())
    };
    for r in slice
        .control
        .iter()
        .chain(slice.env_local.iter())
        .chain(slice.kont.iter())
    {
        check_ref(r)?;
    }
    for (_raw, node) in &slice.nodes {
        match node {
            SerNode::SExpr { children_idx } | SerNode::Conjunction { children_idx } => {
                for c in &slice.children[*children_idx as usize] {
                    check_ref(c)?;
                }
            }
            SerNode::Error(a, b) => {
                check_ref(a)?;
                check_ref(b)?;
            }
            SerNode::Type(a) | SerNode::Quoted(a) | SerNode::Lazy(a) => check_ref(a)?,
            SerNode::Spanned(a, _) => check_ref(a)?,
            _ => {}
        }
    }

    // Re-intern children-before-parents via a remap fixpoint: a node is interned
    // only once every heap Addr it references is already in `remap`. Each pass
    // interns at least the leaves; a node graph that is store-closed (D-CHILD
    // passed) and acyclic-by-construction (σ nodes form a DAG — handles point only
    // at earlier-allocated nodes) converges. We bound the passes by node count.
    let heap_lock = global_index_heap();
    // Experiment #18 (derive-at-restore): the IN-MEMORY remap carries the
    // TAG5 variant code derived from each SerNode at insert — zero wire
    // change (SlotRef/postcard untouched), zero heap reads at resolve.
    let mut remap: HashMap<u32, (Addr, u8)> = HashMap::with_capacity(slice.nodes.len());
    let mut pending: Vec<&(u32, SerNode)> = slice.nodes.iter().collect();

    let max_passes = slice.nodes.len() + 1;
    for _pass in 0..max_passes {
        if pending.is_empty() {
            break;
        }
        let mut next_pending: Vec<&(u32, SerNode)> = Vec::with_capacity(pending.len());
        let mut progressed = false;
        for entry in pending.drain(..) {
            let (old_raw, node) = entry;
            if remap.contains_key(old_raw) {
                continue;
            }
            // Can this node be interned now? (all heap children already remapped)
            if !node_children_ready(node, &slice.children, &remap) {
                next_pending.push(entry);
                continue;
            }
            let new_addr = {
                let mut heap = heap_lock.write().expect("global index heap poisoned");
                intern_node(&mut heap, node, slice, &remap)?
            };
            remap.insert(*old_raw, (new_addr, node.variant_code()));
            progressed = true;
        }
        pending = next_pending;
        if !progressed && !pending.is_empty() {
            // No node became ready this pass yet some remain — the only way this
            // happens after the D-CHILD check is a dangling heap child, which the
            // up-front check already rejected. Report the first offender precisely.
            for (_old, node) in &pending {
                if let Some(raw) = first_unresolved_child(node, &slice.children, &remap) {
                    return Err(SliceError::MissingReachableChild { old_raw: raw });
                }
            }
            break;
        }
    }

    // Rebuild the three seed lists over the remap.
    let control = resolve_all(&slice.control, &remap)?;
    let env_local = resolve_all(&slice.env_local, &remap)?;
    let kont = resolve_all(&slice.kont, &remap)?;

    Ok(RestoredSuspension {
        control,
        env_local,
        kont,
        depth: slice.depth,
        total_reductions: slice.total_reductions,
    })
}

/// True iff every heap child of `node` is already in `remap` (so the node can be
/// interned now, children-before-parents).
fn node_children_ready(
    node: &SerNode,
    children: &[Vec<SlotRef>],
    remap: &HashMap<u32, (Addr, u8)>,
) -> bool {
    first_unresolved_child(node, children, remap).is_none()
}

/// The first heap child Addr of `node` that is NOT yet in `remap`, if any.
fn first_unresolved_child(
    node: &SerNode,
    children: &[Vec<SlotRef>],
    remap: &HashMap<u32, (Addr, u8)>,
) -> Option<u32> {
    let check = |r: &SlotRef| -> Option<u32> {
        match r.old_raw() {
            Some(raw) if !remap.contains_key(&raw) => Some(raw),
            _ => None,
        }
    };
    match node {
        SerNode::SExpr { children_idx } | SerNode::Conjunction { children_idx } => {
            children[*children_idx as usize].iter().find_map(check)
        }
        SerNode::Error(a, b) => check(a).or_else(|| check(b)),
        SerNode::Type(a) | SerNode::Quoted(a) | SerNode::Lazy(a) => check(a),
        SerNode::Spanned(a, _) => check(a),
        _ => None,
    }
}

/// Intern one `SerNode` (all of whose heap children are already in `remap`) into
/// the heap, returning its FRESH Addr.
fn intern_node(
    heap: &mut crate::backend::eval::cesk::index_heap::IndexHeap,
    node: &SerNode,
    slice: &SerializedContinuationSlice,
    remap: &HashMap<u32, (Addr, u8)>,
) -> Result<Addr, SliceError> {
    let addr = match node {
        SerNode::Atom { bytes_idx } => heap.alloc_atom(&slice.bytes[*bytes_idx as usize]),
        SerNode::String { bytes_idx } => heap.alloc_string(&slice.bytes[*bytes_idx as usize]),
        SerNode::Bool(b) => heap.alloc_fixed(Node::Bool(*b)),
        SerNode::Long(n) => heap.alloc_fixed(Node::Long(*n)),
        SerNode::Float(f) => heap.alloc_fixed(Node::Float(*f)),
        SerNode::Unit => heap.alloc_fixed(Node::Unit),
        SerNode::Empty => heap.alloc_fixed(Node::Empty),
        SerNode::NotReducible => heap.alloc_fixed(Node::NotReducible),
        // Space/State/Memo travel by side-table id; the process-global heap's
        // side tables already hold the live handle (the source node referenced it
        // and capture does not free the source), so we re-allocate a fixed node
        // referencing the SAME id.
        SerNode::Space(id) => heap.alloc_fixed(Node::Space(*id)),
        SerNode::State(id) => heap.alloc_fixed(Node::State(*id)),
        SerNode::Memo(id) => heap.alloc_fixed(Node::Memo(*id)),
        SerNode::SExpr { children_idx } => {
            let kids = resolve_all(&slice.children[*children_idx as usize], remap)?;
            heap.alloc_sexpr(&kids)
        }
        SerNode::Conjunction { children_idx } => {
            let kids = resolve_all(&slice.children[*children_idx as usize], remap)?;
            heap.alloc_conjunction(&kids)
        }
        SerNode::Error(a, b) => heap.alloc_fixed(Node::Error(a.resolve(remap)?, b.resolve(remap)?)),
        SerNode::Type(a) => heap.alloc_fixed(Node::Type(a.resolve(remap)?)),
        SerNode::Quoted(a) => heap.alloc_fixed(Node::Quoted(a.resolve(remap)?)),
        SerNode::Lazy(a) => heap.alloc_fixed(Node::Lazy(a.resolve(remap)?)),
        SerNode::Spanned(a, span_idx) => {
            let inner = a.resolve(remap)?;
            heap.alloc_spanned(inner, slice.spans[*span_idx as usize].into())
        }
    };
    // exp46: restore mints BYPASS the factory chokepoint (`alloc_with_reuse_
    // pressure`), so they must populate the shared Inner column themselves —
    // post-alloc, pre-escape (the handle is built from this Addr by `resolve`
    // consumers only after we return). `populate_column` skips Space/Memo
    // (id-store-served, v3-F1) and self-ensures the column segment.
    heap.populate_column(addr);
    Ok(addr)
}

/// Resolve a slice of [`SlotRef`]s to live `MettaValue`s over `remap`.
fn resolve_all(
    refs: &[SlotRef],
    remap: &HashMap<u32, (Addr, u8)>,
) -> Result<Vec<MettaValue>, SliceError> {
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        out.push(r.resolve(remap)?);
    }
    Ok(out)
}

// ────────────────────────────────────────────────────────────────────────────
// Checkpoint (postcard)
// ────────────────────────────────────────────────────────────────────────────

/// Serialize a slice to a compact binary checkpoint (postcard).
pub fn checkpoint(slice: &SerializedContinuationSlice) -> Result<Vec<u8>, SliceError> {
    postcard::to_allocvec(slice).map_err(|e| SliceError::Codec(e.to_string()))
}

/// Deserialize a checkpoint and restore it: the full
/// capture→serialize→deserialize→restore pipeline tail.
pub fn restore_from_bytes(buf: &[u8]) -> Result<RestoredSuspension, SliceError> {
    let slice: SerializedContinuationSlice =
        postcard::from_bytes(buf).map_err(|e| SliceError::Codec(e.to_string()))?;
    restore_slice(&slice)
}

#[cfg(all(test, feature = "index-gc"))]
mod tests {
    use super::*;
    use crate::backend::eval::cesk::index_heap::global_index_heap;
    use crate::backend::models::metta_value::set_gc_mode_index;
    use crate::backend::models::{MettaValueFactory, ValueView};

    /// Build a small heap value via the index factory; mode is set to index for
    /// the whole test process (nextest isolates each test).
    fn idx() -> crate::backend::eval::cesk::index_heap::IndexFactory {
        set_gc_mode_index();
        crate::backend::eval::cesk::index_heap::IndexFactory
    }

    /// `(foo (bar 1) baz)` — exercises SExpr children, nested SExpr, Atom bytes,
    /// and an inline Long child.
    fn sample_expr(f: &impl MettaValueFactory<MettaValue>) -> MettaValue {
        let bar = f.sexpr_from_slice(&[f.atom("bar"), f.long(1)]);
        f.sexpr_from_slice(&[f.atom("foo"), bar, f.atom("baz")])
    }

    fn view_to_string(v: MettaValue) -> String {
        // A stable structural rendering for equality (handles are not directly
        // comparable across remaps, but their printed structure is).
        v.to_metta_string()
    }

    #[test]
    fn capture_restore_round_trip_rebuilds_structure() {
        let f = idx();
        let c = sample_expr(&f);
        let before = view_to_string(c);

        let slice = capture_slice(&[c], &[], &[], 7, 4242);
        // The closure must contain at least: foo, bar-sexpr, the two/three atoms.
        assert!(
            slice.nodes.len() >= 4,
            "closure should hold the whole expression tree, got {}",
            slice.nodes.len()
        );
        assert_eq!(slice.depth, 7);
        assert_eq!(slice.total_reductions, 4242);

        let restored = restore_slice(&slice).expect("restore must succeed");
        assert_eq!(restored.control.len(), 1);
        let after = view_to_string(restored.control[0]);
        assert_eq!(
            before, after,
            "restored control value structurally identical"
        );
        assert_eq!(restored.depth, 7);
        assert_eq!(restored.total_reductions, 4242);
    }

    #[test]
    fn checkpoint_roundtrip_through_bytes() {
        let f = idx();
        let c = sample_expr(&f);
        let before = view_to_string(c);

        let slice = capture_slice(&[c], &[], &[], 3, 99);
        let buf = checkpoint(&slice).expect("checkpoint serializes");
        assert!(!buf.is_empty(), "postcard buffer non-empty");
        let restored = restore_from_bytes(&buf).expect("restore_from_bytes");
        assert_eq!(restored.control.len(), 1);
        assert_eq!(view_to_string(restored.control[0]), before);
    }

    #[test]
    fn fresh_remap_does_not_reuse_source_addrs() {
        let f = idx();
        let c = sample_expr(&f);
        let src_addr = c.as_arena_addr().expect("heap handle in index mode");

        let slice = capture_slice(&[c], &[], &[], 0, 0);
        let restored = restore_slice(&slice).expect("restore");
        let new_addr = restored.control[0]
            .as_arena_addr()
            .expect("restored heap handle");
        // FRESH remap: the restored root must be a DIFFERENT Addr than the source
        // (re-interned, never aliased — ConcurrentBumpFreshOnly).
        assert_ne!(
            src_addr.raw(),
            new_addr.raw(),
            "restore must re-intern to a fresh Addr, not reuse the source"
        );
    }

    #[test]
    fn d_child_missing_reachable_child_is_rejected() {
        let f = idx();
        let c = sample_expr(&f);
        let mut slice = capture_slice(&[c], &[], &[], 0, 0);

        // Delete a NON-seed referenced node (a leaf atom child). Find a node that
        // is NOT the control seed root and remove it from `nodes`.
        let root_raw = match slice.control[0] {
            SlotRef::Heap { raw, .. } => raw,
            _ => unreachable!("control root is a heap handle"),
        };
        let victim_pos = slice
            .nodes
            .iter()
            .position(|(raw, _)| *raw != root_raw)
            .expect("a non-root reachable node exists");
        let (victim_raw, _) = slice.nodes.remove(victim_pos);

        let err = restore_slice(&slice).expect_err("a non-closed slice must be rejected");
        assert_eq!(
            err,
            SliceError::MissingReachableChild {
                old_raw: victim_raw
            },
            "D-CHILD must reject with the missing child's old raw Addr"
        );
    }

    #[test]
    fn d_kont_missing_kont_root_is_rejected() {
        let f = idx();
        let c = sample_expr(&f);
        let k = f.atom("kont-frame-marker");

        // Capture WITH a non-trivial kont so kont_nonempty is set.
        let mut slice = capture_slice(&[c], &[], &[k], 0, 0);
        assert!(slice.kont_nonempty, "captured a non-trivial spine");
        assert!(!slice.kont.is_empty(), "kont seeds present at capture");

        // Now clear the kont seed list on the non-trivial spine.
        slice.kont.clear();
        let err = restore_slice(&slice).expect_err("an empty kont on a live spine is rejected");
        assert_eq!(err, SliceError::MissingKontRoot);
    }

    #[test]
    fn closure_equals_gc_reachable_set() {
        let f = idx();
        let c = sample_expr(&f);
        let seed = c.as_arena_addr().expect("heap handle");

        // The captured node key-set must equal the collector's mark-reached set
        // from the same seed (runtime analog of `reachable_store_in_serialized_slice`).
        let slice = capture_slice(&[c], &[], &[], 0, 0);
        let captured: std::collections::HashSet<u32> =
            slice.nodes.iter().map(|(raw, _)| *raw).collect();

        let gc_reached: std::collections::HashSet<u32> = {
            let heap = global_index_heap().read().expect("heap");
            heap.reachable_closure(&[seed])
                .into_iter()
                .map(|a| a.raw())
                .collect()
        };
        assert_eq!(
            captured, gc_reached,
            "capture_slice nodes key-set must equal IndexHeap::reachable_closure from the same seed"
        );
    }

    #[test]
    fn restored_future_touch_not_freed_after_source_swept() {
        // The empirical discharge of `restored_future_touch_not_freed`: capture a
        // slice, then FORCE a GC cycle that sweeps the source closure's slots (the
        // source value is dropped + unrooted), then restore (FRESH re-intern) and
        // READ the restored value. Restore must touch only the freshly re-interned
        // slots — never the freed source ones. Under ASAN a regression here is a
        // use-after-free; without ASAN this still asserts the restored structure.
        use crate::backend::eval::cesk::index_heap::global_index_heap;

        let f = idx();
        // A distinctive expression so the read after restore is observable.
        let c = f.sexpr_from_slice(&[f.atom("freed-probe"), f.long(123), f.atom("tail")]);
        let before = view_to_string(c);

        // Capture COPIES the closure into `slice`; the source σ nodes are now
        // redundant and may be freed.
        let slice = capture_slice(&[c], &[], &[], 0, 0);
        let src_raw = match slice.control[0] {
            SlotRef::Heap { raw, .. } => raw,
            _ => unreachable!(),
        };

        // Allocate an UNRELATED live value to keep as the GC root set, then force a
        // major mark+sweep. `c`'s closure is NOT in the root set, so its slots are
        // reclaimed (the source Addrs become freed/recyclable). We deliberately do
        // NOT root `c`.
        let keep = f.atom("unrelated-live-root");
        let keep_addr = keep.as_arena_addr().expect("heap handle");
        {
            let mut heap = global_index_heap().write().expect("heap");
            // Mark only the unrelated root, then sweep — reclaims everything else,
            // including the captured source closure.
            heap.mark(&[keep_addr]);
            let _ = heap.sweep();
        }
        // A raw IndexHeap mark+sweep is an INTERNAL collection step; the production
        // cycle (`mark_sweep_if_over_watermark`, index_heap.rs ~2442) ALWAYS follows
        // the sweep with the Addr-keyed thread-local cache invalidation. Replicate it
        // so a slot reused by restore no longer renders its prior occupant through
        // stale hash caches — exactly the consistent heap a real directive-boundary
        // `restore_slice` sees. (Restore itself is correct: it rebuilds FRESH nodes —
        // `view()` shows the right SExpr structure — this only refreshes the
        // display/hash caches the raw primitive left stale. The shared Inner column
        // needs no clear: reused cells are REWRITTEN by `populate_column` at the
        // restore mint, before the new handle escapes.)
        crate::backend::eval::trampoline::eval_loop::clear_aba_sensitive_caches();

        // Now restore: this re-interns the slice into FRESH Addrs and rebuilds the
        // control root. Reading it must not dereference any freed source slot.
        let restored = restore_slice(&slice).expect("restore after source sweep");
        let new_raw = restored.control[0]
            .as_arena_addr()
            .expect("restored heap handle")
            .raw();
        // The read below traverses the freshly re-interned store (UAF tripwire).
        let after = view_to_string(restored.control[0]);
        assert_eq!(before, after, "restored value reads correctly post-sweep");
        // Sanity: the restored root is a fresh Addr (it may or may not equal the
        // recycled source slot numerically — what matters is the bytes are the
        // freshly re-interned ones, asserted by the structural equality above).
        let _ = (src_raw, new_raw);
        // Keep `keep` live until here so the sweep above could not reclaim it.
        assert!(keep.as_arena_addr().is_some());
    }

    #[test]
    fn restored_value_resumes_under_eval() {
        // The restored value must be a usable σ value: drive it through a trivial
        // structural check (its view must match the captured one), exercising a
        // read over the freshly re-interned store.
        let f = idx();
        let c = f.sexpr_from_slice(&[f.atom("+"), f.long(2), f.long(3)]);
        let slice = capture_slice(&[c], &[], &[], 0, 0);
        let restored = restore_slice(&slice).expect("restore");
        match restored.control[0].view() {
            ValueView::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0].view(), ValueView::Atom("+")));
                assert!(matches!(items[1].view(), ValueView::Long(2)));
                assert!(matches!(items[2].view(), ValueView::Long(3)));
            }
            other => panic!("restored value should be the (+ 2 3) sexpr, got {other:?}"),
        }
    }
}
