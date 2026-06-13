//! exp46 — the arena-coresident shared Inner column (design v3.1, CONVERGED:
//! `docs/cesk-gc/inner-column-v2-design-2026-06-12.md`).
//!
//! One `MettaValueInner` per node slot, built ONCE at intern and read by ALL
//! workers — replacing the per-thread `INNER_SHADOW` whose re-materialization
//! multiplied work N-fold under FANOUT (the 2.25× parallel decomposition,
//! `ecc7a5f9`; the N=8 gate measured an 86.7% aggregate-CPU win).
//!
//! ## The discipline (mirrors `IndexArena::segments`, index_arena.rs:484-487)
//!
//! - A STATIC, never-realloc directory of `MAX_SEGMENTS` cells, OUTSIDE the
//!   heap `RwLock` (v3-F3): readers take NO lock. Cell `i` is initialized
//!   exactly once, under [`COLUMN_DIR_LOCK`], before [`COLUMN_SEG_COUNT`] is
//!   advanced past `i` (Release); readers dereference cell `i` only for
//!   `i < COLUMN_SEG_COUNT.load(Acquire)`.
//! - Cell payloads are POD (v3-F1: only Copy-payload `MettaValueInner`
//!   variants are ever written — `TAG5_SPACE`/`TAG5_MEMO` are served from the
//!   heap's append-only id store instead), so overwrite-at-reuse leaks
//!   nothing and segment release frees wholesale (the B1 sweep loops are
//!   untouched).
//! - Entry writes happen POST-publish (v3-F2): the factory writes the cell
//!   after the node alloc returns and BEFORE the handle escapes. Soundness:
//!   the ONLY column reader is handle-mediated (`inner_ref_index` v2) — no
//!   scanner walks column cells by published length, and a handle that has
//!   not escaped cannot be read. Cross-thread visibility rides the handle's
//!   own escape channel (pool handoff / mutex+condvar / space RwLock /
//!   hash-cons write lock / `thread::spawn` — the R2-parallel trace found no
//!   pre-Release escape).
//! - Segment RELEASE only for fully-dead segments (no live handle names an
//!   Addr in them ⇒ no reader can reach the cells — the I2 argument).

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::backend::eval::cesk::index_arena::{Addr, MAX_SEGMENTS};
use crate::backend::models::MettaValueInner;

/// One segment's column: `capacity` cells, written by the allocating thread
/// (post-publish, pre-escape) and read lock-free by every worker.
struct ColumnSeg {
    cells: Box<[UnsafeCell<MaybeUninit<MettaValueInner>>]>,
}

// SAFETY: the cell-write/handle-escape protocol (module doc) guarantees no
// cell is written while another thread reads it: a cell is written (a) once
// before its handle first escapes, or (b) at slot REUSE, which only happens
// after a sweep proved no live handle names the Addr (collector soundness),
// i.e. no reader can hold or derive a reference to the cell across the write.
unsafe impl Sync for ColumnSeg {}
unsafe impl Send for ColumnSeg {}

struct ColumnDir {
    cells: Box<[UnsafeCell<MaybeUninit<Box<ColumnSeg>>>]>,
}
// SAFETY: directory cell `i` is written exactly once under COLUMN_DIR_LOCK
// before COLUMN_SEG_COUNT advances past `i` (Release); readers dereference
// only `i < COLUMN_SEG_COUNT.load(Acquire)` — the IndexArena::segments
// discipline verbatim.
unsafe impl Sync for ColumnDir {}
unsafe impl Send for ColumnDir {}

static COLUMN_DIR: OnceLock<ColumnDir> = OnceLock::new();
static COLUMN_SEG_COUNT: AtomicUsize = AtomicUsize::new(0);
static COLUMN_DIR_LOCK: Mutex<()> = Mutex::new(());

#[inline]
fn dir() -> &'static ColumnDir {
    COLUMN_DIR.get_or_init(|| {
        let mut v: Vec<UnsafeCell<MaybeUninit<Box<ColumnSeg>>>> =
            Vec::with_capacity(MAX_SEGMENTS);
        v.resize_with(MAX_SEGMENTS, || UnsafeCell::new(MaybeUninit::uninit()));
        ColumnDir {
            cells: v.into_boxed_slice(),
        }
    })
}

/// Publish column segments up through `seg` (each `capacity` cells).
/// Called from the heap's `ensure_side_seg` (any alloc path that opens a
/// segment) — idempotent, growth-locked, Release-published.
pub(crate) fn ensure_column_seg(seg: usize, capacity: usize) {
    if seg < COLUMN_SEG_COUNT.load(Ordering::Acquire) {
        return;
    }
    let d = dir();
    let _guard = COLUMN_DIR_LOCK.lock().expect("column dir lock poisoned");
    let mut next = COLUMN_SEG_COUNT.load(Ordering::Acquire);
    if seg < next {
        return;
    }
    assert!(seg < MAX_SEGMENTS, "column directory exhausted");
    while next <= seg {
        let mut cells: Vec<UnsafeCell<MaybeUninit<MettaValueInner>>> =
            Vec::with_capacity(capacity);
        cells.resize_with(capacity, || UnsafeCell::new(MaybeUninit::uninit()));
        let boxed = Box::new(ColumnSeg {
            cells: cells.into_boxed_slice(),
        });
        // SAFETY: cell `next` is unpublished (next == COLUMN_SEG_COUNT) and we
        // hold the growth lock — exactly-once initialization.
        unsafe {
            (*d.cells[next].get()).write(boxed);
        }
        next += 1;
        COLUMN_SEG_COUNT.store(next, Ordering::Release);
    }
}

/// Write the column entry for `addr` (the allocating thread, post-publish,
/// PRE-ESCAPE — see the module doc). POD-only payloads (v3-F1): the caller
/// (factory/heap) never passes `Space`/`Memo` variants.
///
/// # Safety
/// The caller owns the slot (it just allocated or reused it; no live handle
/// to `addr` exists on any other thread yet).
#[inline]
pub(crate) unsafe fn column_write(addr: Addr, inner: MettaValueInner) {
    debug_assert!(
        !matches!(
            inner,
            MettaValueInner::Space(_) | MettaValueInner::Memo(_)
        ),
        "Space/Memo are served from the id store, never the column (v3-F1)"
    );
    let seg = addr.segment() as usize;
    debug_assert!(
        seg < COLUMN_SEG_COUNT.load(Ordering::Acquire),
        "column_write before ensure_column_seg({seg})"
    );
    let d = dir();
    // SAFETY: seg published (debug-asserted; production callers run after
    // ensure_side_seg which grows the column in lockstep); the slot is
    // exclusively ours per the function contract.
    let seg_ref = &*(*d.cells[seg].get()).assume_init_ref();
    (*seg_ref.cells[addr.offset() as usize].get()).write(inner);
}

/// The lock-free two-load read: `addr → COLUMN_DIR[seg] → cell` (v3.1).
/// Returns a `&'static` into the column — address-stable until the segment
/// is released, which only happens for fully-dead segments (I2: a live
/// handle to `addr` precludes release of its segment).
///
/// # Safety
/// `addr` must come from a LIVE handle (collector soundness guarantees the
/// cell was written before the handle escaped and has not been released).
#[inline(always)]
pub(crate) unsafe fn column_read(addr: Addr) -> &'static MettaValueInner {
    let seg = addr.segment() as usize;
    debug_assert!(
        seg < COLUMN_SEG_COUNT.load(Ordering::Acquire),
        "column_read of unpublished segment {seg}"
    );
    let d = dir();
    let seg_ref = &*(*d.cells[seg].get()).assume_init_ref();
    (*seg_ref.cells[addr.offset() as usize].get()).assume_init_ref()
}

/// Wholesale-drop a fully-dead segment's cells (POD — no per-cell drops
/// needed; this just releases the memory by replacing the box).
/// Called from the heap's segment-release path under the write lock.
pub(crate) fn column_release_seg(seg: usize, capacity: usize) {
    if seg >= COLUMN_SEG_COUNT.load(Ordering::Acquire) {
        return;
    }
    let d = dir();
    let _guard = COLUMN_DIR_LOCK.lock().expect("column dir lock poisoned");
    // SAFETY: the caller (heap segment release, write-locked) guarantees the
    // segment is fully dead — no live handle can name an Addr in it, so no
    // reader holds or can derive a pointer into these cells (I2). Replacing
    // the ColumnSeg box frees the old cells; the fresh (uninit) cells are
    // ready for the slot indices' reuse after re-interning.
    unsafe {
        let cell = &mut *(*d.cells[seg].get()).assume_init_mut();
        let mut cells: Vec<UnsafeCell<MaybeUninit<MettaValueInner>>> =
            Vec::with_capacity(capacity);
        cells.resize_with(capacity, || UnsafeCell::new(MaybeUninit::uninit()));
        cell.cells = cells.into_boxed_slice();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_write_read_roundtrip_and_growth() {
        ensure_column_seg(0, 64);
        ensure_column_seg(2, 64); // grows 0..=2 idempotently
        assert!(COLUMN_SEG_COUNT.load(Ordering::Acquire) >= 3);
        let a = Addr::new(2, 7);
        unsafe {
            column_write(a, MettaValueInner::Long(424242));
            match column_read(a) {
                MettaValueInner::Long(n) => assert_eq!(*n, 424242),
                other => panic!("wrong variant: {other:?}"),
            }
            // overwrite-at-reuse is a plain POD overwrite
            column_write(a, MettaValueInner::Bool(true));
            assert!(matches!(column_read(a), MettaValueInner::Bool(true)));
        }
    }

    #[test]
    fn column_release_replaces_cells() {
        ensure_column_seg(1, 16);
        let a = Addr::new(1, 3);
        unsafe {
            column_write(a, MettaValueInner::Float(1.5));
            column_release_seg(1, 16);
            // After release the cell is uninit — re-intern writes before any
            // read (the production contract); here we just rewrite + read.
            column_write(a, MettaValueInner::Long(7));
            assert!(matches!(column_read(a), MettaValueInner::Long(7)));
        }
    }
}
