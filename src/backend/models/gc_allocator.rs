//! Lock-Free Global Slab Allocator for GC-Managed MeTTa Values
//!
//! This module provides a lock-free, thread-safe, single-global slab allocator
//! that replaces both the heap-based `MettaValue` (Arc) and the bumpalo dual-arena
//! model. All `MettaValue` instances are allocated from this allocator.
//!
//! ## Design
//!
//! - **Fixed-size value slots**: All `MettaValueInner` instances are the same size
//!   (Rust enum = largest variant). One slab with uniform slots.
//! - **Power-of-2 data classes**: Variable-length data (strings, slices) allocated
//!   in size-class buckets (16, 32, 64, ... 4096 bytes).
//! - **Lock-free Treiber stack**: Free lists use 128-bit ABA-safe CAS via
//!   `portable-atomic` (`CMPXCHG16B` on x86-64, `LDXP/STXP` on ARM64).
//! - **Atomic bump allocation**: Per-page atomic bump pointer via CAS.
//! - **mmap-based pages**: Guaranteed OS memory release via `munmap`.
//! - **Global singleton**: Single `SlabAllocator` per application via `OnceLock`.
//!
//! ## Thread Safety
//!
//! The allocator is fully thread-safe without mutexes on the hot path:
//! - **Allocation**: Lock-free Treiber stack pop + atomic bump (CAS loops)
//! - **Free**: Lock-free Treiber stack push (CAS loop)
//! - **Page creation**: `RwLock` on page array (writes are rare; reads for GC)
//! - **Large allocs**: `Mutex` on large_allocs Vec (rare, not on hot path)
//!
//! ## Memory Safety
//!
//! The `'static` lifetime on `MettaValue` is valid because the global
//! allocator lives for the entire program duration. Values must not be accessed
//! after the allocator is dropped (only at program exit).

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use std::alloc::Layout;
use std::cell::Cell;
use std::collections::HashMap;
use std::mem;
use std::ptr;
use std::sync::atomic::{
    AtomicBool, AtomicIsize, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use parking_lot::{Condvar, Mutex, RwLock};
use portable_atomic::AtomicU128;

use super::metta_value::read_varint;
use super::metta_value::serialize_tags::*;
use super::metta_value::{MettaValue, MettaValueInner};
use super::metta_value_trait::MettaValueFactory;

// ============================================================================
// Fibonacci Hash for Pointer HashSets (re-exported from hash_utils)
// ============================================================================

pub(crate) use crate::backend::hash_utils::{PtrBuildHasher, PtrHashSet};

// ============================================================================
// GC Trace Flag (METTA_GC_TRACE environment variable)
// ============================================================================


// ============================================================================
// Constants
// ============================================================================

/// Page size for value and data slabs (256 KB).
/// Larger pages reduce mmap syscall overhead (4x fewer pages for the same
/// total allocation). Trade-off: coarser page release granularity during GC.
pub(crate) const PAGE_SIZE: usize = 256 * 1024;

/// Power-of-2 size classes for variable-length data.
/// Minimum 16 bytes to hold the Treiber stack `FreeNode` (u128 = 16 bytes).
const DATA_SIZE_CLASSES: [usize; 9] = [16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

/// Alignment for all slots (16 bytes for SIMD-friendly access).
const SLOT_ALIGN: usize = 16;

/// Minimum GC threshold (4 MB).
const MIN_GC_THRESHOLD: usize = 4 * 1024 * 1024;

/// Null sentinel for Treiber stack (no free slots).
const TREIBER_NULL: u128 = 0;

// ============================================================================
// Hash-Consing Table for Ground S-Expressions (Phase 5)
// ============================================================================
//
// Thread-local deduplication table for ground (variable-free) S-expressions.
// When `sexpr()` or `sexpr_from_slice()` is called with all-ground children,
// we compute a content hash from the children's tagged pointer values and
// look up in this table. On hit, we return the existing MettaValue — zero
// allocation. This provides:
//
// 1. O(1) PartialEq via pointer equality for structurally identical expressions
// 2. Elimination of redundant slab allocation for repeated ground sub-expressions
// 3. Improved hash cache hit rates (same pointer → same cached hash)
//
// The table is cleared at GC safepoints alongside VALUE_HASH_CACHE.

/// Golden ratio for hash-consing content hash (Boost hash_combine).
const CONS_GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;

thread_local! {
    /// Hash-consing table: content hash of children → existing MettaValue.
    ///
    /// Key: u64 content hash computed from children's `tagged` pointer values.
    /// Value: the previously-allocated MettaValue with identical structure.
    ///
    /// Only ground S-expressions (FLAG_HAS_VARIABLES == 0 for all children)
    /// are eligible for consing. Variable-containing expressions are always
    /// freshly allocated to avoid aliasing issues during unification.
    ///
    /// Cleared at GC safepoints via `clear_hash_cons_table()` to prevent
    /// stale entries from referencing freed slab slots.
    ///
    /// Uses FxBuildHasher since keys are already well-distributed content hashes.
    static HASH_CONS_TABLE: Cell<Option<Box<HashMap<u64, MettaValue, crate::backend::hash_utils::FxBuildHasher>>>> =
        const { Cell::new(None) };

    /// GC sweep epoch observed by this thread's hash-consing table.
    ///
    /// Work-pool threads are reused and may miss a safepoint while idle. The
    /// epoch check lets them lazily clear stale slab-backed entries before the
    /// next lookup dereferences a value from the table.
    static HASH_CONS_EPOCH: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn clear_hash_cons_table_local() {
    HASH_CONS_TABLE.with(|cell| {
        let maybe_map = cell.take();
        if let Some(mut map) = maybe_map {
            map.clear();
            cell.set(Some(map)); // Reuse allocation
        }
    });
}

#[inline]
fn ensure_hash_cons_epoch_current() {
    let current_epoch = gc_sweep_epoch();
    HASH_CONS_EPOCH.with(|epoch| {
        if epoch.get() != current_epoch {
            clear_hash_cons_table_local();
            epoch.set(current_epoch);
        }
    });
}

/// Compute a content hash for an S-expression's children using their tagged pointer values.
/// Uses Boost-style hash_combine (non-commutative, non-self-cancelling).
///
/// **Mode-independent** (it hashes only the children's `tagged` bits, which are
/// stable handle identities in both Slab and Index mode), so the index-arena
/// hash-cons table (`IndexHeap::intern_ground_sexpr`) reuses this exact function
/// to stay byte-for-byte key-compatible with the slab table — a prerequisite for
/// the Inc-3 A/B differential's fixpoint-identity parity (R9).
#[inline]
pub(crate) fn hash_cons_key(items: &[MettaValue]) -> u64 {
    let mut combined: u64 = items.len() as u64;
    for item in items {
        let ptr_hash = item.tagged as u64;
        combined ^= ptr_hash
            .wrapping_add(CONS_GOLDEN_RATIO)
            .wrapping_add(combined << 6)
            .wrapping_add(combined >> 2);
    }
    combined
}

/// Look up a ground S-expression in the hash-consing table.
/// Returns `Some(existing)` if found with matching children, `None` otherwise.
#[inline]
fn hash_cons_lookup(key: u64, items: &[MettaValue]) -> Option<MettaValue> {
    ensure_hash_cons_epoch_current();
    HASH_CONS_TABLE.with(|cell| {
        // SAFETY: We take the Option out, inspect it, and put it back.
        // No re-entrancy possible within this scope.
        let maybe_map = cell.take();
        let result = if let Some(ref map) = maybe_map {
            if let Some(&existing) = map.get(&key) {
                // Verify the children match exactly (handle hash collisions)
                if let MettaValueInner::SExpr(existing_items) = existing.inner_ref() {
                    if existing_items.len() == items.len()
                        && existing_items
                            .iter()
                            .zip(items.iter())
                            .all(|(a, b)| a.tagged == b.tagged)
                    {
                        Some(existing)
                    } else {
                        None // Hash collision
                    }
                } else {
                    None // Corrupted entry
                }
            } else {
                None
            }
        } else {
            None
        };
        cell.set(maybe_map);
        result
    })
}

/// Insert a ground S-expression into the hash-consing table.
#[inline]
fn hash_cons_insert(key: u64, value: MettaValue) {
    ensure_hash_cons_epoch_current();
    HASH_CONS_TABLE.with(|cell| {
        let mut maybe_map = cell.take();
        let map = maybe_map.get_or_insert_with(|| {
            Box::new(HashMap::with_capacity_and_hasher(
                256,
                crate::backend::hash_utils::FxBuildHasher,
            ))
        });
        // Cap table size to prevent unbounded growth
        if map.len() < 8192 {
            map.insert(key, value);
        }
        cell.set(maybe_map);
    })
}

/// Clear the hash-consing table. Must be called at GC safepoints.
pub fn clear_hash_cons_table() {
    clear_hash_cons_table_local();
    let current_epoch = gc_sweep_epoch();
    HASH_CONS_EPOCH.with(|epoch| epoch.set(current_epoch));
}

// ============================================================================
// MmapPage — OS-Backed Page Allocation
// ============================================================================

/// A page of memory backed by `mmap(MAP_PRIVATE | MAP_ANONYMOUS)`.
///
/// When dropped, `munmap` is called, guaranteeing the physical memory and
/// virtual address space are immediately returned to the OS. This ensures
/// RSS decreases when GC releases empty pages.
struct MmapPage {
    ptr: *mut u8,
    len: usize,
}

impl MmapPage {
    /// Allocate a new mmap-backed page of the given size (must be page-aligned).
    fn new(size: usize) -> Self {
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            ) as *mut u8
        };
        assert!(
            !ptr.is_null() && ptr != libc::MAP_FAILED as *mut u8,
            "mmap failed for {} bytes",
            size
        );
        Self { ptr, len: size }
    }

    #[inline]
    fn as_ptr(&self) -> *const u8 {
        self.ptr as *const u8
    }
}

impl Drop for MmapPage {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

// SAFETY: MmapPage is just a pointer + length to OS-backed memory.
// The memory is not shared — each page is owned by a single allocator.
unsafe impl Send for MmapPage {}
unsafe impl Sync for MmapPage {}

// ============================================================================
// ValuePage — Fixed-Size Slots for MettaValueInner (Atomic)
// ============================================================================

/// A page of fixed-size slots for `MettaValueInner` values.
///
/// Each page is a contiguous 64 KB mmap'd block divided into uniform slots.
/// All counters are atomic for lock-free concurrent access.
pub(crate) struct ValuePage {
    /// Raw page memory (mmap-backed for guaranteed OS release).
    data: MmapPage,
    /// Number of slots that have been bump-allocated (atomic for CAS bump).
    bump_count: AtomicUsize,
    /// Maximum slots this page can hold (immutable after creation).
    capacity: usize,
    /// Number of live slots (bumped minus freed). Signed for safe concurrent
    /// inc/dec. When <= 0, page may be eligible for release.
    live_count: AtomicIsize,
    /// Per-slot epoch for TOCTOU prevention. When a slot is re-allocated from
    /// the free list, its epoch is set to the allocator's current epoch.
    /// GC checks: if slot_epoch > snapshot_epoch, skip (re-allocated after snapshot).
    epochs: Vec<AtomicU64>,
    /// Per-slot session context ID for session-based GC.
    /// Context 0 = persistent (never released by session GC).
    /// Other values identify the session that allocated the slot.
    context_ids: Vec<AtomicU32>,
    /// Per-slot execution counter for tiered compilation.
    /// Incremented by eval threads via `increment_exec_count()` (atomic fetch_add).
    /// Flushed to the global TieredCache DashMap by periodic cron task and
    /// before GC frees dead slots.
    exec_counts: Vec<AtomicU32>,
    /// Per-slot cached TieredCache hash (xxh3 of expression content).
    /// Zero = not yet computed. Populated by GC cron counter flush on first
    /// encounter, cleared when slot is freed (GC Phase 3).
    /// Used by trampoline for O(1) DashMap lookup (avoids recursive re-hashing).
    compilation_hashes: Vec<AtomicU64>,
}

impl ValuePage {
    /// Create a new value page for the given slot size.
    fn new(slot_size: usize) -> Self {
        let capacity = PAGE_SIZE / slot_size;
        let data = MmapPage::new(PAGE_SIZE);
        let epochs: Vec<AtomicU64> = (0..capacity).map(|_| AtomicU64::new(0)).collect();
        let context_ids: Vec<AtomicU32> = (0..capacity).map(|_| AtomicU32::new(0)).collect();
        let exec_counts: Vec<AtomicU32> = (0..capacity).map(|_| AtomicU32::new(0)).collect();
        let compilation_hashes: Vec<AtomicU64> = (0..capacity).map(|_| AtomicU64::new(0)).collect();
        Self {
            data,
            bump_count: AtomicUsize::new(0),
            capacity,
            live_count: AtomicIsize::new(0),
            epochs,
            context_ids,
            exec_counts,
            compilation_hashes,
        }
    }

    /// Get pointer to slot at the given index.
    #[inline]
    pub(crate) fn slot_ptr(&self, idx: usize, slot_size: usize) -> *mut u8 {
        debug_assert!(idx < self.capacity);
        unsafe { self.data.ptr.add(idx * slot_size) }
    }

    /// Try to atomically bump-allocate the next slot. Returns None if page is full.
    #[inline]
    fn bump_alloc(&self, slot_size: usize) -> Option<(*mut u8, usize)> {
        loop {
            let current = self.bump_count.load(Ordering::Acquire);
            if current >= self.capacity {
                return None;
            }
            // CAS: try to claim this slot
            match self.bump_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.live_count.fetch_add(1, Ordering::Relaxed);
                    let ptr = self.slot_ptr(current, slot_size);
                    return Some((ptr, current));
                }
                Err(_) => continue, // Retry
            }
        }
    }

    /// Check if this page contains the given pointer.
    #[inline]
    fn contains(&self, ptr: *const u8, slot_size: usize) -> bool {
        let start = self.data.as_ptr() as usize;
        let end = start + self.capacity * slot_size;
        let addr = ptr as usize;
        addr >= start && addr < end
    }

    /// Compute the slot index for a pointer within this page.
    #[inline]
    fn slot_index(&self, ptr: *const u8, slot_size: usize) -> Option<usize> {
        let start = self.data.as_ptr() as usize;
        let offset = (ptr as usize).wrapping_sub(start);
        if offset < self.capacity * slot_size {
            let idx = offset / slot_size;
            if idx < self.bump_count.load(Ordering::Acquire) {
                Some(idx)
            } else {
                None
            }
        } else {
            None
        }
    }


    /// Get slot epoch (atomic).
    #[inline]
    pub(crate) fn slot_epoch(&self, idx: usize) -> u64 {
        self.epochs[idx].load(Ordering::Acquire)
    }

    /// Set slot epoch (atomic).
    #[inline]
    fn set_slot_epoch(&self, idx: usize, epoch: u64) {
        self.epochs[idx].store(epoch, Ordering::Release);
    }


    /// Set slot context ID (atomic).
    #[inline]
    fn set_context_id(&self, idx: usize, id: u32) {
        self.context_ids[idx].store(id, Ordering::Release);
    }

    /// Get the exec_count for a slot (atomic, Relaxed).
    #[inline]
    pub(crate) fn exec_count(&self, idx: usize) -> u32 {
        self.exec_counts[idx].load(Ordering::Relaxed)
    }

    /// Atomically add to the exec_count for a slot (Relaxed).
    #[inline]
    pub(crate) fn exec_count_fetch_add(&self, idx: usize, val: u32) {
        self.exec_counts[idx].fetch_add(val, Ordering::Relaxed);
    }

    /// Atomically subtract from the exec_count for a slot (Relaxed).
    #[inline]
    pub(crate) fn exec_count_fetch_sub(&self, idx: usize, val: u32) {
        self.exec_counts[idx].fetch_sub(val, Ordering::Relaxed);
    }


    /// Get the bump_count (number of allocated slots).
    #[inline]
    pub(crate) fn bump_count(&self) -> usize {
        self.bump_count.load(Ordering::Acquire)
    }

    /// Get a raw pointer to exec_counts[0] for thread-local cache.
    ///
    /// The returned pointer is valid for the page's lifetime (until `release_empty_pages`
    /// drops the page). Callers must check the page generation counter to detect stale
    /// pointers from released pages.
    #[inline]
    pub(crate) fn exec_counts_ptr(&self) -> *const AtomicU32 {
        self.exec_counts.as_ptr()
    }

    /// Get cached compilation hash for a slot (Relaxed).
    #[inline]
    pub(crate) fn compilation_hash(&self, idx: usize) -> u64 {
        self.compilation_hashes[idx].load(Ordering::Relaxed)
    }

    /// Set cached compilation hash for a slot (Relaxed).
    #[inline]
    pub(crate) fn set_compilation_hash(&self, idx: usize, hash: u64) {
        self.compilation_hashes[idx].store(hash, Ordering::Relaxed);
    }

    /// Get raw pointer to compilation_hashes[0] for thread-local cache.
    #[inline]
    pub(crate) fn compilation_hashes_ptr(&self) -> *const AtomicU64 {
        self.compilation_hashes.as_ptr()
    }
}

// ============================================================================
// DataPage — Variable-Length Data Slots (Atomic)
// ============================================================================

/// A page of same-sized data slots for one size class.
struct DataPage {
    data: MmapPage,
    bump_count: AtomicUsize,
    capacity: usize,
    live_count: AtomicIsize,
}

impl DataPage {
    fn new(slot_size: usize) -> Self {
        let capacity = PAGE_SIZE / slot_size;
        let data = MmapPage::new(PAGE_SIZE);
        Self {
            data,
            bump_count: AtomicUsize::new(0),
            capacity,
            live_count: AtomicIsize::new(0),
        }
    }

    #[inline]
    fn slot_ptr(&self, idx: usize, slot_size: usize) -> *mut u8 {
        debug_assert!(idx < self.capacity);
        unsafe { self.data.ptr.add(idx * slot_size) }
    }

    /// Try to atomically bump-allocate the next slot.
    #[inline]
    fn bump_alloc(&self, slot_size: usize) -> Option<*mut u8> {
        loop {
            let current = self.bump_count.load(Ordering::Acquire);
            if current >= self.capacity {
                return None;
            }
            match self.bump_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.live_count.fetch_add(1, Ordering::Relaxed);
                    return Some(self.slot_ptr(current, slot_size));
                }
                Err(_) => continue,
            }
        }
    }

    /// Check if this page contains the given pointer.
    #[inline]
    fn contains(&self, ptr: *const u8, slot_size: usize) -> bool {
        let start = self.data.as_ptr() as usize;
        let end = start + self.capacity * slot_size;
        let addr = ptr as usize;
        addr >= start && addr < end
    }
}

// ============================================================================
// Treiber Stack — Lock-Free Free List
// ============================================================================
//
// ABA-safe via 128-bit atomics (`portable-atomic`):
//   - x86-64: Uses `CMPXCHG16B` instruction.
//   - ARM64:  Uses `LDXP/STXP` (LL/SC) — inherently ABA-immune.
//   - RISC-V: Uses `LR/SC` — inherently ABA-immune.
//
// Layout: [64-bit counter (high) | 64-bit pointer (low)]
// The 64-bit counter makes wrap-around effectively impossible (2^64 ops).

/// Pack a pointer and 64-bit counter into a single u128.
#[inline]
fn treiber_pack(ptr: *mut u8, counter: u64) -> u128 {
    ((counter as u128) << 64) | (ptr as u64 as u128)
}

/// Unpack pointer from a packed u128.
#[inline]
fn treiber_unpack_ptr(packed: u128) -> *mut u8 {
    (packed as u64) as *mut u8
}

/// Unpack counter from a packed u128.
#[inline]
fn treiber_unpack_counter(packed: u128) -> u64 {
    (packed >> 64) as u64
}

/// Free-list node stored at the beginning of a freed slot.
/// The slot must be at least 16 bytes to hold this u128 next pointer.
/// `MettaValueInner` slots are >= 48 bytes; the smallest data class is 16 bytes.
///
/// Uses `AtomicU128` for the `next` field because the Treiber stack `pop()`
/// speculatively reads `next` from a node that may have already been popped
/// by another thread and is being overwritten (e.g., zeroed by `alloc_value`).
/// The CAS will reject the stale read, but the act of reading must be atomic
/// to avoid undefined behavior from a data race.
#[repr(C)]
struct FreeNode {
    /// Packed [64-bit counter | 64-bit pointer] to the next free node.
    next: AtomicU128,
}

/// Lock-free Treiber stack for free slot management.
struct TreiberStack {
    head: AtomicU128,
}

impl TreiberStack {
    fn new() -> Self {
        Self {
            head: AtomicU128::new(TREIBER_NULL),
        }
    }

    /// Validate that a packed Treiber value has a properly aligned pointer part.
    /// Returns true for TREIBER_NULL or valid 16-byte-aligned pointers.
    #[inline]
    fn validate_packed(packed: u128, label: &str) {
        if packed == TREIBER_NULL {
            return;
        }
        let ptr = treiber_unpack_ptr(packed);
        let addr = ptr as usize;
        debug_assert!(
            addr % SLOT_ALIGN == 0,
            "TreiberStack::{label}: misaligned pointer 0x{addr:x} in packed \
             0x{packed:032x} (counter={})",
            treiber_unpack_counter(packed),
        );
        debug_assert!(
            addr < 0x0000_8000_0000_0000, // must be in userspace
            "TreiberStack::{label}: non-userspace pointer 0x{addr:x} in packed \
             0x{packed:032x}",
        );
    }

    /// Push a freed slot onto the stack (lock-free).
    fn push(&self, ptr: *mut u8) {
        debug_assert!(
            !ptr.is_null() && (ptr as usize) % SLOT_ALIGN == 0,
            "TreiberStack::push: invalid pointer {:?}",
            ptr,
        );
        loop {
            let old_head = self.head.load(Ordering::Acquire);
            Self::validate_packed(old_head, "push(old_head)");
            // Write the current head as this node's next pointer (atomic store
            // to match the atomic load in pop() — prevents TSan data race reports
            // even though this store is not yet visible to other threads until
            // the CAS below publishes it).
            let node = ptr as *const FreeNode;
            unsafe {
                (*node).next.store(old_head, Ordering::Release);
            }
            // Pack with incremented counter for ABA prevention
            let old_counter = treiber_unpack_counter(old_head);
            let new_head = treiber_pack(ptr, old_counter.wrapping_add(1));
            Self::validate_packed(new_head, "push(new_head)");
            match self.head.compare_exchange_weak(
                old_head,
                new_head,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(_) => continue,
            }
        }
    }

    /// Pop a free slot from the stack (lock-free). Returns None if empty.
    fn pop(&self) -> Option<*mut u8> {
        loop {
            let old_head = self.head.load(Ordering::Acquire);
            if old_head == TREIBER_NULL {
                return None;
            }
            Self::validate_packed(old_head, "pop(old_head)");
            let ptr = treiber_unpack_ptr(old_head);
            let old_counter = treiber_unpack_counter(old_head);
            // Read the next pointer from the node. This is a SPECULATIVE read:
            // another thread may have already popped this slot and started
            // overwriting it via write_slot_bytes(), so `next` may contain
            // MettaValueInner data rather than a valid Treiber-packed pointer.
            // The CAS below rejects stale reads; we only validate after success.
            let next = unsafe { (*(ptr as *const FreeNode)).next.load(Ordering::Acquire) };
            // Pack with incremented counter (may be garbage if `next` is stale —
            // the CAS will reject it).
            let new_head = if next == TREIBER_NULL {
                TREIBER_NULL
            } else {
                treiber_pack(treiber_unpack_ptr(next), old_counter.wrapping_add(1))
            };
            match self.head.compare_exchange_weak(
                old_head,
                new_head,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // CAS succeeded — old_head was still the head, so no
                    // concurrent pop/write_slot_bytes touched this node.
                    // `next` and `new_head` are guaranteed valid.
                    Self::validate_packed(next, "pop(next)");
                    Self::validate_packed(new_head, "pop(new_head)");
                    return Some(ptr);
                }
                Err(_) => continue,
            }
        }
    }

}

// ============================================================================
// ASAN Integration for Custom Slab Allocator
// ============================================================================
//
// Standard AddressSanitizer doesn't know about our slab allocator's internal
// slot recycling. Without manual poisoning, ASAN sees all slab memory as valid
// (mmap'd pages are always accessible) and cannot detect use-after-free when
// we recycle a slot via the Treiber stack free list.
//
// These helpers call ASAN's manual poisoning API to mark freed slots as
// inaccessible and re-mark them as accessible on allocation. This gives ASAN
// full visibility into our custom allocator's lifecycle, producing proper
// use-after-free reports with allocation and deallocation stacks.
//
// When ASAN is not active (normal builds), these are compile-time no-ops.

/// Poison a slab slot after freeing it.
///
/// Marks bytes after the FreeNode header as inaccessible to ASAN. The first
/// `size_of::<FreeNode>()` bytes (16 bytes) are left unpoisoned because the
/// Treiber stack stores `FreeNode.next` (a u128) at the beginning of the freed slot.
#[inline(always)]
#[allow(unused_variables)]
unsafe fn asan_poison_slab_slot(ptr: *mut u8, slot_size: usize) {
    #[cfg(sanitize = "address")]
    {
        extern "C" {
            fn __asan_poison_memory_region(addr: *const std::ffi::c_void, size: usize);
        }
        let free_node_size = mem::size_of::<FreeNode>();
        if slot_size > free_node_size {
            __asan_poison_memory_region(
                ptr.add(free_node_size) as *const std::ffi::c_void,
                slot_size - free_node_size,
            );
        }
    }
}

/// Unpoison a slab slot before using it after allocation.
///
/// Marks the entire slot as accessible to ASAN.
#[inline(always)]
#[allow(unused_variables)]
unsafe fn asan_unpoison_slab_slot(ptr: *mut u8, slot_size: usize) {
    #[cfg(sanitize = "address")]
    {
        extern "C" {
            fn __asan_unpoison_memory_region(addr: *const std::ffi::c_void, size: usize);
        }
        __asan_unpoison_memory_region(ptr as *const std::ffi::c_void, slot_size);
    }
}


// ============================================================================
// DataClassAllocator — Per-Size-Class Allocator (Thread-Safe)
// ============================================================================

/// Allocator for one power-of-2 data size class.
struct DataClassAllocator {
    slot_size: usize,
    pages: RwLock<Vec<Box<DataPage>>>,
    free_list: TreiberStack,
    /// Pointer to the current page for bump allocation.
    current_page: AtomicPtr<DataPage>,
}

impl DataClassAllocator {
    fn new(slot_size: usize) -> Self {
        Self {
            slot_size,
            pages: RwLock::new(Vec::new()),
            free_list: TreiberStack::new(),
            current_page: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Allocate a slot of this size class (lock-free hot path).
    ///
    /// After popping from the free list, validates the pointer against the
    /// pages vector under a read-lock. If the page was munmapped by
    /// `release_empty_pages()` between the pop and the lock acquisition,
    /// the stale pointer is discarded and allocation falls through to the
    /// bump allocator.
    fn alloc(&self) -> *mut u8 {
        // Fast path: pop from Treiber stack free list
        if let Some(ptr) = self.free_list.pop() {
            // Validate that the page still exists — it may have been munmapped
            // by release_empty_pages() between our pop() and this read-lock.
            let pages = self.pages.read();
            for page in pages.iter() {
                if page.contains(ptr as *const u8, self.slot_size) {
                    // Page still mapped — safe to reuse this slot
                    page.live_count.fetch_add(1, Ordering::Relaxed);
                    // ASAN: unpoison the slot before reuse
                    unsafe {
                        asan_unpoison_slab_slot(ptr, self.slot_size);
                    }
                    return ptr;
                }
            }
            // Page was munmapped between pop() and read-lock — discard stale
            // pointer and fall through to bump allocation.
        }

        // Try bump-allocating from the current page
        let page_ptr = self.current_page.load(Ordering::Acquire);
        if !page_ptr.is_null() {
            let page = unsafe { &*page_ptr };
            if let Some(ptr) = page.bump_alloc(self.slot_size) {
                // NOTE: page.live_count already incremented inside bump_alloc()
                return ptr;
            }
        }

        // Need a new page — acquire write lock (rare)
        self.alloc_new_page()
    }

    /// Slow path: allocate a new page.
    fn alloc_new_page(&self) -> *mut u8 {
        let mut pages = self.pages.write();
        // Double-check: another thread may have added a page
        if let Some(last) = pages.last() {
            if let Some(ptr) = last.bump_alloc(self.slot_size) {
                // NOTE: page.live_count already incremented inside bump_alloc()
                return ptr;
            }
        }
        let page = Box::new(DataPage::new(self.slot_size));
        let ptr = page
            .bump_alloc(self.slot_size)
            .expect("fresh page should have room");
        // NOTE: page.live_count already incremented inside bump_alloc()
        let page_ptr = &*page as *const DataPage as *mut DataPage;
        self.current_page.store(page_ptr, Ordering::Release);
        pages.push(page);
        ptr
    }

    /// Increment the live_count of the page containing the given pointer.
    // DISABLED: increment_page_live_count is no longer needed — alloc() now
    // inlines the page lookup to validate pointers after free_list.pop(),
    // combining the page existence check with the live_count increment.
    //
    // fn increment_page_live_count(&self, ptr: *mut u8) {
    //     let pages = self.pages.read();
    //     for page in pages.iter() {
    //         if page.contains(ptr as *const u8, self.slot_size) {
    //             page.live_count.fetch_add(1, Ordering::Relaxed);
    //             return;
    //         }
    //     }
    // }

    /// Return a slot to the free list (lock-free).
    /// Note: single-slot free is now handled by SlabAllocator::free_data_slot()
    /// with thread-local caching. This method is retained for batch-free paths.
    #[allow(dead_code)]
    fn free(&self, ptr: *mut u8) {
        // Decrement page live_count
        {
            let pages = self.pages.read();
            for page in pages.iter() {
                if page.contains(ptr as *const u8, self.slot_size) {
                    page.live_count.fetch_sub(1, Ordering::Relaxed);
                    break;
                }
            }
        }
        // ASAN: poison the freed slot BEFORE push (skip FreeNode header used by Treiber stack).
        // Must poison before push to avoid race: another thread could pop() + unpoison()
        // between push and poison, then we'd poison an in-use slot.
        unsafe {
            asan_poison_slab_slot(ptr, self.slot_size);
        }
        self.free_list.push(ptr);
    }


    /// Total bytes committed by this size class.
    fn committed_bytes(&self) -> usize {
        let pages = self.pages.read();
        pages.len() * PAGE_SIZE
    }

}

// ============================================================================
// Thread-Local Free-List Cache (TreiberStack Contention Reduction)
// ============================================================================

/// Maximum number of validated pointers in the thread-local cache.
const THREAD_CACHE_CAPACITY: usize = 64;

/// Number of slots to batch-pop from global TreiberStack when the cache is empty.
const THREAD_CACHE_REFILL: usize = 32;

/// Global generation counter, incremented when pages are munmapped.
/// Thread caches compare against this to detect stale pointers.
static CACHE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// A cached allocation slot with pre-computed page pointer and slot index.
///
/// Storing `*const ValuePage` eliminates `pages.read()` + O(P) linear scan on
/// Tier 1 cache hits. The raw page pointer is valid as long as the generation
/// matches — pages are only freed in `release_empty_pages()` which bumps
/// `CACHE_GENERATION`, invalidating all caches before any stale pointer is used.
#[derive(Clone, Copy)]
struct CachedSlot {
    /// Raw pointer to the allocated slot memory.
    ptr: *mut u8,
    /// Raw pointer to the owning ValuePage (stable within a generation).
    page: *const ValuePage,
    /// Slot index within the page (for epoch/context stamping).
    slot_idx: u16,
}

/// Thread-local cache of pre-validated free-list slots.
///
/// Each slot has been popped from the global TreiberStack and validated
/// against the pages vector. Allocations from the cache are O(1) with
/// no CAS, no page validation, and no RwLock acquisition.
#[derive(Clone, Copy)]
struct ThreadFreeCache {
    /// Pre-validated slots. Only `slots[0..len]` are valid.
    slots: [CachedSlot; THREAD_CACHE_CAPACITY],
    /// Number of valid slots in `slots`.
    len: usize,
    /// Generation at which these pointers were validated.
    /// If `CACHE_GENERATION` has advanced, all pointers must be discarded.
    generation: u64,
}

// SAFETY: Raw pointers in the cache are only used by the owning thread.
// The cache is thread-local (never shared).
unsafe impl Send for ThreadFreeCache {}

impl ThreadFreeCache {
    const EMPTY_SLOT: CachedSlot = CachedSlot {
        ptr: ptr::null_mut(),
        page: ptr::null(),
        slot_idx: 0,
    };

    const fn new() -> Self {
        Self {
            slots: [Self::EMPTY_SLOT; THREAD_CACHE_CAPACITY],
            len: 0,
            generation: 0,
        }
    }

    /// Pop a cached slot from the cache. Returns None if empty.
    #[inline(always)]
    fn pop(&mut self) -> Option<CachedSlot> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.slots[self.len])
    }

    /// Push a cached slot into the cache. Returns the slot back if full.
    #[inline(always)]
    fn push(&mut self, slot: CachedSlot) -> Option<CachedSlot> {
        if self.len >= THREAD_CACHE_CAPACITY {
            return Some(slot);
        }
        self.slots[self.len] = slot;
        self.len += 1;
        None
    }

    // NOTE: drain_ptrs() removed — was only used by flush_value_cache_to_global()
    // which pushed stale pointers to the Treiber stack, causing SIGSEGV on munmapped
    // pages. Stale caches are now silently discarded via `cache.len = 0`.

    /// Check if the generation matches the global generation.
    /// If not, all cached pointers are stale and must be discarded.
    #[inline(always)]
    fn is_valid_generation(&self) -> bool {
        self.generation == CACHE_GENERATION.load(Ordering::Acquire)
    }

    /// Update the cached generation to the current global generation.
    #[inline(always)]
    fn sync_generation(&mut self) {
        self.generation = CACHE_GENERATION.load(Ordering::Acquire);
    }
}

thread_local! {
    /// Thread-local free-list cache for value slots.
    static VALUE_CACHE: Cell<ThreadFreeCache> = const { Cell::new(ThreadFreeCache::new()) };
}

// NOTE: flush_value_cache_to_global() removed — it pushed stale pointers (potentially
// in munmapped pages) to the global Treiber stack, where TreiberStack::push() writes
// FreeNode::next at the pointer's address, causing SIGSEGV. Stale caches are now
// silently discarded via `cache.len = 0` (matching the data cache pattern).

// ============================================================================
// Thread-Local Data Free-List Cache (DataClassAllocator Contention Reduction)
// ============================================================================

/// Maximum number of cached pointers per data size class.
/// Doubled from 32 to 64 to halve TreiberStack::pop frequency (~3.16% CPU).
/// Memory: 64 ptrs × 9 size classes × 8 bytes = 4.5 KB per thread.
const DATA_CACHE_CAPACITY: usize = 64;

/// Number of slots to batch-pop from global TreiberStack when the data cache is empty.
const DATA_CACHE_REFILL: usize = 32;

/// Global generation counter for data pages, incremented when data pages are munmapped.
/// Data caches compare against this to detect stale pointers.
static DATA_CACHE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Thread-local cache of free-list pointers for a single data size class.
///
/// Unlike `ThreadFreeCache` (for values), data slots don't need epoch/context
/// stamping or page pointer pre-computation — they don't participate in GC
/// marking. Only raw pointer caching is needed.
#[derive(Clone, Copy)]
struct ThreadDataCache {
    /// Cached free-list pointers. Only `ptrs[0..len]` are valid.
    ptrs: [*mut u8; DATA_CACHE_CAPACITY],
    /// Number of valid pointers in `ptrs`.
    len: usize,
    /// Generation at which these pointers were validated.
    generation: u64,
}

// SAFETY: Raw pointers in the cache are only used by the owning thread.
// The cache is thread-local (never shared).
unsafe impl Send for ThreadDataCache {}

impl ThreadDataCache {
    const fn new() -> Self {
        Self {
            ptrs: [ptr::null_mut(); DATA_CACHE_CAPACITY],
            len: 0,
            generation: 0,
        }
    }

    /// Pop a cached pointer. Returns None if empty.
    #[inline(always)]
    fn pop(&mut self) -> Option<*mut u8> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.ptrs[self.len])
    }

    /// Push a pointer into the cache. Returns true if full (caller should flush).
    #[inline(always)]
    fn push(&mut self, ptr: *mut u8) -> bool {
        if self.len >= DATA_CACHE_CAPACITY {
            return true; // full
        }
        self.ptrs[self.len] = ptr;
        self.len += 1;
        false
    }

    /// Check if the generation matches the global data generation.
    #[inline(always)]
    fn is_valid_generation(&self) -> bool {
        self.generation == DATA_CACHE_GENERATION.load(Ordering::Acquire)
    }

    /// Update the cached generation to the current global data generation.
    #[inline(always)]
    fn sync_generation(&mut self) {
        self.generation = DATA_CACHE_GENERATION.load(Ordering::Acquire);
    }
}

/// Thread-local data caches: one `ThreadDataCache` per size class (9 total).
///
/// Stored as a flat array indexed by size class index (0..9).
/// Uses `Cell` for interior mutability without locking (single-threaded access).
#[derive(Clone, Copy)]
struct ThreadDataCacheSet {
    caches: [ThreadDataCache; 9],
}

impl ThreadDataCacheSet {
    const fn new() -> Self {
        Self {
            caches: [ThreadDataCache::new(); 9],
        }
    }
}

thread_local! {
    /// Thread-local free-list caches for data size classes (9 caches, one per class).
    static DATA_CACHES: Cell<ThreadDataCacheSet> = const { Cell::new(ThreadDataCacheSet::new()) };
}

// ============================================================================
// ValueAllocator — Fixed-Size Value Slot Manager (Thread-Safe)
// ============================================================================

/// Allocator for fixed-size `MettaValueInner` slots.
struct ValueAllocator {
    /// Size of each slot (aligned to SLOT_ALIGN).
    slot_size: usize,
    /// All allocated pages. RwLock: reads are hot (GC snapshots), writes are rare.
    pages: RwLock<Vec<Box<ValuePage>>>,
    /// Lock-free Treiber stack free list.
    free_list: TreiberStack,
    /// Monotonic epoch counter for TOCTOU prevention.
    epoch: AtomicU64,
    /// Pointer to current page for bump allocation.
    current_page: AtomicPtr<ValuePage>,
}

impl ValueAllocator {
    fn new() -> Self {
        let raw_size = mem::size_of::<MettaValueInner>();
        let slot_size = (raw_size + SLOT_ALIGN - 1) & !(SLOT_ALIGN - 1);
        Self {
            slot_size,
            pages: RwLock::new(Vec::new()),
            free_list: TreiberStack::new(),
            epoch: AtomicU64::new(0),
            current_page: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Allocate a value slot (lock-free hot path).
    ///
    /// Uses a three-tier allocation strategy:
    /// 1. **Thread-local cache** — O(1), no CAS, no page validation
    /// 2. **Global TreiberStack** — CAS pop + batch page validation
    /// 3. **Bump allocation** — atomic bump pointer from current page
    ///
    /// Free-list allocations increment the epoch and tag the slot.
    /// Bump allocations don't need epoch tagging.
    /// All paths stamp the current thread's session context ID.
    fn alloc(&self) -> *mut u8 {
        let ctx_id = current_context_id();
        let slot_size = self.slot_size;

        // Tier 1: Thread-local cache (O(1), no CAS, no RwLock, no page scan)
        //
        // Phase 9 optimization: CachedSlot stores a raw `*const ValuePage` pointer
        // and pre-computed slot_idx, eliminating both `pages.read()` and O(P) scan.
        // Safety: generation check invalidates the entire cache before any page is freed.
        let cached = VALUE_CACHE.try_with(|cell| {
            // SAFETY: Cell<ThreadFreeCache> is only accessed from this thread.
            // We take ownership, modify, and put back — no concurrent access.
            let mut cache = cell.get();

            // Check generation validity
            if !cache.is_valid_generation() {
                // Stale pointers may reference munmapped pages — silently discard.
                // Cannot push back to global free list because push() writes
                // FreeNode::next at the pointer's address, which would SIGSEGV
                // if the page was munmapped by release_empty_pages().
                cache.len = 0;
                cache.sync_generation();
            }

            if let Some(slot) = cache.pop() {
                // Re-check generation after pop to close TOCTOU window.
                // Between is_valid_generation() above and this pop, a concurrent
                // release_empty_pages() could have munmapped pages and bumped
                // generation. If so, the popped CachedSlot may reference a
                // munmapped page — discard entire cache and fall through to Tier 2.
                if cache.generation != CACHE_GENERATION.load(Ordering::Acquire) {
                    cache.len = 0;
                    cache.sync_generation();
                    cell.set(cache);
                    return None; // fall through to Tier 2
                }
                cell.set(cache);
                // SAFETY: Generation double-check above guarantees the page hasn't been freed.
                // Pages are append-only within a generation — only release_empty_pages()
                // modifies the Vec, which bumps generation and invalidates all caches first.
                let page = unsafe { &*slot.page };
                let new_epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
                page.set_slot_epoch(slot.slot_idx as usize, new_epoch);
                page.set_context_id(slot.slot_idx as usize, ctx_id);
                page.live_count.fetch_add(1, Ordering::Relaxed);
                unsafe {
                    asan_unpoison_slab_slot(slot.ptr, slot_size);
                }
                return Some(slot.ptr);
            }

            // Cache empty — batch-refill from global TreiberStack
            let mut batch: [*mut u8; THREAD_CACHE_REFILL] = [ptr::null_mut(); THREAD_CACHE_REFILL];
            let mut batch_len = 0;
            for slot in batch.iter_mut() {
                if let Some(ptr) = self.free_list.pop() {
                    *slot = ptr;
                    batch_len += 1;
                } else {
                    break;
                }
            }

            if batch_len > 0 {
                // Validate entire batch under a single pages.read() lock.
                // Record (page_ptr, slot_idx) for each valid pointer — these are stored
                // in CachedSlot so future Tier 1 hits avoid pages.read() entirely.
                let pages = self.pages.read();
                let mut first_slot: Option<CachedSlot> = None;

                for i in 0..batch_len {
                    let ptr = batch[i];
                    let mut found = false;
                    for page in pages.iter() {
                        if let Some(idx) = page.slot_index(ptr as *const u8, slot_size) {
                            let cached_slot = CachedSlot {
                                ptr,
                                page: &**page as *const ValuePage,
                                slot_idx: idx as u16,
                            };
                            if first_slot.is_none() {
                                first_slot = Some(cached_slot);
                            } else {
                                let _ = cache.push(cached_slot);
                            }
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        // Invalid pointer (from munmapped page) — silently discarded
                    }
                }
                drop(pages);

                if let Some(slot) = first_slot {
                    cache.sync_generation();
                    cell.set(cache);

                    // Complete allocation metadata using the pre-computed page/slot_idx
                    // SAFETY: We just validated the page above under pages.read().
                    // Generation hasn't changed (we're in the same alloc call).
                    let page = unsafe { &*slot.page };
                    let new_epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
                    page.set_slot_epoch(slot.slot_idx as usize, new_epoch);
                    page.set_context_id(slot.slot_idx as usize, ctx_id);
                    page.live_count.fetch_add(1, Ordering::Relaxed);
                    unsafe {
                        asan_unpoison_slab_slot(slot.ptr, slot_size);
                    }
                    return Some(slot.ptr);
                }
            }

            cache.sync_generation();
            cell.set(cache);
            None
        });

        // If thread-local access succeeded and returned a pointer, use it
        if let Ok(Some(ptr)) = cached {
            return ptr;
        }

        // Tier 2: Direct global TreiberStack pop (fallback when thread-local fails)
        if let Some(ptr) = self.free_list.pop() {
            let pages = self.pages.read();
            for page in pages.iter() {
                if let Some(idx) = page.slot_index(ptr as *const u8, slot_size) {
                    let new_epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
                    page.set_slot_epoch(idx, new_epoch);
                    page.set_context_id(idx, ctx_id);
                    page.live_count.fetch_add(1, Ordering::Relaxed);
                    unsafe {
                        asan_unpoison_slab_slot(ptr, slot_size);
                    }
                    return ptr;
                }
            }
            // Page was munmapped — discard stale pointer
        }

        // Tier 3: Bump-allocate from the current page
        let page_ptr = self.current_page.load(Ordering::Acquire);
        if !page_ptr.is_null() {
            let page = unsafe { &*page_ptr };
            if let Some((ptr, idx)) = page.bump_alloc(slot_size) {
                page.set_context_id(idx, ctx_id);
                return ptr;
            }
        }

        // Need a new page — acquire write lock (rare)
        self.alloc_new_page_with_ctx(ctx_id)
    }

    /// Slow path: allocate a new page with a specific context ID.
    fn alloc_new_page_with_ctx(&self, ctx_id: u32) -> *mut u8 {
        let mut pages = self.pages.write();
        // Double-check: another thread may have added a page while we waited
        if let Some(last) = pages.last() {
            if let Some((ptr, idx)) = last.bump_alloc(self.slot_size) {
                // NOTE: page.live_count already incremented inside bump_alloc()
                last.set_context_id(idx, ctx_id);
                return ptr;
            }
        }
        let page = Box::new(ValuePage::new(self.slot_size));
        let (ptr, idx) = page
            .bump_alloc(self.slot_size)
            .expect("fresh page should have room");
        // NOTE: page.live_count already incremented inside bump_alloc()
        page.set_context_id(idx, ctx_id);
        let page_ptr = &*page as *const ValuePage as *mut ValuePage;
        self.current_page.store(page_ptr, Ordering::Release);
        pages.push(page);
        ptr
    }

    /// Return a value slot to the free list (lock-free).
    ///
    /// Sets the slot epoch to `u64::MAX` before pushing to the free list.
    /// This prevents the GC sweep from treating already-freed slots as dead
    /// (the epoch filter in `process_gc_response` checks `slot_epoch > snapshot_epoch`,
    /// and `u64::MAX` is always greater than any snapshot epoch).
    ///
    /// Without this, an empty `free_set` in `build_snapshot()` causes double-freeing:
    /// the sweep sees the freed slot as "allocated but unmarked" and adds it to the
    /// dead set again. The Treiber stack then gets the same pointer pushed twice,
    /// creating a cycle that causes two allocations to alias the same slot → SEGV.
    ///
    /// Corresponds to TLA+ model's `freeSet` tracking in `ProcessGcResponse`.
    fn free(&self, ptr: *mut u8) {
        // Decrement page live_count and set slot epoch to u64::MAX (sentinel)
        {
            let pages = self.pages.read();
            for page in pages.iter() {
                if let Some(idx) = page.slot_index(ptr as *const u8, self.slot_size) {
                    page.live_count.fetch_sub(1, Ordering::Relaxed);
                    // Sentinel epoch: marks slot as freed so GC sweep won't re-free it.
                    // Cleared on re-allocation (alloc() sets epoch to current allocator epoch).
                    page.set_slot_epoch(idx, u64::MAX);
                    break;
                }
            }
        }
        // ASAN: mark slot as inaccessible (detects use-after-free)
        unsafe {
            asan_poison_slab_slot(ptr, self.slot_size);
        }
        self.free_list.push(ptr);
    }

    /// Check if a pointer belongs to this allocator.
    fn contains(&self, ptr: *const u8) -> bool {
        let pages = self.pages.read();
        pages.iter().any(|page| page.contains(ptr, self.slot_size))
    }

    /// Total committed bytes.
    fn committed_bytes(&self) -> usize {
        let pages = self.pages.read();
        pages.len() * PAGE_SIZE
    }

}

// ============================================================================
// SlabAllocator — Top-Level Lock-Free Allocator
// ============================================================================

/// Global lock-free slab allocator for GC-managed MeTTa values.
///
/// Provides O(1) lock-free allocation for both fixed-size values and
/// variable-length data. Thread-safe without mutexes on the hot path.
pub struct SlabAllocator {
    /// Allocator for MettaValueInner (fixed-size slots)
    values: ValueAllocator,
    /// Allocators for variable-length data (strings, slices)
    data_classes: Vec<DataClassAllocator>,
    /// Fallback for data > 4096 bytes (rare, uses system allocator)
    large_allocs: Mutex<Vec<(*mut u8, Layout)>>,
    /// GC trigger threshold (bytes). Arc-wrapped for sharing with cron manager.
    gc_threshold: Arc<AtomicUsize>,
    /// Atomic committed bytes for cross-thread reads (cron manager).
    committed_bytes_atomic: Arc<AtomicUsize>,
    /// Atomic allocation counter for cross-thread reads (cron manager).
    alloc_count_atomic: Arc<AtomicU64>,
}

// SAFETY: SlabAllocator is fully thread-safe via lock-free algorithms
// and RwLock/Mutex where necessary.
unsafe impl Send for SlabAllocator {}
unsafe impl Sync for SlabAllocator {}

impl SlabAllocator {
    /// Create a new slab allocator.
    pub fn new() -> Self {
        let data_classes = DATA_SIZE_CLASSES
            .iter()
            .map(|&size| DataClassAllocator::new(size))
            .collect();

        Self {
            values: ValueAllocator::new(),
            data_classes,
            large_allocs: Mutex::new(Vec::new()),
            gc_threshold: Arc::new(AtomicUsize::new(MIN_GC_THRESHOLD)),
            committed_bytes_atomic: Arc::new(AtomicUsize::new(0)),
            alloc_count_atomic: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get the atomic committed bytes counter (for cron manager).
    #[inline]
    pub fn committed_bytes_atomic(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.committed_bytes_atomic)
    }

    /// Get the atomic allocation count counter (for cron manager).
    #[inline]
    pub fn alloc_count_atomic(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.alloc_count_atomic)
    }

    /// Get the atomic GC threshold counter (for cron manager).
    #[inline]
    pub fn gc_threshold_atomic(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.gc_threshold)
    }

    /// Atomically write the first 16 bytes (FreeNode header area) of a
    /// freshly-allocated slot, then non-atomically copy any remaining bytes.
    ///
    /// This prevents a data race with `TreiberStack::pop()`'s speculative
    /// `AtomicU128::load` of `FreeNode::next`: a concurrent `pop()` may still
    /// hold a stale `old_head` pointing at this slot. Its CAS will reject the
    /// stale value, but the speculative read must be paired with an atomic
    /// write to avoid undefined behavior (non-atomic write + atomic read on
    /// the same memory = data race = UB in the Rust/C++ memory model).
    ///
    /// # Invariant: push() and write_slot_bytes() must NEVER race on the same slot
    ///
    /// `AtomicU128::store` on x86_64 compiles to a CMPXCHG16B CAS loop (no
    /// native 128-bit store). If GC's `push()` and `write_slot_bytes()` both
    /// target the same slot, their CAS loops contend — `write_slot_bytes`
    /// retries until it wins, overwriting `push()`'s chain pointer and
    /// corrupting the free list. The quiescent GC protocol prevents this:
    /// GC only frees unreachable slots (during quiescence, no evaluators
    /// are allocating), so a slot being written by `write_slot_bytes` is
    /// always live and never freed concurrently.
    ///
    /// # Safety
    /// - `dst` must be a valid slab-allocated slot pointer (16-byte aligned)
    /// - `src` and `len` must describe a valid byte range
    /// - The slot must already be exclusively owned by this thread (popped
    ///   from the free list or bump-allocated)
    /// - GC must not free (push) this slot while it is being written
    #[inline]
    unsafe fn write_slot_bytes(dst: *mut u8, src: *const u8, len: usize) {
        let header_size = mem::size_of::<FreeNode>(); // 16
        if len >= header_size {
            // Read first 16 bytes from source, store atomically
            let first_chunk: u128 = ptr::read_unaligned(src as *const u128);
            (*(dst as *const FreeNode))
                .next
                .store(first_chunk, Ordering::Release);
            // Non-atomically copy remaining bytes
            let remaining = len - header_size;
            if remaining > 0 {
                ptr::copy_nonoverlapping(src.add(header_size), dst.add(header_size), remaining);
            }
        } else if len > 0 {
            // Source is < 16 bytes: zero-pad to 16, store atomically
            let mut buf = [0u8; 16];
            ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len);
            let chunk: u128 = u128::from_ne_bytes(buf);
            (*(dst as *const FreeNode))
                .next
                .store(chunk, Ordering::Release);
        } else {
            // len == 0: just atomically zero the header
            (*(dst as *const FreeNode)).next.store(0, Ordering::Release);
        }
    }

    /// Allocate an `MettaValueInner` and return a reference.
    ///
    /// Lock-free hot path. The slot is zero-initialized before writing.
    #[inline]
    pub fn alloc_value(&self, val: MettaValueInner) -> &'static MettaValueInner {
        let ptr = self.values.alloc();

        // Update atomic counters for cron manager
        self.alloc_count_atomic.fetch_add(1, Ordering::Relaxed);
        // Periodically update committed bytes (every 1024 allocs to avoid overhead)
        if self.alloc_count_atomic.load(Ordering::Relaxed) % 1024 == 0 {
            self.committed_bytes_atomic
                .store(self.committed_bytes(), Ordering::Relaxed);
            // NOTE: We intentionally do NOT call request_gc() here. GC must only
            // be triggered at safe points (the trampoline's maybe_gc()) where the
            // root set is complete. The cron manager's threshold/rate checks handle
            // setting GC_REQUESTED; the trampoline picks it up at the next safe point.
        }

        let result: &'static MettaValueInner = unsafe {
            let val_size = mem::size_of::<MettaValueInner>();
            let val_bytes = &val as *const MettaValueInner as *const u8;

            // Write value bytes with atomic header to prevent Treiber stack race
            Self::write_slot_bytes(ptr, val_bytes, val_size);

            // Zero any trailing slot padding beyond MettaValueInner
            if self.values.slot_size > val_size {
                ptr::write_bytes(ptr.add(val_size), 0, self.values.slot_size - val_size);
            }

            // We manually wrote the bytes — prevent drop of the original
            mem::forget(val);

            &*(ptr as *const MettaValueInner)
        };

        // Track in nursery for incremental GC (~3ns RefCell borrow overhead,
        // bounded by the slab alloc cost of 10-50ns).
        let slot_size = self.values.slot_size;
        crate::backend::eval::cesk::with_nursery_collector(|c| {
            c.record_alloc(ptr as usize, slot_size);
        });

        // Track in region stack for let* bulk deallocation.
        // Fast early-exit when no region is active.
        crate::backend::eval::cesk::with_region_stack(|s| {
            s.record_alloc();
        });

        result
    }

    /// Allocate a string slice, return a reference.
    #[inline]
    pub fn alloc_str(&self, s: &str) -> &'static str {
        if s.is_empty() {
            return "";
        }
        let bytes = s.as_bytes();
        let ptr = self.alloc_data(bytes.len());
        unsafe {
            // Atomic header write to prevent Treiber stack race
            Self::write_slot_bytes(ptr, bytes.as_ptr(), bytes.len());
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, bytes.len()))
        }
    }

    /// Allocate a Span in the slab, returning a `&'static Span`.
    ///
    /// The Span (48 bytes on 64-bit) is allocated via `alloc_data` and copied in.
    pub fn alloc_span(&self, span: crate::ir::Span) -> &'static crate::ir::Span {
        let size = mem::size_of::<crate::ir::Span>();
        let ptr = self.alloc_data(size);
        unsafe {
            // Atomic header write to prevent Treiber stack race
            let span_bytes = &span as *const crate::ir::Span as *const u8;
            Self::write_slot_bytes(ptr, span_bytes, size);
            // No mem::forget needed — Span is Copy (no drop glue)
            &*(ptr as *const crate::ir::Span)
        }
    }

    /// Allocate a slice of MettaValues from an iterator.
    pub fn alloc_slice_from_iter(
        &self,
        items: impl IntoIterator<Item = MettaValue>,
    ) -> &'static [MettaValue] {
        let items: Vec<MettaValue> = items.into_iter().collect();
        if items.is_empty() {
            return &[];
        }
        let len = items.len();
        let byte_len = len * mem::size_of::<MettaValue>();
        let ptr = self.alloc_data(byte_len);
        unsafe {
            // Atomic header write to prevent Treiber stack race.
            // MettaValue is Copy, so the Vec buffer is a contiguous byte range.
            let src = items.as_ptr() as *const u8;
            Self::write_slot_bytes(ptr, src, byte_len);
            std::slice::from_raw_parts(ptr as *const MettaValue, len)
        }
    }

    /// Allocate a slice of MettaValues from an existing slice (copy).
    pub fn alloc_slice_copy(&self, items: &[MettaValue]) -> &'static [MettaValue] {
        if items.is_empty() {
            return &[];
        }
        let byte_len = items.len() * mem::size_of::<MettaValue>();
        let ptr = self.alloc_data(byte_len);
        unsafe {
            // Atomic header write to prevent Treiber stack race
            let src = items.as_ptr() as *const u8;
            Self::write_slot_bytes(ptr, src, byte_len);
            std::slice::from_raw_parts(ptr as *const MettaValue, items.len())
        }
    }

    /// Allocate variable-length data. Selects the appropriate size class
    /// or falls back to system allocator for large data.
    ///
    /// Uses a two-tier allocation strategy for size-class allocations:
    /// 1. **Thread-local cache** — O(1), no CAS, no RwLock
    /// 2. **DataClassAllocator** — TreiberStack pop + page validation + bump alloc
    fn alloc_data(&self, size: usize) -> *mut u8 {
        if size == 0 {
            return SLOT_ALIGN as *mut u8;
        }
        // Find the smallest size class that fits
        for (i, &class_size) in DATA_SIZE_CLASSES.iter().enumerate() {
            if size <= class_size {
                // Tier 1: Thread-local data cache (O(1), no CAS)
                let cached = DATA_CACHES.try_with(|cell| {
                    let mut set = cell.get();
                    let cache = &mut set.caches[i];

                    // Check generation validity
                    if !cache.is_valid_generation() {
                        // Stale pointers may be in munmapped pages — silently discard.
                        // Cannot push back to global free list because push() writes
                        // FreeNode::next at the pointer's address, which would SIGSEGV
                        // if the page was munmapped by release_empty_pages().
                        cache.len = 0;
                        cache.sync_generation();
                    }

                    if let Some(ptr) = cache.pop() {
                        cell.set(set);
                        // Validate and increment live_count (same as DataClassAllocator::alloc)
                        let pages = self.data_classes[i].pages.read();
                        for page in pages.iter() {
                            if page.contains(ptr as *const u8, class_size) {
                                page.live_count.fetch_add(1, Ordering::Relaxed);
                                unsafe {
                                    asan_unpoison_slab_slot(ptr, class_size);
                                }
                                return Some(ptr);
                            }
                        }
                        // Stale pointer (page munmapped between cache and validation)
                        // — discard and fall through
                        return None;
                    }

                    // Cache empty — batch-refill from global TreiberStack
                    let mut batch_len = 0usize;
                    let mut batch: [*mut u8; DATA_CACHE_REFILL] =
                        [ptr::null_mut(); DATA_CACHE_REFILL];
                    for slot in batch.iter_mut() {
                        if let Some(ptr) = self.data_classes[i].free_list.pop() {
                            *slot = ptr;
                            batch_len += 1;
                        } else {
                            break;
                        }
                    }

                    if batch_len > 0 {
                        // Validate batch and cache survivors
                        let pages = self.data_classes[i].pages.read();
                        let mut first_ptr: Option<*mut u8> = None;

                        for idx in 0..batch_len {
                            let ptr = batch[idx];
                            let mut valid = false;
                            for page in pages.iter() {
                                if page.contains(ptr as *const u8, class_size) {
                                    valid = true;
                                    break;
                                }
                            }
                            if valid {
                                if first_ptr.is_none() {
                                    first_ptr = Some(ptr);
                                } else {
                                    let _ = cache.push(ptr);
                                }
                            }
                            // Invalid pointers silently discarded
                        }
                        drop(pages);

                        if let Some(ptr) = first_ptr {
                            cache.sync_generation();
                            cell.set(set);
                            // Complete allocation: validate page + increment live_count
                            let pages = self.data_classes[i].pages.read();
                            for page in pages.iter() {
                                if page.contains(ptr as *const u8, class_size) {
                                    page.live_count.fetch_add(1, Ordering::Relaxed);
                                    unsafe {
                                        asan_unpoison_slab_slot(ptr, class_size);
                                    }
                                    return Some(ptr);
                                }
                            }
                            // Extremely unlikely: page released between validation and here
                            return None;
                        }
                    }

                    cache.sync_generation();
                    cell.set(set);
                    None
                });

                // If thread-local access succeeded and returned a pointer, use it
                if let Ok(Some(ptr)) = cached {
                    return ptr;
                }

                // Tier 2: Fall through to DataClassAllocator (TreiberStack + bump)
                return self.data_classes[i].alloc();
            }
        }
        // Large allocation: use system allocator
        let layout =
            Layout::from_size_align(size, SLOT_ALIGN).expect("invalid layout for large allocation");
        let ptr = unsafe { std::alloc::alloc(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        self.large_allocs.lock().push((ptr, layout));
        ptr
    }

    /// Free a data slot.
    ///
    /// Uses thread-local data cache to avoid TreiberStack CAS on free.
    /// When the cache is full, flushes half to the global free list.
    fn free_data_slot(&self, ptr: *mut u8, size: usize) {
        if size == 0 {
            return;
        }
        for (i, &class_size) in DATA_SIZE_CLASSES.iter().enumerate() {
            if size <= class_size {
                // Decrement page live_count first (same as DataClassAllocator::free)
                {
                    let pages = self.data_classes[i].pages.read();
                    for page in pages.iter() {
                        if page.contains(ptr as *const u8, class_size) {
                            page.live_count.fetch_sub(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                // ASAN: poison the freed slot
                unsafe {
                    asan_poison_slab_slot(ptr, class_size);
                }

                // Try to cache in thread-local data cache
                let cached = DATA_CACHES.try_with(|cell| {
                    let mut set = cell.get();
                    let cache = &mut set.caches[i];

                    // Check generation: if stale, flush all before caching new pointer
                    if !cache.is_valid_generation() {
                        for j in 0..cache.len {
                            self.data_classes[i].free_list.push(cache.ptrs[j]);
                        }
                        cache.len = 0;
                        cache.sync_generation();
                    }

                    let full = cache.push(ptr);
                    if full {
                        // Flush half the cache to global free list (batch amortization)
                        let flush_count = cache.len / 2;
                        for j in 0..flush_count {
                            self.data_classes[i].free_list.push(cache.ptrs[j]);
                        }
                        // Compact: move remaining to front
                        let remaining = cache.len - flush_count;
                        for j in 0..remaining {
                            cache.ptrs[j] = cache.ptrs[flush_count + j];
                        }
                        cache.len = remaining;
                        // Now push the new pointer (guaranteed space)
                        let _ = cache.push(ptr);
                    }

                    cell.set(set);
                });

                // If thread-local cache not accessible (thread shutdown), fall through
                if cached.is_err() {
                    self.data_classes[i].free_list.push(ptr);
                }
                return;
            }
        }
        // Large allocation
        let mut large = self.large_allocs.lock();
        if let Some(pos) = large.iter().position(|(p, _)| *p == ptr) {
            let (ptr, layout) = large.swap_remove(pos);
            unsafe {
                std::alloc::dealloc(ptr, layout);
            }
        }
    }


    /// Total committed bytes (all pages).
    pub fn committed_bytes(&self) -> usize {
        let value_bytes = self.values.committed_bytes();
        let data_bytes: usize = self
            .data_classes
            .iter()
            .map(|dc| dc.committed_bytes())
            .sum();
        let large_bytes: usize = self.large_allocs.lock().iter().map(|(_, l)| l.size()).sum();
        value_bytes + data_bytes + large_bytes
    }

    /// Get the GC threshold.
    pub fn gc_threshold(&self) -> usize {
        self.gc_threshold.load(Ordering::Relaxed)
    }

    /// Set the GC threshold.
    pub fn set_gc_threshold(&self, threshold: usize) {
        self.gc_threshold.store(threshold, Ordering::Relaxed);
    }

    /// Get the value slot size.
    pub fn value_slot_size(&self) -> usize {
        self.values.slot_size
    }

    /// Get the current epoch counter.
    pub fn epoch(&self) -> u64 {
        self.values.epoch.load(Ordering::Acquire)
    }

    /// Get read access to value pages (for cron counter sync).
    ///
    /// Callers must hold the returned guard for the minimum necessary duration
    /// to avoid blocking page allocation (which needs a write lock).
    pub(crate) fn value_pages_read(&self) -> parking_lot::RwLockReadGuard<'_, Vec<Box<ValuePage>>> {
        self.values.pages.read()
    }

    /// Get the page generation counter for cache invalidation.
    ///
    /// Incremented when value pages are released (munmapped). Thread-local
    /// page caches compare against this to detect stale entries.
    pub fn page_generation(&self) -> u64 {
        CACHE_GENERATION.load(Ordering::Acquire)
    }

    /// Check if a value pointer is valid (not freed, within a known page).
    ///
    /// Returns `true` if `ptr`:
    /// 1. Falls within a known value page
    /// 2. Is within the bump-allocated range for that page
    /// 3. Has not been freed (epoch != u64::MAX sentinel)
    ///
    /// Used for diagnostic instrumentation (METTA_GC_TRACE mode) to detect
    /// dangling pointers before dereferencing them.
    pub fn is_value_ptr_valid(&self, ptr: *const u8) -> bool {
        let pages = self.values.pages.read();
        for page in pages.iter() {
            if let Some(idx) = page.slot_index(ptr, self.values.slot_size) {
                // Slot exists in a valid page — check if it's been freed
                return page.slot_epoch(idx) != u64::MAX;
            }
        }
        false
    }

    /// Check if a slot was re-allocated after a given epoch.
    pub fn is_realloc_after_epoch(&self, ptr: *const u8, snapshot_epoch: u64) -> bool {
        let pages = self.values.pages.read();
        for page in pages.iter() {
            if let Some(idx) = page.slot_index(ptr, self.values.slot_size) {
                return page.slot_epoch(idx) > snapshot_epoch;
            }
        }
        false
    }

    /// Read the allocation epoch for the slab slot at `ptr`.
    /// Returns `Some(epoch)` if the pointer is in a known page, `None` otherwise.
    /// A freed slot has epoch `u64::MAX`.
    pub fn get_slot_epoch(&self, ptr: *const u8) -> Option<u64> {
        let pages = self.values.pages.read();
        for page in pages.iter() {
            if let Some(idx) = page.slot_index(ptr, self.values.slot_size) {
                return Some(page.slot_epoch(idx));
            }
        }
        None
    }

    /// Free a value slot (lock-free).
    ///
    /// # Safety
    /// The caller must ensure the pointer was allocated by this allocator.
    pub unsafe fn free_value(&self, ptr: *mut u8) {
        self.values.free(ptr);
    }

    /// Free a data slot.
    ///
    /// # Safety
    /// The caller must ensure the pointer was allocated by this allocator.
    pub unsafe fn free_data(&self, ptr: *mut u8, size: usize) {
        self.free_data_slot(ptr, size);
    }

    /// Check if a value pointer belongs to this allocator.
    pub fn contains_value(&self, ptr: *const u8) -> bool {
        self.values.contains(ptr)
    }

    /// Get page-level allocation statistics for diagnostics.
    pub fn page_stats(&self) -> PageStats {
        let pages = self.values.pages.read();
        let page_count = pages.len();
        let slots_per_page = PAGE_SIZE / self.values.slot_size;
        let mut total_bumped = 0usize;
        let mut total_live = 0usize;

        for page in pages.iter() {
            total_bumped += page.bump_count.load(Ordering::Relaxed);
            total_live += page.live_count.load(Ordering::Relaxed) as usize;
        }

        let mut data_pages = 0usize;
        let mut data_committed = 0usize;
        for dc in &self.data_classes {
            let dp = dc.pages.read();
            data_pages += dp.len();
            data_committed += dc.committed_bytes();
        }

        PageStats {
            value_page_count: page_count,
            slots_per_page,
            total_bumped_slots: total_bumped,
            total_live_slots: total_live,
            value_committed_bytes: self.values.committed_bytes(),
            data_page_count: data_pages,
            data_committed_bytes: data_committed,
        }
    }
}

/// Page-level allocation statistics.
pub struct PageStats {
    pub value_page_count: usize,
    pub slots_per_page: usize,
    pub total_bumped_slots: usize,
    pub total_live_slots: usize,
    pub value_committed_bytes: usize,
    pub data_page_count: usize,
    pub data_committed_bytes: usize,
}

impl Default for SlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SlabAllocator {
    fn drop(&mut self) {
        // Free large allocations
        let mut large = self.large_allocs.lock();
        for (ptr, layout) in large.drain(..) {
            unsafe {
                std::alloc::dealloc(ptr, layout);
            }
        }
        // MmapPages are dropped automatically via ValuePage/DataPage drop
    }
}

impl std::fmt::Debug for SlabAllocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pages = self.values.pages.read();
        f.debug_struct("SlabAllocator")
            .field("value_pages", &pages.len())
            .field("value_slot_size", &self.values.slot_size)
            .field("committed_bytes", &self.committed_bytes())
            .field("gc_threshold", &self.gc_threshold.load(Ordering::Relaxed))
            .finish()
    }
}

// ============================================================================
// Global Singleton
// ============================================================================

static GLOBAL_ALLOCATOR: OnceLock<SlabAllocator> = OnceLock::new();

/// Initialize the global allocator (call once at startup). Idempotent.
///
/// The GC cron manager is spawned lazily on the first call to `global_gc_cron()`,
/// which happens when `maybe_process_gc_response()` is first called from the trampoline.
pub fn init_global_allocator() {
    GLOBAL_ALLOCATOR.get_or_init(SlabAllocator::new);
}

/// Get the global allocator. Auto-initializes on first call.
///
/// Also installs signal-triggered diagnostic handlers (SIGTERM/SIGUSR1)
/// on first call, so every code path (binary, tests, benchmarks) gets
/// coverage without explicit setup.
pub fn global_allocator() -> &'static SlabAllocator {
    let alloc = GLOBAL_ALLOCATOR.get_or_init(SlabAllocator::new);
    crate::backend::diagnostics::install_signal_handlers();
    alloc
}

/// Get the ACTIVE value factory: the index-arena store's `IndexFactory`. The
/// accessor returns the active store's alloc interface, so every one of the
/// ~194 `global_factory()` callers follows the index store without per-site
/// changes. It is NOT a runtime mode-dispatch inside the factory.
pub fn global_factory() -> crate::backend::models::ActiveFactory {
    crate::backend::eval::cesk::index_heap::IndexFactory
}

// ============================================================================
// Global GC Thread + Coordination
// ============================================================================

// Legacy global GC executors (GcThread, then the adaptive worker pool) have been
// removed (F4 rungs R1/R3); see git history.

/// Flag set by the cron manager or allocation pressure to request a GC cycle.
/// Checked by `maybe_gc()` in the trampoline loop (every 256 iterations).
/// `pub(crate)` for test observability (clearing between tests).
pub(crate) static GC_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Monotonic counter incremented on every `request_gc()` call.
///
/// Unlike the transient `GC_REQUESTED` flag — which post Phase 9 may be
/// consumed nanoseconds later by `maybe_async_gc()` running on the same
/// cron-thread tick — this counter is sticky and append-only. It records
/// the number of times the system decided GC was needed and reflects the
/// true intent of "GC was requested" regardless of which path subsequently
/// consumed the flag.
///
/// Used by `test_gc_requested_on_high_alloc_rate` (and external telemetry)
/// to observe request events without racing on flag consumption.
static GC_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Request a GC cycle. Sets the `gc_requested` flag which will be picked up
/// by the next `maybe_gc()` call from the trampoline loop. Also increments
/// `GC_REQUESTS_TOTAL` so callers can observe the request event even if the
/// transient flag is consumed by `maybe_async_gc()` on the same thread.
pub fn request_gc() {
    GC_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    GC_REQUESTED.store(true, Ordering::Release);
}

/// Check whether a GC cycle has been requested (test observability).
pub fn is_gc_requested() -> bool {
    GC_REQUESTED.load(Ordering::Acquire)
}

/// Get the total number of `request_gc()` calls since process start.
///
/// Monotonic and sticky: each call to `request_gc()` increments this once,
/// and it never decreases. This is the canonical observable for "did the
/// system decide GC was needed?" — the `is_gc_requested()` flag is transient
/// (post Phase 9 it is consumed by `maybe_async_gc()` immediately after
/// being set by the cron monitor) and unsuitable for cross-thread polling.
#[inline]
pub fn gc_requests_total() -> u64 {
    GC_REQUESTS_TOTAL.load(Ordering::Relaxed)
}

// ============================================================================
// Global GC Cron Manager
// ============================================================================

/// Global GC cron manager singleton. Lazily spawned on first use.
/// No `Mutex` needed — `CronHandle` is `Clone + Send`.
static GLOBAL_GC_CRON: OnceLock<super::gc_cron::GcCronSingleton> = OnceLock::new();

/// Get the global GC cron manager, spawning it if needed.
///
/// Wires the global allocator's atomic counters (`committed_bytes_atomic`,
/// `alloc_count_atomic`, `gc_threshold`) into the cron manager.
pub fn global_gc_cron() -> &'static super::gc_cron::GcCronSingleton {
    GLOBAL_GC_CRON.get_or_init(|| {
        let alloc = global_allocator();
        super::gc_cron::spawn_gc_cron(
            alloc.committed_bytes_atomic(),
            alloc.alloc_count_atomic(),
            alloc.gc_threshold_atomic(),
        )
    })
}

// ============================================================================
// GC Runtime Control — Quiescent-State Collection
// ============================================================================
//
// GC is always enabled by default. The `--no-gc` CLI flag disables it via
// `disable_gc()`. The quiescent-state protocol ensures GC only triggers when
// no eval threads are active (trampoline stacks are empty), so environment
// roots + MettaState source/output roots form a complete root set.

/// Whether GC collection is disabled at runtime (via `--no-gc` CLI flag).
/// Defaults to false (GC enabled). Set once at startup.
static GC_DISABLED: AtomicBool = AtomicBool::new(false);

/// Disable GC collection (called from CLI `--no-gc` flag).
pub fn disable_gc() {
    GC_DISABLED.store(true, Ordering::Release);
}

/// Check if GC is disabled.
pub fn is_gc_disabled() -> bool {
    GC_DISABLED.load(Ordering::Acquire)
}

// ============================================================================
// Session-Based GC — Context ID Infrastructure
// ============================================================================
//
// Each top-level eval gets a unique context ID. All allocations during that
// eval are tagged with the ID. On eval completion, an RAII guard triggers
// bulk release of the session's values (minus anything referenced by roots).
//
// Context 0 = persistent (compile-time values, never released by sessions).
// Other values are monotonically increasing session IDs.

/// Global monotonic counter for session context IDs.
/// Context 0 is reserved for persistent (non-session) allocations.
static NEXT_CONTEXT_ID: AtomicU32 = AtomicU32::new(1);

thread_local! {
    /// Per-thread current session context ID.
    /// 0 = no active session (allocations are persistent).
    static THREAD_CONTEXT_ID: Cell<u32> = const { Cell::new(0) };
}

/// Get the current thread's session context ID.
///
/// Returns 0 if no `SessionGuard` is active (persistent allocation).
/// Cost: thread-local read (~1ns), no atomic operations.
#[inline]
pub fn current_context_id() -> u32 {
    THREAD_CONTEXT_ID.with(|c| c.get())
}

/// RAII guard for session-based GC. Each top-level eval creates one.
///
/// On creation: allocates a unique context ID and sets the thread-local.
/// On drop: clears the thread-local and enqueues an async release of the
/// session's values (minus survivors). The actual collection runs on a
/// dedicated background thread, not on the eval thread.
///
/// The guard must be held until results have been formatted/consumed,
/// because values are freed asynchronously after drop.
pub struct SessionGuard {
    context_id: u32,
}

impl SessionGuard {
    /// Enter a new session — allocates a unique context ID.
    ///
    /// All values allocated while this guard is alive will be tagged with
    /// the session's context ID and eligible for bulk release on drop.
    ///
    /// Context ID 0 is reserved as the "persistent" sentinel (values that
    /// are never released by session-based GC). On wrap-around (~4.29B
    /// sessions), ID 0 is skipped to avoid accidentally marking session
    /// allocations as persistent.
    pub fn enter() -> Self {
        let mut id = NEXT_CONTEXT_ID.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            // Wrapped around; skip 0 (persistent sentinel)
            id = NEXT_CONTEXT_ID.fetch_add(1, Ordering::Relaxed);
        }
        THREAD_CONTEXT_ID.with(|c| c.set(id));
        SessionGuard { context_id: id }
    }

    /// Get this session's context ID.
    #[inline]
    pub fn context_id(&self) -> u32 {
        self.context_id
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        // Clear thread-local so subsequent allocations are persistent (ctx=0)
        THREAD_CONTEXT_ID.with(|c| c.set(0));
        // Enqueue async release — the background session release thread handles
        // root tracing and sweeping. Cost: one channel send (~50ns).
        if !is_gc_disabled() {
            enqueue_session_release(self.context_id);
        }
    }
}

// ============================================================================
// Async Session Release Thread
// ============================================================================
//
// A dedicated background thread processes session releases asynchronously.
// The eval thread only pays the cost of a channel send (~50ns) per session,
// not the full root trace + sweep.
//
// The thread is lazily spawned on the first session release request.

/// Enqueue a session context ID for async release via the adaptive GC pool.
///
/// Cost: one channel send (~50ns). A GC pool worker performs the actual
/// quiescence waiting, root tracing, and sweep.
fn enqueue_session_release(context_id: u32) {
    if context_id == 0 {
        return; // Never release persistent values
    }

    // F4 R3: under index-gc, session release is handled by the CESK/index
    // regime; there is no legacy slab pool to submit work to.
}

/// Release all values allocated during a session, except those reachable from roots.
///
/// Public free function for use by external callers. Enqueues an async release
/// on the adaptive GC pool's LOW priority channel.
pub fn release_session(context_id: u32) {
    enqueue_session_release(context_id);
}

// ============================================================================
// Quiescent-State Coordination — EvalGuard + ACTIVE_EVALUATORS
// ============================================================================
//
// The quiescent-state protocol ensures GC snapshots are only built when no
// eval threads are active. This avoids scanning trampoline Rust call stacks
// (work_stack, continuations, locals) which are NOT registered as GC roots.
//
// Protocol:
// 1. Each eval() wraps its body in EvalGuard::enter() / drop
// 2. EvalGuard::enter() increments ACTIVE_EVALUATORS, checks GC_IN_PROGRESS
// 3. maybe_quiescent_gc() checks ACTIVE_EVALUATORS == 0 before building snapshot
// 4. GC_IN_PROGRESS prevents new evals from starting during snapshot building
// 5. GC_CYCLE_IN_FLIGHT prevents queueing multiple snapshots in the GC channel
//    (set on snapshot send, cleared on response processing)
//
// This is NOT stop-the-world: only snapshot capture (sub-millisecond) is
// synchronous. The actual mark-sweep runs asynchronously on the GC thread.

/// Number of concurrently active eval() / eval_trampoline() calls.
/// GC triggers ONLY when this reaches 0 (quiescent state).
pub(super) static ACTIVE_EVALUATORS: AtomicU32 = AtomicU32::new(0);

/// Number of distinct mutator THREADS currently inside ≥1 `EvalGuard` (i.e. with
/// thread-local `EVAL_GUARD_DEPTH > 0`). Unlike [`ACTIVE_EVALUATORS`] — a SUM of
/// guard *increments*, so a depth-`k` thread contributes `k` — `N_THREADS` counts
/// each active thread EXACTLY ONCE: bumped only on the outermost enter (depth
/// 0→1) and dropped only on the outermost leave (depth 1→0). The Phase D+E
/// dedicated-GC-thread driver gates its rendezvous on
/// `WORKERS_PARKED_FOR_GC == n_threads()` (a per-THREAD parked count), NOT on
/// `active_evaluator_count()`, so a nested (depth>1) mutator parks once and is
/// counted once. See docs/cesk-gc/phase-de-concurrent-collector-design.md §1.2.
pub(super) static N_THREADS: AtomicU32 = AtomicU32::new(0);

/// Sticky process-global flag: `true` once ANY eval worker has EVER been spawned
/// (set at the `parallel_dispatch` / `parallel_collapse_dispatch` spawn sites).
///
/// This is the provable-safety gate for the Inc-6 single-threaded index GC. If
/// no eval worker has ever been spawned, then no parked-resumable worker can
/// exist, so the single calling thread at a between-steps safepoint is provably
/// the SOLE thread that can touch the index store σ — the trivially-true
/// instance of the TLA+-proven `QuiescenceInvariant`, requiring no
/// admission-gate / rendezvous. Once a worker has been spawned the flag latches
/// `true` forever and the single-threaded collector backs off entirely (the
/// parallel-rendezvous collector is a separate increment).
static WORKER_EVER_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Set by `maybe_quiescent_gc()` during snapshot building (sub-millisecond).
/// `EvalGuard::enter()` parks on condvar until this is false.
/// NOT stop-the-world: only guards the brief snapshot capture, not mark-sweep.
pub(super) static GC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Check if GC is currently in progress (snapshot being built or response being processed).
///
/// Used by the cron counter-sync task to skip cycles during GC to avoid
/// reading slot content that Phase 3 may be concurrently freeing/poisoning.
#[inline]
pub fn is_gc_in_progress() -> bool {
    GC_IN_PROGRESS.load(Ordering::Acquire)
}

/// Mutex + Condvar pair for parking evaluator threads while GC snapshot is in
/// progress. The mutex protects against lost wakeups: `GcInProgressGuard::drop()`
/// holds the mutex when clearing `GC_IN_PROGRESS`, ensuring threads that checked
/// the flag and are about to `wait()` cannot miss the notification.
pub(super) static GC_PROGRESS_MUTEX: Mutex<()> = Mutex::new(());
pub(super) static GC_PROGRESS_CONDVAR: Condvar = Condvar::new();

/// Mutex + Condvar pair for notifying threads waiting for quiescent state
/// (ACTIVE_EVALUATORS == 0). EvalGuard::drop() notifies when transitioning
/// from 1→0. Used by the GC workers to wait for safe root tracing points.
pub(super) static QUIESCENT_MUTEX: Mutex<()> = Mutex::new(());
pub(super) static QUIESCENT_CONDVAR: Condvar = Condvar::new();

// ============================================================================
// Phase D — Parallel-Collector Cooperative Rendezvous (D1.1 state + primitives)
// ============================================================================
//
// DEAD CODE until D2.x wires the call sites. These `pub(crate)` items implement
// the cooperative stop-the-world rendezvous that lets the index collector run
// WHILE FANOUT>0 eval workers are alive (today the index collector is gated OFF
// the moment a worker spawns — `!worker_ever_spawned()` in `index_heap.rs` —
// so under parallel eval the heap grows uncollected until quiescence). The full
// protocol, the 4-role happens-before chain, the lost-wakeup avoidance, and the
// sub-increment spec are documented in
// `docs/cesk-gc/phase-d-d1-d2-rendezvous-design.md` (§D1).
//
// PROTOCOL (one collector at a time; workers SELF-COLLECT their structural roots
// because the requestor cannot read a parked worker's thread-local registers —
// see the design doc's "Genuine-CESK crux"):
//
//   Requestor: begin_gc_rendezvous() [CAS exclusion] → request_gc() [Release] →
//     self-root → drop_eval_guard_for_safepoint() → requestor_wait_for_parked()
//     [waits active==0, the proven TLA+ `BeginMark` predicate] → drain ∪ E₀ ∪
//     driver-C → mark_sweep_if_over_watermark(.write()) → GC_REQUESTED=false
//     [Release] → resume_workers() → end_gc_rendezvous() →
//     reacquire_eval_guard_after_safepoint().
//
//   Worker (at a poll point): is_gc_requested() [Acquire] ⇒ self-root into the
//     shared buffer, drop its EvalGuard, then `worker_park_and_root(roots)`
//     (append + count + notify-requestor + park until !is_gc_requested), then
//     reacquire its EvalGuard. (The eval_loop caller owns the drop/reacquire of
//     the EvalGuard around the call; `worker_park_and_root` itself is JUST the
//     buffer-append + counter-bump + notify + park, so it is testable in
//     isolation — see the D1.1 unit test.)
//
// HAPPENS-BEFORE (design doc §D1 "Happens-before"):
//   HB1  request_gc() Release  →  worker is_gc_requested() Acquire (request seen)
//   HB2  worker buffer-append + `WORKERS_PARKED_FOR_GC.fetch_add(AcqRel)` (the
//        AcqRel acts as the buffer-append release fence) + RENDEZVOUS_MUTEX held
//        across {parked-count predicate, notify_all}  →  requestor observes ALL
//        buffer writes before it drains+marks.
//   HB3  the collector's `.write()` lock  →  excludes any allocator `.read()`
//        during mark (D-RLOCK); parked workers hold NO `.read()` (they park
//        between allocations).
//   HB4  `GC_REQUESTED.store(false)` Release  →  worker resume Acquire sees the
//        post-sweep store state.
//
// LOST-WAKEUP AVOIDANCE (copied from `EvalGuard::enter` :2920-2941): on BOTH the
// RENDEZVOUS side (requestor waits parked) and the RESUME side (worker waits
// resume), the mutex is held across BOTH the predicate observation and the
// matching `notify_all`, so a thread that re-checks the predicate and is about
// to `wait` cannot miss the notification. The 5 s `wait_for` + warn-retry is a
// liveness backstop (NOT a hard budget — that was the deleted
// `safepoint_wait_for_quiescence`'s pathology), bounding R1 (a worker that never
// reaches a safepoint) to a warn-and-retry rather than a deadlock.
//
// SEPARATE condvar pairs (Risk R4): `RENDEZVOUS_*` / `RESUME_*` are NEW and
// distinct from the slab `GC_PROGRESS_*` / `QUIESCENT_*` pairs — overloading
// them would risk cross-wakeups between the slab snapshot protocol and the index
// rendezvous. They are loom-verified on the new pairs (`loom_rendezvous`).

/// Number of eval workers currently parked at a rendezvous safepoint (having
/// self-rooted into [`WORKER_ROOT_BUFFER`]). It is BOTH the buffer happens-before
/// carrier (the `fetch_add(AcqRel)` is the release fence for the worker's prior
/// buffer-append — HB2) AND an observability aid. The PRIMARY termination gate
/// for the requestor is [`active_evaluator_count`]`()==0` (the TLA+-proven
/// `BeginMark` predicate), not this counter; this counter exists so the protocol
/// can be reasoned about / asserted directly. `pub(crate)` for test/loom parity.
///
/// Source-coupled by the dedicated rendezvous paths and retained for focused
/// rendezvous tests.
#[allow(dead_code)]
pub(crate) static WORKERS_PARKED_FOR_GC: AtomicU32 = AtomicU32::new(0);

/// Monotonic GC cycle generation, bumped at the END of each rendezvous cycle (by
/// the dedicated-GC-thread driver, under [`RENDEZVOUS_MUTEX`], together with the
/// parked-count/buffer reset). A parking worker captures `my_gen` at park time;
/// its resume-wait ([`worker_resume_wait_for_cycle`]) releases when `GC_CYCLE_GEN
/// != my_gen` (the cycle it parked for ended) — robust to the boolean
/// `GC_REQUESTED` being re-set by an UNRELATED back-to-back trigger (≥10 non-driver
/// callers set it), which a boolean-gated resume could not distinguish (Round-4
/// fix F2). Also the straggler-exclusion key in [`worker_park_and_root_in_cycle`]:
/// a parker whose `gen != my_gen` drops its stale roots and does NOT bump the
/// parked-count, so a late finisher from cycle K cannot corrupt cycle K+1's gate.
///
/// **Starts at 1, NOT 0 (E1-FLIP Path B V4 requirement).** The V4 witness uses
/// `published_gen = acquired_gen - 1` as the "not-yet-published-this-cycle" sentinel
/// (`witness_acquire_slot`). With unsigned wraparound, `acquired_gen == 0` would set
/// `published_gen = u64::MAX`, which the strict-`>` predicate (`published >= cur_gen`)
/// would read as SATISFIED for `cur_gen == 0` — a freshly-acquired, unpublished slot
/// would spuriously pass the gate on the VERY FIRST rendezvous (gen 0), reopening the
/// publish-timing UAF on cycle #1. Because the driver bumps the gen only at cycle END,
/// cycle #1 runs at the initial value; starting at 1 guarantees `cur_gen >= 1` for
/// every rendezvous, so `published = acquired - 1` never wraps and the red-teamed
/// strict-`>` algebra is exactly correct for ALL cycles. The existing gen-gated
/// primitives compare a CAPTURED `my_gen` for equality and only ever `fetch_add`, so
/// they are agnostic to the initial value (no code depends on it being 0).
#[allow(dead_code)]
pub(crate) static GC_CYCLE_GEN: AtomicU64 = AtomicU64::new(1);

/// Current GC cycle generation (Acquire). See [`GC_CYCLE_GEN`].
#[allow(dead_code)]
pub(crate) fn current_cycle_gen() -> u64 {
    GC_CYCLE_GEN.load(Ordering::Acquire)
}

/// E5 (the straddle-deadlock fix, `docs/cesk-gc/e1-flip-deadlock-straddle-rootcause-2026-06-03.md`):
/// the generation of a cycle that a LIVE driver has ACTUALLY STARTED (committed to),
/// as distinct from [`GC_CYCLE_GEN`] (which is bumped only at cycle END and thus, in
/// the few-instruction teardown window between `end_rendezvous_cycle`'s gen-bump and
/// the `GC_IN_PROGRESS` clear, already names the NEXT — not-yet-started — cycle).
///
/// **Premise (load-bearing):** there is NO start-of-cycle gen bump. The driver READS
/// `cur_gen = current_cycle_gen()` at its prologue (gc_driver.rs:~197) and the witness
/// predicate keys on it; `GC_CYCLE_STARTED := that cur_gen`. So during cycle K it equals
/// `GC_CYCLE_GEN` (both K), and it is always `<= GC_CYCLE_GEN`. In the teardown window
/// it is `started = K < gen = K+1` — which is EXACTLY why the straddle re-park, gated on
/// `started > my_reparked_gen`, does NOT phantom-re-park there (`started=K ≯ my=K`).
///
/// **MUST be lock-free Release/Acquire — NEVER read/stored under [`RENDEZVOUS_MUTEX`].**
/// The straddle body calls `worker_park_and_root_in_cycle` → `RENDEZVOUS_MUTEX.lock()`
/// (non-reentrant), so any mutex-guarded `started` read in the straddle self-deadlocks
/// 100% (a previously-rejected round). The happens-before that makes a Release store at
/// the driver prologue visible to the straddle's Acquire read comes from the gip-CAS
/// (`GcInProgressGuard::try_enter`, AcqRel) being ordered-before the prologue store; the
/// driver `debug_assert!(gc_in_progress())`s right before the store to pin that order.
///
/// **Starts at 0** (not 1): before ANY driver has run, no cycle is started, and the
/// init value must be `< GC_CYCLE_GEN`'s init (1) so a worker that somehow reaches the
/// straddle before the first rendezvous never re-parks for a phantom "started" cycle.
///
/// Source-coupled by the E5 straddle gate and dedicated driver prologue.
#[allow(dead_code)]
pub(crate) static GC_CYCLE_STARTED: AtomicU64 = AtomicU64::new(0);

/// E5: generation of the cycle a live driver has ACTUALLY STARTED (Acquire). See
/// [`GC_CYCLE_STARTED`]. Read in the straddle re-park loop's `started`-gate (lock-free).
#[allow(dead_code)]
#[inline]
pub(crate) fn current_cycle_started() -> u64 {
    GC_CYCLE_STARTED.load(Ordering::Acquire)
}

/// E5: publish that the driver has STARTED cycle `g` (Release), called ONLY at the
/// driver prologue AFTER `try_enter` closed admission (so the gip-CAS AcqRel is
/// ordered-before this store — the HB that carries `g` to the straddle's Acquire read).
/// ALSO notifies [`GC_PROGRESS_CONDVAR`] (under [`GC_PROGRESS_MUTEX`]) to wake any
/// worker parked in the straddle's teardown-window else-arm (which waits on that
/// condvar for `!gc_in_progress() || started>my`) — without it that worker would sleep
/// to its 5 s `wait_for` timeout when the NEXT cycle starts. Lock-free w.r.t. the
/// `started` value itself; the GC_PROGRESS_MUTEX is taken ONLY to make the notify
/// lost-wakeup-safe (Mesa discipline — the else-arm re-checks `started`/`gip` under the
/// same mutex). MUST NOT be called under [`RENDEZVOUS_MUTEX`] (see [`GC_CYCLE_STARTED`]).
#[allow(dead_code)]
#[inline]
pub(crate) fn set_current_cycle_started(g: u64) {
    GC_CYCLE_STARTED.store(g, Ordering::Release);
    // Wake the straddle teardown-window else-arm waiters (lost-wakeup-safe: the
    // else-arm holds GC_PROGRESS_MUTEX across its predicate re-check + wait). A
    // spurious wake is absorbed by that re-check; it can never cause incorrectness.
    {
        let _lock = GC_PROGRESS_MUTEX.lock();
        GC_PROGRESS_CONDVAR.notify_all();
    }
}

/// E1-FLIP: the `n` the dedicated GC thread snapshotted for THIS cycle (set in
/// `gc_driver_rendezvous_cycle` AFTER admission closed [`GcInProgressGuard`], BEFORE
/// `requestor_wait_for_parked_count`). Read by `gate_open_rendezvous` so the
/// rendezvous-collect completeness gate observes the SAME `n` the parked-count was
/// waited against — the witness that every snapshot participant has self-rooted into
/// `WORKER_ROOT_BUFFER` (HB2) before the sweep runs.
///
/// Source-coupled to the dedicated driver root-preparation path and
/// rendezvous gate diagnostics.
#[allow(dead_code)]
pub(crate) static N_THREADS_AT_SNAPSHOT: AtomicU32 = AtomicU32::new(0);

/// E1-FLIP: read the snapshot `n` for the in-flight rendezvous cycle.
#[allow(dead_code)]
#[inline]
pub(crate) fn n_threads_at_snapshot() -> u32 {
    N_THREADS_AT_SNAPSHOT.load(Ordering::Acquire)
}

/// E1-FLIP: publish the snapshot `n` (driver, post-admission, pre-wait).
#[allow(dead_code)]
#[inline]
pub(crate) fn set_n_threads_at_snapshot(n: u32) {
    N_THREADS_AT_SNAPSHOT.store(n, Ordering::Release);
}

/// E1-FLIP: parked-count reader for `gate_open_rendezvous` (Acquire — pairs with the
/// parkers'/finishers' AcqRel `fetch_add`, HB2).
#[allow(dead_code)]
#[inline]
pub(crate) fn workers_parked_for_gc() -> u32 {
    WORKERS_PARKED_FOR_GC.load(Ordering::Acquire)
}

/// E1-FLIP: GC-in-progress reader for `gate_open_rendezvous` (Acquire). True ⇒ the
/// dedicated GC thread holds the rendezvous (admission closed).
#[allow(dead_code)]
#[inline]
pub(crate) fn gc_in_progress() -> bool {
    GC_IN_PROGRESS.load(Ordering::Acquire)
}

thread_local! {
    /// E1-FLIP §Part 3: the `GC_CYCLE_GEN` this thread has ALREADY balanced
    /// (parked-count-bumped) for. `None` ⇒ not yet bumped this cycle. Prevents the
    /// `EvalGuard::drop` panic/cancel finish-bump from DOUBLE-counting a worker that
    /// already bumped via the normal finisher (`worker_finish_into_buffer`) or a park
    /// (`worker_park_and_root_in_cycle`). Self-invalidates across cycles: the stored
    /// `Some(old_gen)` no longer `== Some(current_gen)` once the gen advances, so no
    /// explicit per-cycle clear is needed.
    static GC_CYCLE_BUMPED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// E1-FLIP §Part 3: record that THIS thread has balanced the parked-count for cycle
/// `gen` (idempotent per cycle). Called by every bump site that actually bumped.
#[allow(dead_code)]
#[inline]
pub(crate) fn note_cycle_bumped(gen: u64) {
    GC_CYCLE_BUMPED.with(|c| c.set(Some(gen)));
}

/// E1-FLIP §Part 3: has THIS thread already balanced the parked-count for `gen`?
#[allow(dead_code)]
#[inline]
pub(crate) fn already_bumped_this_cycle(gen: u64) -> bool {
    GC_CYCLE_BUMPED.with(|c| c.get() == Some(gen))
}

// ============================================================================
// E1-FLIP Path B V4 — the WITNESS directory (reified-park-only, witness-SOLE-gate)
// ============================================================================
//
// THE FIX for the publish-timing UAF (docs/cesk-gc/e1-flip-VALIDATION-FAILED-2026-06-02.md):
// the fungible parked-count gate (`WORKERS_PARKED_FOR_GC >= n`) lets the driver proceed
// while a COUNTED participant has not yet published its machine (a finisher's bump "covers"
// for the parent's not-yet-published `work_stack`). The witness replaces the fungible count
// with a PER-SLOT predicate: the sweep runs ONLY when every OCCUPIED slot was STAMPED this
// cycle by a genuine reified park (`note_reified_park`, the SOLE published-setter).
//
// V4 slot lifecycle (the crux; docs/cesk-gc/e1-flip-pathB-v2-impl.md §"V4 — the slot
// lifecycle"): a slot is OCCUPIED ⟺ the thread holds an unpublished-this-cycle LIVE machine
// — from `EvalGuard::enter` (prev==0) continuously to the OUTERMOST `EvalGuard::drop`
// (depth==1), INCLUDING across every park. DECOUPLED from `N_THREADS` (which releases at a
// park; a parked thread is not "active"). Intra-slot atomic order: write `acquired`(Release)
// → `published`(Release) → `occupied=true`(Release LAST); read `occupied`(Acq) →
// `acquired`(Acq) → `published`(Acq). TWO `AtomicU64` (not packed). The driver gate is the
// strict-`>` predicate `published>=cur_gen OR acquired>cur_gen` over a LIVE-RE-WALK of the
// never-realloc chunk list (A-straddle-2: a value-snapshot would miss a slot re-occupied
// AFTER the snapshot instant → sweep-without-waiting → UAF).
//
// `dedicated_gc_enabled()` gates at the wiring sites keep the dormant path
// byte-identical while making the index dedicated driver the default.

/// One witness slot — owned by exactly one mutator thread for its lifetime (the
/// thread-local [`MY_WITNESS_SLOT`] points at it; slots are grow-only and never
/// reused across threads, so there is no slot-ABA). `acquired_gen`/`published_gen`
/// are the two un-packed `AtomicU64` (rt2 RT-3); `occupied` is the `AtomicBool`
/// that gates whether this slot participates in a snapshot.
///
/// Source-coupled by the V4 witness-slot lifecycle.
#[allow(dead_code)]
pub(crate) struct WitnessSlot {
    /// The `GC_CYCLE_GEN` this slot's owner most-recently ACQUIRED/RESTAMPED for.
    /// `acquired > cur_gen` ⇒ a post-snapshot entrant (excluded from the wait).
    acquired_gen: AtomicU64,
    /// The `GC_CYCLE_GEN` this slot's owner has PUBLISHED a complete machine for
    /// (the SOLE setter is [`note_reified_park`]). `published >= cur_gen` ⇒ this
    /// participant's machine is in `cur_gen`'s `WORKER_ROOT_BUFFER` (or B3's
    /// SAFEPOINT_ROOTS) — the driver may mark it.
    published_gen: AtomicU64,
    /// `true` ⟺ the owning thread holds an unpublished-this-cycle live machine
    /// (enter→outermost-drop, across parks). Snapshot collects ONLY occupied slots.
    occupied: AtomicBool,
}

/// A grow-only never-realloc chunk of [`WitnessSlot`]s. New chunks are appended via
/// `next` (an `AtomicPtr`) so a slot POINTER handed out earlier stays valid forever
/// (the live-re-walk + [`MY_WITNESS_SLOT`] rely on this). 256 slots/chunk amortizes
/// allocation; a chunk is `Box::leak`ed (lives for the process).
///
/// Grow-only backing storage for the V4 witness directory.
#[allow(dead_code)]
struct WitnessChunk {
    slots: [WitnessSlot; 256],
    /// Next chunk in the grow-only list (null = end). Published Release on growth,
    /// read Acquire on walk.
    next: AtomicPtr<WitnessChunk>,
}

#[allow(dead_code)]
impl WitnessChunk {
    /// Allocate a fresh all-zero chunk and leak it (process-lifetime). `acquired`/
    /// `published` start 0, `occupied` false — a free slot.
    fn new_leaked() -> *mut WitnessChunk {
        // Build 256 zeroed slots. `from_fn` avoids needing `Copy` on the atomics.
        let slots = std::array::from_fn(|_| WitnessSlot {
            acquired_gen: AtomicU64::new(0),
            published_gen: AtomicU64::new(0),
            occupied: AtomicBool::new(false),
        });
        Box::into_raw(Box::new(WitnessChunk {
            slots,
            next: AtomicPtr::new(ptr::null_mut()),
        }))
    }
}

/// HEAD of the grow-only witness chunk list (null until the first acquire). Both
/// the per-thread acquire (which may grow it) and the driver's live-re-walk read
/// from HEAD; growth is a Release CAS on a chunk's `next` (or on HEAD for the very
/// first chunk).
///
/// Source-coupled by witness slot acquire/snapshot paths.
#[allow(dead_code)]
static WITNESS_HEAD: AtomicPtr<WitnessChunk> = AtomicPtr::new(ptr::null_mut());

/// Global cursor of the next free slot INDEX across the whole chunk list (a flat
/// index; chunk = idx/256, slot = idx%256). Bumped with `fetch_add` at acquire;
/// when it crosses a 256 boundary the acquiring thread grows a new chunk. Only
/// EVER increases (slots are never freed back — grow-only), so no ABA.
///
/// Source-coupled by witness slot acquisition.
#[allow(dead_code)]
static WITNESS_NEXT_INDEX: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread's permanently-owned witness slot pointer (null until the first
    /// [`witness_acquire_slot`]). Process-lifetime + never-realloc ⇒ the raw
    /// pointer stays valid for the thread's life. Used by restamp/release to reach
    /// THIS thread's slot in O(1) without re-walking.
    ///
    static MY_WITNESS_SLOT: Cell<*const WitnessSlot> = const { Cell::new(ptr::null()) };
}

/// Set true by the driver (`set_current_witness_ok(true)`) ONLY after
/// [`requestor_wait_for_all_reified_parked`] proves every occupied slot is stamped
/// for `cur_gen`; cleared at `end_rendezvous_cycle`. The SOLE sweep gate
/// (`gate_open_rendezvous` reads [`current_witness_ok`]). rt1 #4: one predicate,
/// two readers.
///
/// Source-coupled by the dedicated driver witness wait and cycle teardown.
#[allow(dead_code)]
static CURRENT_WITNESS_OK: AtomicBool = AtomicBool::new(false);

/// Resolve (lazily allocating/growing) the chunk + slot for a flat slot `index`.
/// Walks the grow-only list from HEAD, appending chunks as needed. Append is a
/// Release CAS (HEAD for the first chunk, else the predecessor's `next`); a lost
/// CAS means a peer grew it — re-read and continue. Returns a stable `*const`.
///
/// Source-coupled by witness slot acquisition.
#[allow(dead_code)]
fn witness_slot_at(index: usize) -> *const WitnessSlot {
    let chunk_idx = index / 256;
    let slot_idx = index % 256;
    // Ensure HEAD exists.
    let mut head = WITNESS_HEAD.load(Ordering::Acquire);
    if head.is_null() {
        let fresh = WitnessChunk::new_leaked();
        match WITNESS_HEAD.compare_exchange(
            ptr::null_mut(),
            fresh,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => head = fresh,
            Err(actual) => {
                // A peer won — reclaim our leak and use theirs.
                // SAFETY: `fresh` came from Box::into_raw and was never published.
                drop(unsafe { Box::from_raw(fresh) });
                head = actual;
            }
        }
    }
    // Walk to the target chunk, growing as needed.
    let mut cur = head;
    for _ in 0..chunk_idx {
        // SAFETY: every chunk pointer in the list is a leaked Box (process-lifetime).
        let next = unsafe { (*cur).next.load(Ordering::Acquire) };
        if next.is_null() {
            let fresh = WitnessChunk::new_leaked();
            // SAFETY: `cur` is a valid leaked chunk.
            match unsafe {
                (*cur).next.compare_exchange(
                    ptr::null_mut(),
                    fresh,
                    Ordering::Release,
                    Ordering::Acquire,
                )
            } {
                Ok(_) => cur = fresh,
                Err(actual) => {
                    // SAFETY: `fresh` was never published.
                    drop(unsafe { Box::from_raw(fresh) });
                    cur = actual;
                }
            }
        } else {
            cur = next;
        }
    }
    // SAFETY: `cur` is a valid leaked chunk; `slot_idx < 256`.
    unsafe { &(*cur).slots[slot_idx] as *const WitnessSlot }
}

/// V4 slot ACQUIRE — called ONLY at `EvalGuard::enter` (`prev==0`), BEFORE the
/// `N_THREADS.fetch_add` is observable (RT-1, so every counted thread is in `snap`).
/// On the thread's FIRST acquire it claims a fresh grow-only slot (recorded in
/// [`MY_WITNESS_SLOT`]); thereafter it reuses the same slot. Sets
/// `acquired=current_cycle_gen()`, `published=acquired-1` (so a stale stamp can
/// never satisfy `published>=cur_gen`), `occupied=true` LAST (Release).
///
/// Source-coupled by `EvalGuard::enter`.
#[allow(dead_code)]
pub(crate) fn witness_acquire_slot() {
    let slot_ptr = MY_WITNESS_SLOT.with(|c| c.get());
    let slot_ptr = if slot_ptr.is_null() {
        let index = WITNESS_NEXT_INDEX.fetch_add(1, Ordering::AcqRel);
        let p = witness_slot_at(index);
        MY_WITNESS_SLOT.with(|c| c.set(p));
        p
    } else {
        slot_ptr
    };
    // SAFETY: `slot_ptr` is a process-lifetime never-realloc slot owned by THIS thread.
    let slot = unsafe { &*slot_ptr };
    let g = current_cycle_gen();
    // Intra-slot order: acquired (Release) → published (Release) → occupied (Release LAST).
    slot.acquired_gen.store(g, Ordering::Release);
    slot.published_gen.store(g.wrapping_sub(1), Ordering::Release);
    slot.occupied.store(true, Ordering::Release);
    // MINOR liveness (v3 pin): a driver blocked in the 5 s warn-loop on new-chunk
    // growth wakes promptly. Not safety (new entrants are admission-blocked).
    {
        let _lock = RENDEZVOUS_MUTEX.lock();
        RENDEZVOUS_CONDVAR.notify_all();
    }
}

/// V4 slot RELEASE — called ONLY at the OUTERMOST `EvalGuard::drop` (`depth==1`),
/// AFTER the `N_THREADS.fetch_sub`. Clears `occupied` (Release). This is the ONLY
/// site that un-occupies a slot; the frozen machine of a PARKED thread keeps its
/// slot occupied (do NOT release at any safepoint drop — Pin 3).
///
/// Source-coupled by the true outermost `EvalGuard::drop`.
#[allow(dead_code)]
pub(crate) fn witness_release_slot() {
    let slot_ptr = MY_WITNESS_SLOT.with(|c| c.get());
    if slot_ptr.is_null() {
        return; // never acquired on this thread (e.g. a depth bookkeeping edge)
    }
    // SAFETY: process-lifetime never-realloc slot owned by THIS thread.
    let slot = unsafe { &*slot_ptr };
    slot.occupied.store(false, Ordering::Release);
    // ── LOST-NOTIFY FIX (2026-06-03) ──────────────────────────────────────────
    // Clearing `occupied` REMOVES this slot from the driver's witness predicate
    // (`requestor_wait_for_all_reified_parked` skips un-occupied slots), so this
    // store can flip the driver's all-satisfied predicate false→true. The driver
    // may be blocked on `RENDEZVOUS_CONDVAR` waiting for EXACTLY this — a worker it
    // is waiting on (occupied, published<cur_gen) that simply FINISHES and drops its
    // outermost `EvalGuard` (releasing its slot) instead of re-parking+publishing.
    // `witness_acquire_slot` already notifies on the symmetric occupied=false→true
    // transition; release MUST notify too. Without it the driver sleeps to its 5 s
    // `wait_for` timeout on every such release, and robot's many FANOUT=8 rendezvous
    // cycles accumulate those stalls past the test budget — the observed ~6%
    // "deadlock" (all threads parked in a diag snapshot). A notify is SAFE-BY-
    // CONSTRUCTION: it can only cause a spurious wake, which the driver's predicate
    // re-check absorbs (Mesa-monitor discipline) — it can never cause a sweep-too-
    // early or any incorrectness. Taken under RENDEZVOUS_MUTEX, mirroring
    // `witness_acquire_slot` (no new lock-order edge: both acquire & release run at
    // EvalGuard enter/drop and take ONLY this mutex, briefly).
    {
        let _lock = RENDEZVOUS_MUTEX.lock();
        RENDEZVOUS_CONDVAR.notify_all();
    }
}

/// V4 slot RESTAMP — called at BOTH `reacquire_*` (`:5190` non-full, `:5232` full)
/// and at EACH straddle re-park iteration. Updates `acquired=g` (Release) WITHOUT
/// toggling `occupied` (the slot stays occupied across the whole resume). Does NOT
/// touch `published` — only a genuine [`note_reified_park`] re-publishes.
///
/// Source-coupled by safepoint rejoin and straddle re-park.
#[allow(dead_code)]
pub(crate) fn witness_restamp_acquired(g: u64) {
    let slot_ptr = MY_WITNESS_SLOT.with(|c| c.get());
    if slot_ptr.is_null() {
        return;
    }
    // SAFETY: process-lifetime never-realloc slot owned by THIS thread.
    let slot = unsafe { &*slot_ptr };
    slot.acquired_gen.store(g, Ordering::Release);
}

/// V4 STAMP — the SOLE setter of `published_gen`. Sets `published=g` IFF this
/// thread's slot is `occupied && acquired==g` (a genuine reified park of THIS
/// cycle's machine). Called ONLY from inside [`worker_park_and_root_in_cycle`]
/// (the 2 reified parks + the straddle re-park) — after the machine is in
/// `WORKER_ROOT_BUFFER` — and from B3 (after register+fence). The finishers
/// ([`worker_finish_into_buffer`]) + the zero-root drop bump get NO stamp ⇒ they
/// can NEVER satisfy the witness BY CONSTRUCTION.
///
/// Source-coupled to genuine reified parks only.
#[allow(dead_code)]
pub(crate) fn note_reified_park(g: u64) {
    let slot_ptr = MY_WITNESS_SLOT.with(|c| c.get());
    if slot_ptr.is_null() {
        return;
    }
    // SAFETY: process-lifetime never-realloc slot owned by THIS thread.
    let slot = unsafe { &*slot_ptr };
    // Read occupied (Acq) → acquired (Acq) before publishing.
    if slot.occupied.load(Ordering::Acquire) && slot.acquired_gen.load(Ordering::Acquire) == g {
        slot.published_gen.store(g, Ordering::Release);
    }
}

/// A stable snapshot of the witness directory for the driver: the list of slot
/// POINTERS that are occupied at the snapshot instant (taken AFTER admission
/// closed). The pointers are process-lifetime never-realloc, so
/// [`requestor_wait_for_all_reified_parked`] can LIVE-RE-WALK them (and the whole
/// grow-only list from HEAD) on every wake. `_cur_gen` is accepted for symmetry
/// with the predicate; the snapshot itself is gen-agnostic (it is just "who is
/// occupied now").
///
/// Source-coupled by dedicated driver root preparation.
#[allow(dead_code)]
pub(crate) fn snapshot_witness(_cur_gen: u64) -> Vec<*const WitnessSlot> {
    let mut out: Vec<*const WitnessSlot> = Vec::new();
    let mut cur = WITNESS_HEAD.load(Ordering::Acquire);
    while !cur.is_null() {
        // SAFETY: every chunk pointer is a leaked Box (process-lifetime).
        let chunk = unsafe { &*cur };
        for slot in chunk.slots.iter() {
            if slot.occupied.load(Ordering::Acquire) {
                out.push(slot as *const WitnessSlot);
            }
        }
        cur = chunk.next.load(Ordering::Acquire);
    }
    out
}

/// The strict-`>` per-slot witness predicate: `published_gen >= cur_gen OR
/// acquired_gen > cur_gen`. NEVER `acquired >= cur_gen` (a re-stamped-but-not-yet-
/// republished slot `acquired==cur_gen, published<cur_gen` MUST read "wait"). Read
/// order: occupied is NOT re-checked here (the caller decides scope); `published`
/// (Acq) then `acquired` (Acq).
///
/// Source-coupled by the witness wait and oracle.
#[allow(dead_code)]
#[inline]
fn witness_slot_satisfied(slot: &WitnessSlot, cur_gen: u64) -> bool {
    slot.published_gen.load(Ordering::Acquire) >= cur_gen
        || slot.acquired_gen.load(Ordering::Acquire) > cur_gen
}

/// REQUISITE driver wait (replaces `requestor_wait_for_parked_count`): block until
/// EVERY occupied witness slot satisfies the strict-`>` predicate for `cur_gen`.
/// LIVE-RE-WALKS the grow-only chunk list from HEAD on every wake (A-straddle-2:
/// picks up a slot re-occupied-for-`cur_gen` AFTER the snapshot + newly-grown
/// chunks). `snap` is consulted only as a non-empty hint; the authoritative scan
/// is the live re-walk (an occupied slot NOT in `snap` — a straddle re-occupant —
/// is still required to satisfy). Waits on [`RENDEZVOUS_CONDVAR`] holding
/// [`RENDEZVOUS_MUTEX`] across {predicate, wait_for} (lost-wakeup-safe: the
/// parker's stamp+notify is under the same mutex). 5 s warn-recheck — NEVER
/// proceed-on-timeout (witness-sole-gate ⇒ a proceed-on-timeout = silent UAF).
///
/// Source-coupled by the dedicated driver before root-buffer drain.
#[allow(dead_code)]
pub(crate) fn requestor_wait_for_all_reified_parked(snap: &[*const WitnessSlot], cur_gen: u64) {
    // `snap` is retained for the caller's diagnostics / future use; the wait scans
    // the LIVE directory each wake (A-straddle-2). Touch it so the param is not
    // flagged unused if the live walk is the sole authority.
    let _ = snap;
    let mut lock = RENDEZVOUS_MUTEX.lock();
    loop {
        // LIVE re-walk from HEAD: ∀ occupied slot: predicate holds?
        let mut all_ok = true;
        let mut cur = WITNESS_HEAD.load(Ordering::Acquire);
        while !cur.is_null() {
            // SAFETY: process-lifetime leaked chunk.
            let chunk = unsafe { &*cur };
            for slot in chunk.slots.iter() {
                if slot.occupied.load(Ordering::Acquire) && !witness_slot_satisfied(slot, cur_gen) {
                    all_ok = false;
                    break;
                }
            }
            if !all_ok {
                break;
            }
            cur = chunk.next.load(Ordering::Acquire);
        }
        if all_ok {
            return;
        }
        let result = RENDEZVOUS_CONDVAR.wait_for(&mut lock, RENDEZVOUS_WAIT_TIMEOUT);
        if result.timed_out() {
            tracing::warn!(
                "requestor_wait_for_all_reified_parked: not all occupied witness slots stamped \
                 for cur_gen {} after {:?} — re-checking (NEVER proceeding on timeout)",
                cur_gen,
                RENDEZVOUS_WAIT_TIMEOUT,
            );
        }
    }
}

/// Per-slot oracle predicate (the SAME live-re-walk the driver wait uses), exposed
/// for the D5 oracle (`assert_rendezvous_union_complete`). Returns `(all_ok,
/// first_violator_ptr)`. Debug-only callers.
///
/// Source-coupled by the rendezvous oracle.
#[allow(dead_code)]
pub(crate) fn all_occupied_slots_satisfied(cur_gen: u64) -> (bool, *const WitnessSlot) {
    let mut cur = WITNESS_HEAD.load(Ordering::Acquire);
    while !cur.is_null() {
        // SAFETY: process-lifetime leaked chunk.
        let chunk = unsafe { &*cur };
        for slot in chunk.slots.iter() {
            if slot.occupied.load(Ordering::Acquire) && !witness_slot_satisfied(slot, cur_gen) {
                return (false, slot as *const WitnessSlot);
            }
        }
        cur = chunk.next.load(Ordering::Acquire);
    }
    (true, ptr::null())
}

/// Diagnostic aggregate over the V4 witness directory — the SAME lock-free live-re-walk
/// the driver wait ([`all_occupied_slots_satisfied`]) uses, but summing instead of
/// short-circuiting. Returns `(total_slots, occupied, occupied_unpublished)` where
/// `occupied_unpublished` counts OCCUPIED slots NOT yet satisfied for `cur_gen` (a parked
/// mutator that has not re-published its machine for the current cycle — the stranded-parker
/// indicator the E1 liveness diagnosis keys on). Lock-free (atomic loads over the never-realloc
/// leaked chunk list), so it is safe to call from the SIGUSR1 diagnostic watcher thread during
/// a suspected hang. #275: feeds the index-mode dump's `GC Cycle / Rendezvous` section.
pub(crate) fn witness_directory_summary(cur_gen: u64) -> (usize, usize, usize) {
    let total = WITNESS_NEXT_INDEX.load(Ordering::Acquire);
    let mut occupied = 0usize;
    let mut occupied_unpublished = 0usize;
    let mut cur = WITNESS_HEAD.load(Ordering::Acquire);
    while !cur.is_null() {
        // SAFETY: process-lifetime leaked chunk (same invariant as `all_occupied_slots_satisfied`).
        let chunk = unsafe { &*cur };
        for slot in chunk.slots.iter() {
            if slot.occupied.load(Ordering::Acquire) {
                occupied += 1;
                if !witness_slot_satisfied(slot, cur_gen) {
                    occupied_unpublished += 1;
                }
            }
        }
        cur = chunk.next.load(Ordering::Acquire);
    }
    (total, occupied, occupied_unpublished)
}

/// Driver: publish that the witness gate is SATISFIED for the in-flight cycle (the
/// SOLE thing `gate_open_rendezvous` reads). Set true only AFTER
/// [`requestor_wait_for_all_reified_parked`] returns; cleared at
/// [`end_rendezvous_cycle`] via `set_current_witness_ok(false)`.
///
/// Source-coupled by the dedicated driver witness gate.
#[allow(dead_code)]
#[inline]
pub(crate) fn set_current_witness_ok(ok: bool) {
    CURRENT_WITNESS_OK.store(ok, Ordering::Release);
}

/// Reader for `gate_open_rendezvous` (Acquire). True ⟺ the driver proved every
/// occupied slot stamped this cycle.
///
/// Source-coupled by `IndexHeap::gate_open_rendezvous`.
#[allow(dead_code)]
#[inline]
pub(crate) fn current_witness_ok() -> bool {
    CURRENT_WITNESS_OK.load(Ordering::Acquire)
}

// ============================================================================
// E1-FLIP Path B V4 — B2′: the GC-walked global live-env (E₀) registry
// ============================================================================
//
// B2′ (docs/cesk-gc/e1-flip-pathB-design.md §B2′): E₀'s env STRUCT
// (named_spaces/bindings/types/states/...) is per-`GenericEnvironmentShared` (CoW-
// cloned at fork), NOT one process-global Arc — so the GC thread cannot reach it
// from a global handle. This registry (modeled byte-for-byte on `LIVE_DISPATCHES`)
// lets the driver walk EVERY live env's roots each cycle, participant-independently,
// so E₀ is covered even when no Trampoline participant happens to be parked.

/// E1-FLIP Path B V4 (B2′): a GC-thread-readable live environment whose persistent
/// E₀ roots the driver walks every cycle. Implemented by
/// `GenericEnvironmentShared<MettaValue>` (delegating to the verified-complete
/// `collect_roots_into`, core.rs:2170).
///
/// Live on the index-gc dedicated-rendezvous path; cfg-gated out of the slab
/// build.
pub trait EnvRoots: Send + Sync {
    fn collect_env_roots(&self, out: &mut Vec<MettaValue>);
}

/// E1-FLIP Path B V4 (B2′): the registry of live env structs. `None` marks a free
/// slot (kept, not removed — the `LIVE_DISPATCHES`/`SAFEPOINT_ROOTS` discipline).
/// Each entry is a `Weak` so an env dropped without deregistering self-prunes on the
/// next walk. `std::sync::Weak` is fully-qualified because the top-level
/// `use std::sync::Weak` import was slab-`ROOT_REGISTRY`-only and was removed with it.
static LIVE_ENVS: OnceLock<Mutex<Vec<Option<std::sync::Weak<dyn EnvRoots>>>>> = OnceLock::new();

fn live_envs() -> &'static Mutex<Vec<Option<std::sync::Weak<dyn EnvRoots>>>> {
    LIVE_ENVS.get_or_init(|| Mutex::new(Vec::new()))
}

/// E1-FLIP Path B V4 (B2′): RAII handle that frees its [`LIVE_ENVS`] slot on drop.
/// Held for the lifetime of the registration scope (every `EvalGuard::enter` +
/// every branch-worker spawn). Mirrors `LiveDispatchHandle`.
///
/// Live on the index-gc dedicated-rendezvous path; cfg-gated out of the slab
/// build.
pub struct LiveEnvHandle {
    idx: usize,
}

impl Drop for LiveEnvHandle {
    fn drop(&mut self) {
        let registry = live_envs();
        let mut guard = registry.lock();
        if self.idx < guard.len() {
            guard[self.idx] = None;
        }
    }
}

/// E1-FLIP Path B V4 (B2′): register a live env so the driver can walk its E₀ roots
/// for the registration's lifetime. Stores `Arc::downgrade(e)` (a `Weak`); reuses a
/// free slot or appends. Call (RAII) at `EvalGuard::enter` + branch-worker spawn.
///
/// Live on the index-gc dedicated-rendezvous path; cfg-gated out of the slab
/// build.
pub fn register_live_env(e: &Arc<dyn EnvRoots>) -> LiveEnvHandle {
    let registry = live_envs();
    let mut guard = registry.lock();
    let weak = Arc::downgrade(e);
    for (idx, slot) in guard.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(weak);
            return LiveEnvHandle { idx };
        }
    }
    let idx = guard.len();
    guard.push(Some(weak));
    LiveEnvHandle { idx }
}

/// E1-FLIP Path B V4 (B2′): walk every live env's E₀ roots into `out`, pruning slots
/// whose `Weak` no longer upgrades. Called by the driver every cycle (beside
/// `collect_safepoint_roots`). The `collect_env_roots` body uses blocking `.read()`
/// (via `collect_roots_into`); the B2′-deadlock-unreachable argument (no park site
/// spans an env `.write()`, design §"Source-verified facts") makes this safe.
///
/// Live on the index-gc dedicated-rendezvous path; cfg-gated out of the slab
/// build.
pub fn collect_live_env_anchors(out: &mut Vec<MettaValue>) {
    if let Some(registry) = LIVE_ENVS.get() {
        let mut guard = registry.lock();
        for slot in guard.iter_mut() {
            let prune = match slot {
                Some(weak) => match weak.upgrade() {
                    Some(strong) => {
                        strong.collect_env_roots(out);
                        false
                    }
                    None => true,
                },
                None => false,
            };
            if prune {
                *slot = None;
            }
        }
    }
}

/// `true` while a rendezvous-collector requestor owns the rendezvous (one
/// collector at a time). Set by [`begin_gc_rendezvous`] via a `false→true` CAS
/// (AcqRel) and cleared by [`end_gc_rendezvous`] (Release). A second would-be
/// requestor that loses the CAS backs off rather than racing a concurrent mark.
///
/// DEAD until D2.x. See `docs/cesk-gc/phase-d-d1-d2-rendezvous-design.md` §D1.
#[allow(dead_code)] // DEAD until D2.x wires the rendezvous call sites.
pub(crate) static GC_REQUESTOR_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Shared buffer of structural roots self-collected by parking workers (D2). A
/// single `Vec`; appends are O(roots/worker) and happen once per worker per
/// cycle. The requestor [`drain_worker_root_buffer`]s it (∪ `E₀` ∪ driver-C)
/// before marking. Carries `MettaValue` root HANDLES — exactly what
/// `collect_machine_roots` already produces — NOT a serialized machine.
///
/// Live on the dedicated-rendezvous path; the attribute is for build/test
/// configurations that do not enter that path.
#[allow(dead_code)]
pub(crate) static WORKER_ROOT_BUFFER: Mutex<Vec<MettaValue>> = Mutex::new(Vec::new());

/// Mutex + Condvar pair on which the REQUESTOR waits for all workers to park
/// (`active_evaluator_count()==0`). A parking worker holds [`RENDEZVOUS_MUTEX`]
/// across {`WORKERS_PARKED_FOR_GC.fetch_add`, `RENDEZVOUS_CONDVAR.notify_all`}
/// and the requestor holds it across {predicate check, `wait_for`} — this is the
/// lost-wakeup-safe handshake (HB2 + EvalGuard::enter :2920-2941 pattern). NEW
/// and SEPARATE from `GC_PROGRESS_*` / `QUIESCENT_*` (Risk R4).
///
/// Live on the dedicated-rendezvous path; the attribute is for build/test
/// configurations that do not enter that path.
#[allow(dead_code)]
pub(crate) static RENDEZVOUS_MUTEX: Mutex<()> = Mutex::new(());
#[allow(dead_code)]
pub(crate) static RENDEZVOUS_CONDVAR: Condvar = Condvar::new();

/// Mutex + Condvar pair on which a parked WORKER waits to be resumed
/// (`!is_gc_requested()`). The requestor holds [`RESUME_MUTEX`] across
/// {`GC_REQUESTED.store(false)`, `RESUME_CONDVAR.notify_all`} (done by
/// [`resume_workers`]) and a parked worker holds it across {predicate check,
/// `wait_for`} — the lost-wakeup-safe resume handshake (HB4). NEW and SEPARATE
/// from `GC_PROGRESS_*` / `QUIESCENT_*` (Risk R4).
///
/// Live on the dedicated-rendezvous path; the attribute is for build/test
/// configurations that do not enter that path.
#[allow(dead_code)]
pub(crate) static RESUME_MUTEX: Mutex<()> = Mutex::new(());
#[allow(dead_code)]
pub(crate) static RESUME_CONDVAR: Condvar = Condvar::new();

/// Maximum time a rendezvous wait (`requestor_wait_for_parked` /
/// `worker_park_and_root`) parks before logging a warning and re-checking its
/// predicate. This is a LIVENESS BACKSTOP, not a hard budget: the loop re-checks
/// the real predicate after every timeout and only exits when it actually holds,
/// so a slow worker yields a warn-and-retry (bounding Risk R1) rather than a
/// premature unblock. Mirrors `EvalGuard::enter`'s `GC_WAIT_TIMEOUT` (:2918).
#[allow(dead_code)]
const RENDEZVOUS_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// TEST-ONLY: force the D2.1 rendezvous integration test into the parked-worker
/// primitive path. Production rendezvous collection is governed by
/// [`dedicated_gc_enabled`]; there is intentionally no second production env gate.
///
/// Reset to `false` at the end of the test so it does not leak into other tests.
#[cfg(test)]
pub(crate) fn force_rendezvous_enabled_for_test(on: bool) {
    RENDEZVOUS_FORCED_FOR_TEST.store(on, Ordering::Release);
}

/// TEST-ONLY: read back the D2.1 rendezvous override.
#[cfg(test)]
pub(crate) fn rendezvous_forced_for_test() -> bool {
    RENDEZVOUS_FORCED_FOR_TEST.load(Ordering::Acquire)
}

/// Backing flag for [`force_rendezvous_enabled_for_test`]. Default `false` ⇒ the
/// integration test has not engaged the direct rendezvous primitive path.
#[cfg(test)]
static RENDEZVOUS_FORCED_FOR_TEST: AtomicBool = AtomicBool::new(false);

/// Whether index collection is driven by the dedicated CESK GC thread.
///
/// This is no longer an environment-gated debug mode: the E1 dedicated-driver
/// protocol is the normal collector regime, so this is just `gc_mode_is_index()`
/// (always true since the slab store was removed).
pub(crate) fn dedicated_gc_enabled() -> bool {
    crate::backend::models::metta_value::gc_mode_is_index()
}

/// Acquire the rendezvous as the sole collector (one collector at a time).
///
/// CAS [`GC_REQUESTOR_ACTIVE`] `false→true` (AcqRel on success so the subsequent
/// `request_gc()`/buffer reads happen-after any prior collector's
/// `end_gc_rendezvous` Release; Acquire on failure). Returns `true` if THIS
/// caller now owns the rendezvous; `false` if another requestor already owns it
/// (caller must back off — do NOT proceed to mark concurrently).
///
/// PROTOCOL: paired with [`end_gc_rendezvous`]; see the §D1 requestor sequence.
/// DEAD until D2.3.
#[allow(dead_code)] // DEAD until D2.3 (requestor wiring); exercised by the D1.1 test.
pub(crate) fn begin_gc_rendezvous() -> bool {
    GC_REQUESTOR_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Release rendezvous ownership (the last step of the requestor sequence, after
/// `resume_workers()`).
///
/// `Release` so a subsequent [`begin_gc_rendezvous`] on another thread that
/// acquires ownership observes all of this collector's writes (the just-finished
/// sweep + buffer reset). DEAD until D2.3.
#[allow(dead_code)] // DEAD until D2.3 (requestor wiring); exercised by the D1.1 test.
pub(crate) fn end_gc_rendezvous() {
    GC_REQUESTOR_ACTIVE.store(false, Ordering::Release);
}

/// WORKER side: append `roots` to the shared buffer, signal "parked", and park
/// until the requestor clears `GC_REQUESTED`.
///
/// This is JUST the buffer-append + counter-bump + notify-requestor + park; the
/// `eval_loop` caller owns dropping its `EvalGuard` BEFORE this call and
/// reacquiring it AFTER (D2.1) — keeping this function self-contained makes it
/// unit-testable in isolation (see `test_rendezvous_park_resumes_on_clear`).
///
/// Sequence (HB2 then HB4):
///   1. `WORKER_ROOT_BUFFER.lock().extend_from_slice(roots)` — publish my roots.
///   2. `WORKERS_PARKED_FOR_GC.fetch_add(1, AcqRel)` — the AcqRel is the release
///      fence for step 1's buffer writes (so the requestor, after its Acquire on
///      the same counter / under RENDEZVOUS_MUTEX, sees them — HB2).
///   3. Under [`RENDEZVOUS_MUTEX`]: `RENDEZVOUS_CONDVAR.notify_all()` — wake the
///      requestor that is waiting for everyone to park. Holding the mutex across
///      the notify is the lost-wakeup guard (the requestor holds it across its
///      predicate check + wait).
///   4. Park: under [`RESUME_MUTEX`], `while is_gc_requested() { wait_for(5s) }`
///      — observe the requestor's `GC_REQUESTED.store(false)` Release (HB4),
///      lost-wakeup-safe because `resume_workers()` holds RESUME_MUTEX across the
///      store + notify.
///
/// DEAD until D2.1. See `docs/cesk-gc/phase-d-d1-d2-rendezvous-design.md` §D1/§D2.
#[allow(dead_code)] // DEAD until D2.1 (worker poll points); exercised by the D1.1 test.
pub(crate) fn worker_park_and_root(roots: &[MettaValue]) {
    // (1) publish my self-collected roots.
    WORKER_ROOT_BUFFER.lock().extend_from_slice(roots);
    // (2) signal parked; the AcqRel fetch_add release-fences the buffer append.
    WORKERS_PARKED_FOR_GC.fetch_add(1, Ordering::AcqRel);
    // (3) wake the requestor (lost-wakeup-safe: mutex held across notify).
    {
        let _lock = RENDEZVOUS_MUTEX.lock();
        RENDEZVOUS_CONDVAR.notify_all();
    }
    // (4) park until the requestor clears GC_REQUESTED (HB4).
    worker_wait_for_resume();
}

/// WORKER side (E1-c, cycle-generation-gated variant of [`worker_park_and_root`]).
/// Publish `roots` + signal parked ONLY if still in the cycle the worker observed
/// (`GC_CYCLE_GEN == my_gen`), all under [`RENDEZVOUS_MUTEX`] so the gen-check,
/// buffer append, count bump, and notify are atomic w.r.t. the requestor's
/// cycle-end gen bump (under the same mutex). A straggler whose cycle already
/// ended finds `gen != my_gen`, DROPS its (stale) roots, and does NOT bump the
/// parked-count — so it cannot corrupt the next cycle's `WORKERS_PARKED_FOR_GC ==
/// n` gate (§1.3 straggler exclusion). Then parks via
/// [`worker_resume_wait_for_cycle`] until the cycle ends.
///
/// Source-coupled by VM/JIT tier leaves and trampoline parent parks.
#[allow(dead_code)]
pub(crate) fn worker_park_and_root_in_cycle(roots: &[MettaValue], my_gen: u64) {
    {
        let _lock = RENDEZVOUS_MUTEX.lock();
        // ── Bug #309: the PHANTOM-FUTURE park gate (tla/ParkedPhantomCycleGate.tla,
        // RealCycleGate = TRUE; the branch-B/pump twin of the E5 straddle's
        // StartedCycleGate). A worker that observed `is_gc_requested()` reads
        // `my_gen = current_cycle_gen()` on its way here; if the driver CLOSED the
        // cycle in that window (end-bump K→K+1 + request cleared, both under THIS
        // mutex), the worker arrives with my_gen = K+1 — the post-close gen of a
        // cycle NOBODY requested. The straggler gate below (`gen == my_gen`)
        // PASSES for it, so without this gate the phantom would (a) pre-bump the
        // parked count of a future cycle (a masked-parker missed-root hazard),
        // (b) publish stale-conservative roots into that cycle's buffer, and
        // (c) strand forever in `worker_resume_wait_for_cycle` waiting for a
        // gen bump that never comes (captured live: autopsy rep 17 —
        // cycle_gen 95, cycle_started 94, gc_requested false, gc_wait park = 2).
        // A cycle `my_gen` is REAL iff it is already OPEN (started == my_gen) or
        // still PENDING (requested). Otherwise: skip the park entirely and
        // resume evaluating — the request this worker saw is fully serviced.
        let real_cycle = current_cycle_started() == my_gen || is_gc_requested();
        if !real_cycle && GC_CYCLE_GEN.load(Ordering::Acquire) == my_gen {
            tracing::debug!(
                my_gen,
                started = current_cycle_started(),
                "worker_park_and_root_in_cycle: phantom-future park skipped (#309)"
            );
            return;
        }
        if GC_CYCLE_GEN.load(Ordering::Acquire) == my_gen {
            // still my cycle: publish roots; the AcqRel fetch_add release-fences
            // the append (HB2); then wake the requestor.
            WORKER_ROOT_BUFFER.lock().extend_from_slice(roots);
            WORKERS_PARKED_FOR_GC.fetch_add(1, Ordering::AcqRel);
            RENDEZVOUS_CONDVAR.notify_all();
            // E1-FLIP §Part 3: this thread has now balanced the parked-count for
            // `my_gen` — suppress a redundant EvalGuard::drop finish-bump this cycle.
            note_cycle_bumped(my_gen);
            // ── E1-FLIP Path B V4 — THE STAMP (the SOLE published-setter) ──
            // The machine is now in WORKER_ROOT_BUFFER (above, publish-BEFORE-stamp);
            // stamp this thread's witness slot `published=my_gen` IFF it is
            // occupied && acquired==my_gen. Inside the gen-gated RENDEZVOUS_MUTEX
            // block so {publish, count-bump, notify, STAMP} are atomic w.r.t. the
            // driver's cycle-end gen bump (same mutex) and the parker's notify
            // (above) is never lost. The finishers + the zero-root drop bump get NO
            // note_reified_park ⇒ they can NEVER satisfy the witness BY CONSTRUCTION.
            // BYTE-IDENTICAL WHEN DORMANT: the `dedicated_gc_enabled()` gate.
            {
                if dedicated_gc_enabled() {
                    note_reified_park(my_gen);
                }
            }
        }
        // else: my cycle already ended → stale roots dropped, no bump, no stamp.
    }
    worker_resume_wait_for_cycle(my_gen);
}

/// WORKER side (E1-c §1.1): the FINISHER — the cycle-generation-gated sibling of
/// [`worker_park_and_root_in_cycle`] WITHOUT the trailing
/// [`worker_resume_wait_for_cycle`] park. A worker about to RETURN its in-flight
/// result (and drop its outermost [`EvalGuard`], whose `N_THREADS.fetch_sub`
/// silently removes it from the active set) while a rendezvous cycle is pending
/// publishes its result roots + bumps the parked-count, then RETURNS (does NOT
/// park). This balances the driver's [`requestor_wait_for_parked_count`] gate for a
/// worker that FINISHES rather than reaching a park safepoint — without it the gate
/// caps below `n` and hangs forever (the §1.1 hang).
///
/// Gen-gating (§1.3): publish + bump ONLY if `GC_CYCLE_GEN == my_gen`, all under
/// [`RENDEZVOUS_MUTEX`] (atomic w.r.t. the driver's cycle-end gen bump in
/// [`end_rendezvous_cycle`], same mutex). A STRAGGLER whose cycle already ended finds
/// `gen != my_gen`, DROPS its (stale) roots, and does NOT bump — so it cannot
/// over-count the NEXT cycle's gate. Sound because such a finisher stored its result
/// into the parent's slot before the next cycle's `n` snapshot (the parent's
/// `WaitForParallel` K-frame then roots it structurally), so dropping its buffer
/// contribution loses nothing live. HB2 is identical to the parker: the AcqRel
/// `fetch_add` release-fences the buffer append. NO park: after the bump the worker
/// keeps running (its later allocations during a concurrent mark are covered by
/// allocate-black).
///
/// Source-coupled by dispatch/collapse finisher paths.
#[allow(dead_code)]
pub(crate) fn worker_finish_into_buffer(roots: &[MettaValue], my_gen: u64) {
    let _lock = RENDEZVOUS_MUTEX.lock();
    if GC_CYCLE_GEN.load(Ordering::Acquire) == my_gen {
        // still my cycle: publish roots; the AcqRel fetch_add release-fences the
        // append (HB2); then wake the requestor. NO worker_resume_wait_for_cycle —
        // the finisher returns to drop its EvalGuard and complete normally.
        WORKER_ROOT_BUFFER.lock().extend_from_slice(roots);
        WORKERS_PARKED_FOR_GC.fetch_add(1, Ordering::AcqRel);
        RENDEZVOUS_CONDVAR.notify_all();
        // E1-FLIP §Part 3: balanced the parked-count for `my_gen` — suppress a
        // redundant EvalGuard::drop finish-bump this cycle.
        note_cycle_bumped(my_gen);
    }
    // else: my cycle already ended → stale roots dropped, no bump (straggler exclusion).
}

/// WORKER side (E1-c, Round-4 F2): park until the cycle the worker parked for ENDS
/// (`GC_CYCLE_GEN != my_gen`), then return. Gating on the cycle GENERATION — not the
/// boolean `GC_REQUESTED` — is the F2 fix: ≥10 non-driver callers set `GC_REQUESTED`,
/// so a back-to-back UNRELATED trigger could re-set it and a boolean-gated resume
/// would re-block (or miss its wake); the gen only ever ADVANCES, so `!= my_gen` is
/// monotone-correct.
///
/// ── E5 (Mesa-correct gen-wait, docs/cesk-gc/e1-flip-deadlock-straddle-rootcause-
/// 2026-06-03.md §Step 2) ──────────────────────────────────────────────────────────
/// This wait is now keyed on [`RENDEZVOUS_CONDVAR`] holding [`RENDEZVOUS_MUTEX`] — the
/// SAME mutex/condvar under which `GC_CYCLE_GEN` is bumped (`end_rendezvous_cycle`).
/// PREVIOUSLY it waited under [`RESUME_MUTEX`] on [`RESUME_CONDVAR`] while the gen was
/// bumped under [`RENDEZVOUS_MUTEX`] — a cross-mutex Mesa-monitor violation
/// (predicate-lock ≠ wait-lock): a worker could read `gen == my_gen`, then the bump +
/// notify run, then the worker locks RESUME_MUTEX + waits and MISSES the wake — a
/// lost-wakeup that self-healed only via the 5 s `wait_for` timeout (a per-occurrence
/// stall that widened the hang window). Waiting on the gen-bump's OWN mutex makes the
/// predicate-check and the bump atomic w.r.t. each other: the worker either observes
/// the advanced gen (and never waits) or is guaranteed to be woken by
/// `end_rendezvous_cycle`'s `RENDEZVOUS_CONDVAR.notify_all()` (under the same mutex,
/// after the bump). NO timeout is required for correctness; the 5 s `wait_for` is kept
/// purely as a warn-and-recheck liveness backstop (the loom `mod loom_straddle` model
/// drops it to a plain `wait` so a lost-wakeup would show as a PERMANENT deadlock).
///
/// Mesa discipline (preserves the witness protocol): [`RENDEZVOUS_CONDVAR`] is ALSO the
/// condvar the driver's witness wait (`requestor_wait_for_all_reified_parked`) and the
/// parkers (`worker_park_and_root_in_cycle`) use. Mixing gen-resume-waiters onto it is
/// SAFE because every waiter re-checks its OWN predicate under the lock on each wake —
/// a parker's notify (or a witness-satisfied notify) that spuriously wakes a
/// resume-waiter is absorbed by its `while gen == my_gen` re-check, and vice versa. No
/// reentrancy: `worker_park_and_root_in_cycle` releases [`RENDEZVOUS_MUTEX`] (its stamp
/// block ends) BEFORE calling this. No new lock-order edge: this takes ONLY
/// [`RENDEZVOUS_MUTEX`] (the cross-mutex W2 RENDEZVOUS→RESUME notify that the old design
/// needed is thereby ELIMINATED).
///
/// Source-coupled by generation-gated worker park/resume.
#[allow(dead_code)]
pub(crate) fn worker_resume_wait_for_cycle(my_gen: u64) {
    let _site = GcWaitSiteGuard::enter(&GC_PARK_WAITERS);
    let mut lock = RENDEZVOUS_MUTEX.lock();
    // Bug #309 (defense-in-depth twin of the entry gate in
    // `worker_park_and_root_in_cycle`): wait only while the cycle `my_gen` is
    // REAL — already open (started == my_gen) or still pending (requested). A
    // phantom-future gen (the post-close TOCTOU read) would otherwise wait for
    // a gen bump that no requested cycle will ever produce. Re-evaluated on
    // every wakeup: a real parked cycle exits via the close's gen bump; a
    // pending one becomes open (started catches up) and then closes.
    while GC_CYCLE_GEN.load(Ordering::Acquire) == my_gen
        && (current_cycle_started() == my_gen || is_gc_requested())
    {
        let result = RENDEZVOUS_CONDVAR.wait_for(&mut lock, RENDEZVOUS_WAIT_TIMEOUT);
        if result.timed_out() && GC_CYCLE_GEN.load(Ordering::Acquire) == my_gen {
            tracing::warn!(
                "worker_resume_wait_for_cycle: still in cycle gen {} after {:?} — re-checking",
                my_gen,
                RENDEZVOUS_WAIT_TIMEOUT,
            );
        }
    }
}

/// WORKER side: park on [`RESUME_CONDVAR`] until the requestor clears
/// `GC_REQUESTED` (HB4), then return.
///
/// Step (4) of [`worker_park_and_root`], factored out so there is ONE park
/// implementation shared by BOTH (a) a worker that has already self-rooted at a
/// poll point (`worker_park_and_root` calls this as its last step) and (b) the
/// D2.1 **WorkerEnter gate** (a brand-new worker that has NOT yet joined the
/// active eval set, so it has no roots to contribute and parks directly here
/// until the in-flight rendezvous completes — the TLA+ `WorkerEnter`
/// `~gcRequested` admission guard, Risk R2).
///
/// Lost-wakeup-safe: [`RESUME_MUTEX`] is held across the `is_gc_requested()`
/// predicate check and the `wait_for`, matching [`resume_workers`], which holds
/// the SAME mutex across `GC_REQUESTED.store(false, Release)` + `notify_all()`.
/// The worker's resume Acquire on `is_gc_requested()` thus observes the
/// post-sweep state (HB4). The 5 s `wait_for` is a warn-and-recheck liveness
/// backstop (Risk R1), NOT a hard budget: the loop only exits when the real
/// predicate (`!is_gc_requested()`) holds.
///
/// Live for dedicated WorkerEnter gating and the legacy D2 park helper; the
/// attribute is for build/test configurations that do not enter those paths.
#[allow(dead_code)]
// ── E1 liveness diagnosis (Inc B): GC wait-site occupancy counters ──
// Lock-free occupancy of the three GC wait sites a parallel-collapse participant (a
// worker OR the collapse parent) can block in. The SIGUSR1 dump prints these so a hang's
// stranded site is identified WITHOUT a debugger (gdb-under-launch perturbs the timing
// race away; `ptrace_scope=1` blocks sibling-attach). A nonzero count at a hang with the
// GC idle names the permanently-blocked site: GATE = `worker_wait_for_resume`
// (WorkerEnter admission), PARK = `worker_resume_wait_for_cycle` (rendezvous gen-wait),
// STRADDLE = `reacquire_eval_guard_after_safepoint_full`. Always compiled (cheap
// atomics); written only on the index collector's wait paths.
pub(crate) static GC_GATE_WAITERS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GC_PARK_WAITERS: AtomicUsize = AtomicUsize::new(0);
// STRADDLE's only writer is the straddle loop in
// `reacquire_eval_guard_after_safepoint_full`, and its only reader is the index
// dump accessor — so it is index-gc-only (GATE/PARK are referenced by the
// always-compiled, slab-dead `worker_wait_for_resume`/`worker_resume_wait_for_cycle`).
pub(crate) static GC_STRADDLE_WAITERS: AtomicUsize = AtomicUsize::new(0);

/// RAII occupancy guard: increments a wait-site counter on entry, decrements on EVERY
/// exit path (normal return, loop break, panic-unwind) via `Drop`.
pub(crate) struct GcWaitSiteGuard(&'static AtomicUsize);
impl GcWaitSiteGuard {
    #[inline]
    pub(crate) fn enter(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        GcWaitSiteGuard(counter)
    }
}
impl Drop for GcWaitSiteGuard {
    #[inline]
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Snapshot `(gate, park, straddle)` GC wait-site occupancy for the diagnostic dump.
pub(crate) fn gc_wait_site_occupancy() -> (usize, usize, usize) {
    (
        GC_GATE_WAITERS.load(Ordering::Acquire),
        GC_PARK_WAITERS.load(Ordering::Acquire),
        GC_STRADDLE_WAITERS.load(Ordering::Acquire),
    )
}

pub(crate) fn worker_wait_for_resume() {
    let _site = GcWaitSiteGuard::enter(&GC_GATE_WAITERS);
    let mut lock = RESUME_MUTEX.lock();
    while is_gc_requested() {
        let result = RESUME_CONDVAR.wait_for(&mut lock, RENDEZVOUS_WAIT_TIMEOUT);
        if result.timed_out() && is_gc_requested() {
            tracing::warn!(
                "worker_wait_for_resume: still GC_REQUESTED after {:?} wait — re-checking",
                RENDEZVOUS_WAIT_TIMEOUT,
            );
            // Loop re-checks the real predicate; a timeout is a benign retry.
        }
    }
}

/// REQUESTOR side: block until every other evaluator has parked
/// (`active_evaluator_count()==0`).
///
/// The requestor MUST have already `drop_eval_guard_for_safepoint()`'d itself, so
/// `active==0` means "all OTHER workers parked" — the proven TLA+ `BeginMark`
/// predicate. Lost-wakeup-safe: [`RENDEZVOUS_MUTEX`] is held across the predicate
/// check and the `wait_for`, matching the worker's notify under the same mutex
/// (HB2). The 5 s timeout is a warn-and-recheck liveness backstop (Risk R1).
///
/// DEAD until D2.3. See `docs/cesk-gc/phase-d-d1-d2-rendezvous-design.md` §D1.
#[allow(dead_code)] // DEAD until D2.3 (requestor wiring).
pub(crate) fn requestor_wait_for_parked() {
    let mut lock = RENDEZVOUS_MUTEX.lock();
    while active_evaluator_count() != 0 {
        let result = RENDEZVOUS_CONDVAR.wait_for(&mut lock, RENDEZVOUS_WAIT_TIMEOUT);
        if result.timed_out() && active_evaluator_count() != 0 {
            tracing::warn!(
                "requestor_wait_for_parked: {} evaluator(s) still active after {:?} — re-checking",
                active_evaluator_count(),
                RENDEZVOUS_WAIT_TIMEOUT,
            );
            // Loop re-checks the real predicate; a timeout is a benign retry.
        }
    }
}

/// REQUESTOR side (legacy count gate): block until at least `n` workers have parked
/// (`WORKERS_PARKED_FOR_GC >= n`). Unlike [`requestor_wait_for_parked`] (which
/// gates on `active_evaluator_count()==0`), this gates on the per-THREAD parked
/// count — the count the dedicated-GC-thread driver snapshots as `n = n_threads()`
/// AFTER closing admission (the §Part-2 admission-before-snapshot). Single-location
/// Acquire/Release HB (the parker's `fetch_add(AcqRel)` in
/// [`worker_park_and_root_in_cycle`] release-fences its buffer append, HB2), so a
/// `>= n` observation sees at least `n` workers' roots — SC-faithful, no SeqCst.
/// The live driver now uses the per-slot reified witness gate instead of this
/// fungible count gate, but tests keep this helper to pin the older handshake.
///
#[allow(dead_code)]
pub(crate) fn requestor_wait_for_parked_count(n: u32) {
    let mut lock = RENDEZVOUS_MUTEX.lock();
    // Wait until AT LEAST `n` participants have bumped (`>= n`, i.e. loop while
    // `< n`) — NOT exactly `== n`. A worker EXCLUDED from the snapshot `n` (it left
    // the active set — via a parker's full-depth drain or a finisher's guard-drop —
    // before the driver's `n = n_threads()` read) can still bump the count (the bump
    // precedes the drop in program order), so the count may OVERSHOOT `n`. A condvar
    // wake can skip the transient `== n` (two bumps between wakes), so an `!= n`
    // predicate would hang on the overshoot. `< n` exits on `>= n`: an overshoot is
    // benign — the extra bump published valid roots and that worker has already left,
    // so the drained union is a sound (over-)approximation. (The count only ever
    // increases during a cycle; `end_rendezvous_cycle` zeroes it at the END.)
    while WORKERS_PARKED_FOR_GC.load(Ordering::Acquire) < n {
        let result = RENDEZVOUS_CONDVAR.wait_for(&mut lock, RENDEZVOUS_WAIT_TIMEOUT);
        if result.timed_out() && WORKERS_PARKED_FOR_GC.load(Ordering::Acquire) < n {
            tracing::warn!(
                "requestor_wait_for_parked_count: parked={} < n={} after {:?} — re-checking",
                WORKERS_PARKED_FOR_GC.load(Ordering::Acquire),
                n,
                RENDEZVOUS_WAIT_TIMEOUT,
            );
        }
    }
}

/// REQUESTOR side: move all worker-contributed roots out of the shared buffer
/// into `out` (which the requestor then unions with `E₀` + driver-C before
/// marking). Drains under the lock so it composes with concurrent worker appends
/// (none should be in flight once `active==0`, but the lock keeps it sound).
///
/// Live for dedicated-rendezvous root preparation; the attribute is for
/// build/test configurations that do not enter that path.
#[allow(dead_code)]
pub(crate) fn drain_worker_root_buffer(out: &mut Vec<MettaValue>) {
    out.extend(WORKER_ROOT_BUFFER.lock().drain(..));
}

/// REQUESTOR side: reset the per-cycle rendezvous counters/buffer to their
/// initial state (parked-count → 0, buffer cleared). Called by the requestor
/// after a cycle completes (or to recover after a backed-off attempt) so the
/// next rendezvous starts clean. `Release` on the counter store pairs with the
/// next cycle's Acquire reads.
///
/// Test/legacy helper. The live dedicated driver calls [`end_rendezvous_cycle`],
/// which performs this reset under the rendezvous mutex and bumps the generation.
#[allow(dead_code)]
pub(crate) fn reset_rendezvous_counters() {
    WORKERS_PARKED_FOR_GC.store(0, Ordering::Release);
    WORKER_ROOT_BUFFER.lock().clear();
}

/// REQUESTOR side (E1-c): END the current rendezvous cycle — bump [`GC_CYCLE_GEN`]
/// + reset the parked-count/buffer, ALL under [`RENDEZVOUS_MUTEX`], so the gen
/// advance (which releases parked workers' gen-gated [`worker_resume_wait_for_cycle`]
/// and excludes late stragglers in [`worker_park_and_root_in_cycle`]) is atomic
/// w.r.t. those parker critical sections. The dedicated GC thread calls this at
/// cycle END, BEFORE [`resume_workers`]'s notify (so the gen has advanced when a
/// woken worker re-checks). A SEPARATE fn from [`reset_rendezvous_counters`] (which
/// existing D2.x tests call without expecting the mutex/gen) to avoid a
/// reentrant-lock deadlock.
///
/// Source-coupled by dedicated driver cycle teardown.
#[allow(dead_code)]
pub(crate) fn end_rendezvous_cycle() {
    let _lock = RENDEZVOUS_MUTEX.lock();
    GC_CYCLE_GEN.fetch_add(1, Ordering::AcqRel);
    WORKERS_PARKED_FOR_GC.store(0, Ordering::Release);
    WORKER_ROOT_BUFFER.lock().clear();
    // E1-FLIP Path B V4: clear the witness-satisfied flag for the NEXT cycle, under the
    // SAME RENDEZVOUS_MUTEX as the gen-bump + buffer-clear so the {gen advance, witness
    // reset} is atomic w.r.t. a parker's gen-gated critical section. A `gate_open_rendezvous`
    // re-check by a woken-then-re-collecting path now reads `current_witness_ok()==false`
    // until the NEXT cycle's driver re-proves the witness — closing the cross-cycle window
    // where the prior cycle's `true` would falsely admit a sweep before the new wait.
    set_current_witness_ok(false);
    // ── E5 Mesa-correct gen-resume notify ──────────────────────────────────────
    // docs/cesk-gc/e1-flip-deadlock-straddle-rootcause-2026-06-03.md §Step 2. The gen
    // bump above is the RESUME CONDITION for `worker_resume_wait_for_cycle` (W2), which
    // now WAITS on `RENDEZVOUS_CONDVAR` holding THIS SAME `RENDEZVOUS_MUTEX` (keyed on
    // `GC_CYCLE_GEN != my_gen`). So the notify is issued HERE, under the lock we already
    // hold, AFTER the bump — making {bump, notify} atomic w.r.t. W2's `while gen==my {
    // wait }` predicate re-check on the same mutex (Mesa-correct: NO cross-mutex
    // lost-wakeup). This SUPERSEDES the prior W2-notify-to-RESUME_CONDVAR hardening: W2
    // no longer waits on RESUME_CONDVAR, so the cross-lock RENDEZVOUS→RESUME notify (and
    // its lock-order edge) is ELIMINATED. A spurious wake of the driver's witness wait
    // (also on RENDEZVOUS_CONDVAR) is impossible here — `end_rendezvous_cycle` runs at
    // teardown, AFTER the witness wait has already returned; even if it fired, that wait
    // re-checks its own predicate (Mesa). `resume_workers()` still notifies
    // RESUME_CONDVAR separately for `worker_wait_for_resume` (the WorkerEnter gate, which
    // keys on `GC_REQUESTED` under RESUME_MUTEX) — a DISTINCT waiter, unaffected.
    RENDEZVOUS_CONDVAR.notify_all();
}

/// REQUESTOR side: clear `GC_REQUESTED` and wake all parked workers, atomically
/// w.r.t. the worker park.
///
/// The lost-wakeup-safe handshake (HB4) requires that the resume CONDITION
/// (`GC_REQUESTED` going false) be published UNDER the same mutex that a parking
/// worker holds across its `while is_gc_requested() { wait }` loop — otherwise a
/// worker can read `GC_REQUESTED==true`, then the requestor clears+notifies, then
/// the worker locks+waits and misses the notification forever. So this function
/// holds [`RESUME_MUTEX`] across BOTH `GC_REQUESTED.store(false, Release)` (HB4:
/// the worker's resume Acquire on `is_gc_requested` then sees the post-sweep
/// store state) AND `RESUME_CONDVAR.notify_all()`.
///
/// DEVIATION from the literal D1.1 spec (which listed `resume_workers()` as just
/// `{ lock RESUME_MUTEX; notify_all() }` with the `GC_REQUESTED` clear done by
/// the requestor separately BEFORE the call): clearing the flag OUTSIDE the lock
/// reopens exactly the lost-wakeup window this pair is meant to close, so the
/// clear is folded INTO this function under the lock. The design doc §D1 requestor
/// sequence ("`GC_REQUESTED.store(false, Release)` → RESUME_CONDVAR.notify_all()")
/// is honored — both steps simply happen here, together, under RESUME_MUTEX. The
/// loom model `loom_rendezvous` verifies this is lost-wakeup-free.
///
/// Live for dedicated rendezvous teardown and for trigger-handoff failure
/// backstops. The attribute is only for build configurations whose current path
/// does not enter the dedicated collector.
#[allow(dead_code)]
pub(crate) fn resume_workers() {
    let _lock = RESUME_MUTEX.lock();
    GC_REQUESTED.store(false, Ordering::Release);
    RESUME_CONDVAR.notify_all();
}

// ----------------------------------------------------------------------------
// D1.1 unit test — rendezvous primitives in isolation (BOTH builds)
// ----------------------------------------------------------------------------
//
// Plain `#[cfg(test)]`: the rendezvous primitives are pure synchronization over
// `MettaValue`, and the Phase-D gate requires this test to run and add +1 to the
// test count. (During the slab/index migration it was deliberately NOT gated to
// the slab-only `mod tests` below, so it ran in both builds.)
#[cfg(test)]
mod rendezvous_d1_1_tests {
    use super::*;
    // `global_factory().long(..)` needs the factory trait in scope; it is a
    // build-agnostic way to mint a `MettaValue` (slab handle or index Addr).
    use super::super::metta_value_trait::MettaValueFactory;

    /// (a) `begin_gc_rendezvous` CAS exclusion: with two threads racing, EXACTLY
    /// one observes `true`. (b) park/notify with no lost wakeup: a worker thread
    /// parks via `worker_park_and_root`, the "requestor" sets then clears
    /// `GC_REQUESTED` via `resume_workers()`, and the worker resumes — verified by
    /// a join that must complete within a bounded timeout (a hang ⇒ lost wakeup ⇒
    /// the join times out ⇒ the test fails, never hangs the suite).
    ///
    /// Both sub-cases live in ONE `#[test]` so they run sequentially: they mutate
    /// PROCESS-GLOBAL statics (`GC_REQUESTED`, `GC_REQUESTOR_ACTIVE`,
    /// `WORKERS_PARKED_FOR_GC`, `WORKER_ROOT_BUFFER`), so concurrent test bodies
    /// would race on shared state. We reset that state up front.
    #[test]
    fn test_rendezvous_primitives_exclusion_and_park_resume() {
        use std::sync::atomic::{AtomicU32, Ordering as O};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        // Clean slate (other tests may have left these set; this test owns them
        // for its duration — it does not run concurrently with itself).
        GC_REQUESTED.store(false, O::Release);
        GC_REQUESTOR_ACTIVE.store(false, O::Release);
        reset_rendezvous_counters();

        // ---- (a) begin_gc_rendezvous mutual exclusion --------------------------
        // Two threads race to acquire; a Barrier maximizes the contention window.
        // Across many rounds, the count of `true` results is ALWAYS exactly 1.
        for _round in 0..64 {
            GC_REQUESTOR_ACTIVE.store(false, O::Release);
            // `start` maximizes the CAS-contention window; `settled` ensures BOTH
            // threads have recorded their CAS result BEFORE the winner releases —
            // otherwise the winner could `end_gc_rendezvous()` (reset the flag) so
            // fast that the "loser" then wins its own CAS too, and we would count
            // 2 winners for a property ("at most one owner at a time") that is in
            // fact upheld. Releasing only after `settled` makes the test measure
            // the true mutual-exclusion invariant.
            let start = Arc::new(std::sync::Barrier::new(2));
            let settled = Arc::new(std::sync::Barrier::new(2));
            let winners = Arc::new(AtomicU32::new(0));

            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let start = Arc::clone(&start);
                    let settled = Arc::clone(&settled);
                    let winners = Arc::clone(&winners);
                    std::thread::spawn(move || {
                        start.wait();
                        let won = begin_gc_rendezvous();
                        if won {
                            winners.fetch_add(1, O::AcqRel);
                        }
                        // Both threads have now CAS'd and recorded the outcome.
                        settled.wait();
                        // Now it is safe for the winner to release ownership.
                        if won {
                            end_gc_rendezvous();
                        }
                    })
                })
                .collect();
            for h in handles {
                h.join().expect("exclusion thread panicked");
            }
            assert_eq!(
                winners.load(O::Acquire),
                1,
                "exactly one thread may win begin_gc_rendezvous per round"
            );
        }
        // Leave ownership released for part (b).
        GC_REQUESTOR_ACTIVE.store(false, O::Release);

        // ---- (b) park / notify, no lost wakeup --------------------------------
        // A worker self-roots a sentinel and parks; the requestor (this thread)
        // sets GC_REQUESTED, waits for the worker to park, drains the buffer
        // (asserting the sentinel arrived — HB2), then resume_workers() clears the
        // flag + notifies (HB4). The worker must resume.
        reset_rendezvous_counters();

        // Requestor publishes the GC request FIRST (HB1), so when the worker
        // reaches its park loop `is_gc_requested()` is already true and it parks.
        GC_REQUESTED.store(true, O::Release);

        let sentinel: i64 = 0x5EED_BEEF;
        let worker = std::thread::spawn(move || {
            // Worker self-collects its structural root(s); here a single sentinel.
            let my_root = global_factory().long(sentinel);
            worker_park_and_root(&[my_root]);
            // If we get here, the park observed `!is_gc_requested()` and returned.
            true
        });

        // Wait for the worker to actually park (WORKERS_PARKED_FOR_GC reaches 1).
        // `requestor_wait_for_parked()` itself keys off `active_evaluator_count()`,
        // which this unit test does not drive (no EvalGuard), so we spin on the
        // parked-count directly — the buffer-HB carrier — with a bounded deadline.
        let park_deadline = Instant::now() + Duration::from_secs(10);
        while WORKERS_PARKED_FOR_GC.load(O::Acquire) < 1 {
            assert!(
                Instant::now() < park_deadline,
                "worker did not park within deadline (parked-count never reached 1)"
            );
            std::thread::yield_now();
        }

        // HB2: the worker's root must be visible in the shared buffer now.
        let mut drained: Vec<MettaValue> = Vec::new();
        drain_worker_root_buffer(&mut drained);
        assert!(
            drained.iter().any(|r| r.as_long() == Some(sentinel)),
            "worker-contributed root (sentinel) must be visible after parking (HB2)"
        );

        // Resume: clear GC_REQUESTED + notify, lost-wakeup-safe (HB4).
        resume_workers();

        // The worker MUST resume. Bounded join (poll `is_finished`) so a lost
        // wakeup surfaces as a test FAILURE, not a hung suite.
        let join_deadline = Instant::now() + Duration::from_secs(10);
        while !worker.is_finished() {
            assert!(
                Instant::now() < join_deadline,
                "worker did not resume after resume_workers() — possible lost wakeup"
            );
            std::thread::yield_now();
        }
        assert!(
            worker.join().expect("worker thread panicked"),
            "worker_park_and_root must return once GC_REQUESTED is cleared"
        );

        // Cleanup so we leave the globals pristine for any sibling tests.
        reset_rendezvous_counters();
        GC_REQUESTED.store(false, O::Release);
        GC_REQUESTOR_ACTIVE.store(false, O::Release);
    }

    // ====================================================================
    // E1-FLIP Path B V4 — witness directory unit tests (Step 0 + Step 1)
    // ====================================================================
    //
    // The witness slot is THREAD-LOCAL (`MY_WITNESS_SLOT`), so each test runs its
    // mutator role on a FRESHLY SPAWNED thread (a fresh thread starts with a null
    // slot and acquires its own grow-only slot). `GC_CYCLE_GEN` is process-global;
    // these tests drive it explicitly and reset it at the end.

    /// Step 0: a NON-reified bump does NOT satisfy the witness. A thread acquires a
    /// slot for gen K, then `worker_finish_into_buffer(&[], K)` (a finisher bump) —
    /// the SOLE non-stamp path — leaves `published` UNSTAMPED (still K-1), so the
    /// strict-`>` predicate reads "wait". Only `note_reified_park(K)` stamps it.
    #[test]
    fn test_witness_non_reified_bump_does_not_satisfy() {
        use std::sync::atomic::Ordering as O;
        use std::time::{Duration, Instant};

        // Drive the cycle gen to a known value K.
        let k = current_cycle_gen();

        // Run the mutator role on a fresh thread (fresh thread-local slot). It
        // acquires, finish-bumps (NON-reified), and reports its slot pointer.
        let (tx, rx) = std::sync::mpsc::channel::<usize>();
        let worker = std::thread::spawn(move || {
            witness_acquire_slot(); // occupied=true, acquired=K, published=K-1
            let slot_ptr = MY_WITNESS_SLOT.with(|c| c.get());
            // A finisher bump for gen K — must NOT stamp `published`.
            worker_finish_into_buffer(&[], k);
            tx.send(slot_ptr as usize).expect("send slot ptr");
            // Park briefly so the slot stays occupied while the main thread asserts.
            std::thread::sleep(Duration::from_millis(50));
            // Release the slot before exit.
            witness_release_slot();
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let slot_ptr_usize = loop {
            if let Ok(p) = rx.recv_timeout(Duration::from_millis(100)) {
                break p;
            }
            assert!(Instant::now() < deadline, "worker never reported its slot");
        };
        let slot = unsafe { &*(slot_ptr_usize as *const WitnessSlot) };

        // After a NON-reified finisher bump: occupied, acquired==K, published==K-1.
        assert!(slot.occupied.load(O::Acquire), "slot should be occupied");
        assert_eq!(slot.acquired_gen.load(O::Acquire), k, "acquired should be K");
        assert_eq!(
            slot.published_gen.load(O::Acquire),
            k.wrapping_sub(1),
            "a finisher bump must NOT stamp published (the SOLE stamp is note_reified_park)"
        );
        // Strict-`>` predicate for cur_gen=K: NOT satisfied (published K-1 < K; acquired K !> K).
        assert!(
            !witness_slot_satisfied(slot, k),
            "non-reified bump must leave the witness UNSATISFIED for cur_gen=K"
        );

        worker.join().expect("worker panicked");
        // GC_CYCLE_GEN is left as-is (we never bumped it).
        let _ = k;
    }

    /// Step 0/1: `note_reified_park(K)` IS the stamp — after it, the slot satisfies
    /// the strict-`>` predicate for cur_gen=K. Complements the negative test above.
    #[test]
    fn test_witness_reified_park_satisfies() {
        use std::sync::atomic::Ordering as O;
        use std::time::{Duration, Instant};

        let k = current_cycle_gen();
        let (tx, rx) = std::sync::mpsc::channel::<usize>();
        let worker = std::thread::spawn(move || {
            witness_acquire_slot();
            let slot_ptr = MY_WITNESS_SLOT.with(|c| c.get());
            // The genuine reified-park stamp.
            note_reified_park(k);
            tx.send(slot_ptr as usize).expect("send slot ptr");
            std::thread::sleep(Duration::from_millis(50));
            witness_release_slot();
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let slot_ptr_usize = loop {
            if let Ok(p) = rx.recv_timeout(Duration::from_millis(100)) {
                break p;
            }
            assert!(Instant::now() < deadline, "worker never reported its slot");
        };
        let slot = unsafe { &*(slot_ptr_usize as *const WitnessSlot) };
        assert_eq!(
            slot.published_gen.load(O::Acquire),
            k,
            "note_reified_park(K) must stamp published=K"
        );
        assert!(
            witness_slot_satisfied(slot, k),
            "a reified park must SATISFY the witness for cur_gen=K"
        );
        worker.join().expect("worker panicked");
    }

    /// Step 1 STRADDLE (single re-park): a thread parks for cycle K, then — its slot
    /// STILL OCCUPIED (V4 decoupling: no release at the park) — re-stamps + re-parks
    /// for cycle K+1. Assert: across the K→K+1 seam the slot stays OCCUPIED, and the
    /// driver's strict-`>` predicate reads "wait" for K+1 until the re-park stamps
    /// K+1, then "satisfied". Drives the slot lifecycle the re-park loop performs.
    #[test]
    fn test_witness_straddle_park_k_then_repark_k_plus_1() {
        use std::sync::atomic::Ordering as O;

        // Use an isolated thread so the slot is fresh; drive gens by hand.
        let slot_ptr_usize = std::thread::spawn(|| {
            let start = current_cycle_gen();
            // Acquire for K (=start). occupied, acquired=K, published=K-1.
            witness_acquire_slot();
            let slot_ptr = MY_WITNESS_SLOT.with(|c| c.get());
            let slot = unsafe { &*slot_ptr };

            // Park for K: stamp published=K.
            note_reified_park(start);
            assert!(slot.occupied.load(O::Acquire));
            assert!(
                witness_slot_satisfied(slot, start),
                "K-park satisfies cur_gen=K"
            );

            // --- the seam: cycle advances to K+1 (the driver bumped GC_CYCLE_GEN) ---
            // V4: the slot is NOT released here — it stays OCCUPIED across the seam.
            let next = start.wrapping_add(1);
            // Before the re-park, the slot is occupied + acquired=K + published=K:
            // for cur_gen=K+1 the predicate must read WAIT (published K !>= K+1;
            // acquired K !> K+1).
            assert!(slot.occupied.load(O::Acquire), "slot stays occupied at seam");
            assert!(
                !witness_slot_satisfied(slot, next),
                "an un-re-parked straddler must read WAIT for K+1"
            );

            // The straddle re-park: restamp acquired=K+1, then stamp published=K+1.
            witness_restamp_acquired(next);
            note_reified_park(next);
            assert!(
                witness_slot_satisfied(slot, next),
                "after re-park the straddler satisfies cur_gen=K+1"
            );

            let ptr = slot_ptr as usize;
            witness_release_slot();
            ptr
        })
        .join()
        .expect("straddle worker panicked");

        // After release the slot is unoccupied (so a later snapshot excludes it).
        let slot = unsafe { &*(slot_ptr_usize as *const WitnessSlot) };
        assert!(
            !slot.occupied.load(O::Acquire),
            "outermost release must un-occupy the slot"
        );
    }

    /// Step 1 BACK-TO-BACK STRADDLE: a thread parks for K, then BEFORE it would
    /// rejoin, cycles K+1 AND K+2 fire in succession. Assert the slot stays
    /// OCCUPIED across BOTH seams (no window where it is occupied-but-unsatisfied for
    /// a cycle it has not yet stamped, and never unoccupied-while-live), and each
    /// cycle's predicate flips to satisfied only after that cycle's re-park stamp.
    /// This is the v3 attack-#3 (back-to-back) coverage.
    #[test]
    fn test_witness_back_to_back_straddle() {
        use std::sync::atomic::Ordering as O;

        std::thread::spawn(|| {
            let k = current_cycle_gen();
            witness_acquire_slot();
            let slot = unsafe { &*MY_WITNESS_SLOT.with(|c| c.get()) };

            // Park@K.
            note_reified_park(k);
            assert!(witness_slot_satisfied(slot, k));

            // Two intervening cycles, no release between them (V4 continuous occupancy).
            for step in 1..=2u64 {
                let g = k.wrapping_add(step);
                // At the seam, before this cycle's re-park: occupied, but unsatisfied
                // for g (published is the PREVIOUS gen).
                assert!(slot.occupied.load(O::Acquire), "occupied across seam {step}");
                assert!(
                    !witness_slot_satisfied(slot, g),
                    "straddler must read WAIT for cycle {g} until it re-parks"
                );
                // The re-park for cycle g.
                witness_restamp_acquired(g);
                note_reified_park(g);
                assert!(
                    witness_slot_satisfied(slot, g),
                    "after re-park the straddler satisfies cycle {g}"
                );
            }
            witness_release_slot();
        })
        .join()
        .expect("back-to-back straddle worker panicked");
    }
}

/// Set when a GC snapshot is sent to the GC thread, cleared when the response
/// is processed. Prevents queueing multiple snapshots in the mpsc channel.
///
/// Aligns with TLA+ model's `~hasGcRequest /\ gcPhase = "idle"` preconditions
/// on `TryQuiescentGc_AcquireFlag`. Without this, `maybe_quiescent_gc()` could
/// send multiple snapshots while the GC thread is still processing a previous
/// one, wasting memory and CPU on redundant GC cycles.
static GC_CYCLE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Monotonic counter incremented each time a GC sweep frees slab slots.
///
/// Thread-local caches that hold `MettaValue` references (e.g., `EVAL_MEMO`)
/// or pointer-keyed entries (e.g., `NORMAL_FORM_BLOOM`) compare their local
/// epoch against this counter. When they diverge, the cache is stale and must
/// be cleared before use — slab slots may have been freed and reused (ABA).
static GC_SWEEP_EPOCH: AtomicU64 = AtomicU64::new(0);


// ============================================================================
// Session GC Statistics — counters for diagnosing memory growth
// ============================================================================

/// Total number of sessions released (incremented in release_session_with_surviving)
#[cfg(feature = "track-stats")]
static SESSION_RELEASES_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Total values freed across all session releases
#[cfg(feature = "track-stats")]
static SESSION_VALUES_FREED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Total values promoted (context_id → 0) across all session releases
#[cfg(feature = "track-stats")]
static SESSION_VALUES_PROMOTED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Total values scanned during session releases
#[cfg(feature = "track-stats")]
static SESSION_VALUES_SCANNED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Size of the last surviving set (root trace result)
#[cfg(feature = "track-stats")]
static LAST_SURVIVING_SET_SIZE: AtomicU64 = AtomicU64::new(0);

/// Public accessor for session GC stats (used by diagnostics module).
#[cfg(feature = "track-stats")]
pub fn session_gc_stats() -> SessionGcStats {
    SessionGcStats {
        releases_total: SESSION_RELEASES_TOTAL.load(Ordering::Relaxed),
        values_freed_total: SESSION_VALUES_FREED_TOTAL.load(Ordering::Relaxed),
        values_promoted_total: SESSION_VALUES_PROMOTED_TOTAL.load(Ordering::Relaxed),
        values_scanned_total: SESSION_VALUES_SCANNED_TOTAL.load(Ordering::Relaxed),
        last_surviving_set_size: LAST_SURVIVING_SET_SIZE.load(Ordering::Relaxed),
    }
}

/// Session GC statistics snapshot.
pub struct SessionGcStats {
    pub releases_total: u64,
    pub values_freed_total: u64,
    pub values_promoted_total: u64,
    pub values_scanned_total: u64,
    pub last_surviving_set_size: u64,
}

/// RAII guard that tracks active evaluators for quiescent-state GC.
///
/// When an `EvalGuard` is alive, GC snapshot building is inhibited (the guard
/// increments `ACTIVE_EVALUATORS` on creation and decrements on drop). GC can
/// only trigger at quiescent points when all guards have been dropped.
///
/// The guard also blocks briefly (condvar park) if a GC snapshot is currently
/// being built (`GC_IN_PROGRESS`), ensuring the snapshot sees a consistent
/// root set.
pub struct EvalGuard;

impl EvalGuard {
    /// Enter an evaluation — blocks briefly if GC snapshot is in progress.
    ///
    /// Increments `ACTIVE_EVALUATORS` and checks `GC_IN_PROGRESS`. If a
    /// snapshot is being built, backs off and parks on a condvar to prevent
    /// a TOCTOU race where a new eval starts between the quiescent check
    /// and snapshot capture.
    ///
    /// Also increments the thread-local `EVAL_GUARD_DEPTH` counter, used by
    /// safepoint drop/reacquire to track guard nesting.
    #[inline]
    pub fn enter() -> Self {
        /// Maximum time to wait for GC_IN_PROGRESS to clear before retrying.
        /// Prevents permanent deadlock if the GC guard is never dropped
        /// (e.g., thread killed, panic in non-unwind context).
        const GC_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

        loop {
            ACTIVE_EVALUATORS.fetch_add(1, Ordering::AcqRel);
            if !GC_IN_PROGRESS.load(Ordering::Acquire) {
                break; // Fast path: no GC in progress (common case)
            }
            // GC snapshot in progress — back off and park
            ACTIVE_EVALUATORS.fetch_sub(1, Ordering::AcqRel);
            // Double-checked locking: park on condvar instead of spinning.
            // The mutex prevents lost wakeups (see GcInProgressGuard::drop).
            let mut lock = GC_PROGRESS_MUTEX.lock();
            while GC_IN_PROGRESS.load(Ordering::Acquire) {
                let result = GC_PROGRESS_CONDVAR.wait_for(&mut lock, GC_WAIT_TIMEOUT);
                if result.timed_out() && GC_IN_PROGRESS.load(Ordering::Acquire) {
                    tracing::warn!(
                        "EvalGuard::enter(): GC_IN_PROGRESS still set after {:?} wait — retrying",
                        GC_WAIT_TIMEOUT,
                    );
                    break; // Break inner loop to retry outer loop
                }
            }
            drop(lock);
        }
        EVAL_GUARD_DEPTH.with(|d| {
            let prev = d.get();
            d.set(prev + 1);
            // §1.2: count this THREAD once, on the OUTERMOST enter only. Placed
            // AFTER the admission loop, so only a thread that has cleared
            // GC_IN_PROGRESS joins the active-thread set (the Phase D+E
            // dedicated-GC-thread driver's `n_threads()` rendezvous gate).
            if prev == 0 {
                // ── E1-FLIP Path B V4 — slot ACQUIRE (RT-1) ──
                // OCCUPY this thread's witness slot BEFORE the N_THREADS.fetch_add is
                // observable, so EVERY thread the driver counts in its post-admission
                // `n` snapshot is also present (occupied) in the witness snapshot. The
                // slot stays occupied continuously from here to the OUTERMOST drop
                // (across parks) — DECOUPLED from N_THREADS (which releases at a park).
                // BYTE-IDENTICAL WHEN DORMANT: the `dedicated_gc_enabled()` gate.
                {
                    if dedicated_gc_enabled() {
                        witness_acquire_slot();
                    }
                }
                N_THREADS.fetch_add(1, Ordering::AcqRel);
            }
        });
        EvalGuard
    }
}

impl Drop for EvalGuard {
    #[inline]
    fn drop(&mut self) {
        EVAL_GUARD_DEPTH.with(|d| {
            let depth = d.get();
            if depth > 0 {
                d.set(depth - 1);
                // §1.2: leave the active-thread set on the OUTERMOST drop only.
                if depth == 1 {
                    N_THREADS.fetch_sub(1, Ordering::AcqRel);
                    // E1-FLIP §Part 3: if this thread is leaving the active set while a
                    // rendezvous cycle is in flight and it has NOT yet balanced the
                    // parked-count (panic / BranchCancelled / any non-finisher exit),
                    // finish-bump NOW with ZERO roots so the driver's
                    // requestor_wait_for_parked_count(n) does not stall on the 5 s
                    // backstop. Zero roots is correct: a dying worker's in-flight value
                    // is dead (never stored to results[slot]); a NORMALLY-finishing
                    // worker already bumped via the Ok-path finisher and is suppressed by
                    // already_bumped_this_cycle. Cheap (gated dedicated + one
                    // is_gc_requested() load, only on depth 1→0). worker_finish_into_buffer
                    // locks a parking_lot mutex (no poison, unwind-safe) + only infallible
                    // ops, so it is safe during a panic-unwind drop (no double-panic).
                    // BYTE-IDENTICAL WHEN DORMANT: the `dedicated_gc_enabled()` gate.
                    {
                        if dedicated_gc_enabled() && is_gc_requested() {
                            let my_gen = current_cycle_gen();
                            if !already_bumped_this_cycle(my_gen) {
                                worker_finish_into_buffer(&[], my_gen);
                            }
                        }
                    }
                    // ── E1-FLIP Path B V4 — slot RELEASE (the ONLY un-occupy site) ──
                    // AFTER the N_THREADS.fetch_sub. This thread's live machine is gone
                    // (true outermost drop), so un-occupy its witness slot. A safepoint
                    // (park) drop does NOT release — the frozen machine is still live —
                    // so this is reached ONLY at the genuine end of the thread's
                    // evaluation. SLAB-BYTE-IDENTICAL: cfg + index-mode gate.
                    {
                        if dedicated_gc_enabled() {
                            witness_release_slot();
                        }
                    }
                }
            }
        });
        let prev = ACTIVE_EVALUATORS.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            // Transitioned to quiescent state (0 active evaluators).
            // Notify session release thread waiting for safe root tracing.
            // Must hold mutex to prevent lost wakeups: a thread that checked
            // ACTIVE_EVALUATORS > 0 and is about to wait() must see our notify.
            let _lock = QUIESCENT_MUTEX.lock();
            QUIESCENT_CONDVAR.notify_all();
        }
    }
}

/// Get the current active evaluator count (for testing and diagnostics).
pub fn active_evaluator_count() -> u32 {
    ACTIVE_EVALUATORS.load(Ordering::Acquire)
}

/// Number of distinct mutator threads currently inside ≥1 `EvalGuard` (each
/// counted ONCE, regardless of guard nesting). This is the Phase D+E
/// dedicated-GC-thread driver's rendezvous gate target
/// (`WORKERS_PARKED_FOR_GC == n_threads()`). See [`N_THREADS`].
pub fn n_threads() -> u32 {
    N_THREADS.load(Ordering::Acquire)
}

/// Latch the [`WORKER_EVER_SPAWNED`] flag. Called at the eval-worker spawn sites
/// (`parallel_dispatch` / `parallel_collapse_dispatch`) BEFORE the worker is
/// handed to the pool, so that once any worker exists the single-threaded index
/// GC gate (`worker_ever_spawned()`) reports `true` and the collector backs off.
#[inline]
pub fn note_worker_spawned() {
    WORKER_EVER_SPAWNED.store(true, Ordering::Release);
}

/// `true` once any eval worker has EVER been spawned (sticky). The provable
/// single-threaded-quiescence gate for the Inc-6 index GC — see
/// [`WORKER_EVER_SPAWNED`].
#[inline]
pub fn worker_ever_spawned() -> bool {
    WORKER_EVER_SPAWNED.load(Ordering::Acquire)
}

/// Counter of session-release inhibitors held by the runtime.
///
/// **H10 dual-counter protocol**: This counter gates ONLY session-release GC,
/// not mark-sweep collection. `GcHoldGuard` increments this counter (instead
/// of `ACTIVE_EVALUATORS`) so that mark-sweep can fire during long top-level
/// expressions while still protecting result values from session-release
/// sweeps that would invalidate thread-local caches.
///
/// Read by session-release sweeps to defer them.
/// Mark-sweep paths (`maybe_quiescent_gc`, `safepoint_wait_for_quiescence`)
/// do NOT consult this counter — they only check `ACTIVE_EVALUATORS`.
pub(super) static SESSION_RELEASE_INHIBITORS: AtomicU32 = AtomicU32::new(0);

/// Notified when `SESSION_RELEASE_INHIBITORS` transitions to 0.
pub(super) static INHIBITOR_MUTEX: Mutex<()> = Mutex::new(());
pub(super) static INHIBITOR_CONDVAR: Condvar = Condvar::new();

/// Lightweight RAII guard that inhibits **session-release** GC, allowing
/// mark-sweep collection to continue. Unlike `EvalGuard`, does NOT touch
/// `EVAL_GUARD_DEPTH` (not an actual eval) and does NOT increment
/// `ACTIVE_EVALUATORS` (does not block mark-sweep).
///
/// **H10 fix (2026-05-05)**: Previously this guard incremented
/// `ACTIVE_EVALUATORS`, conflating two distinct GC paths. For long-running
/// PLN top-level expressions this caused the global counter to never drop
/// to zero, blocking ALL GC for tens of seconds and producing OOM/hang
/// (slab grew to 778× threshold). The fix decouples the counters:
/// `GcHoldGuard` now increments `SESSION_RELEASE_INHIBITORS` only.
/// `execute_session_release` waits for BOTH `ACTIVE_EVALUATORS == 0`
/// AND `SESSION_RELEASE_INHIBITORS == 0`. Mark-sweep is unaffected.
///
/// Use this to protect result values between `eval()` returning and
/// result formatting/consumption.
pub struct GcHoldGuard;

impl GcHoldGuard {
    /// Increment `SESSION_RELEASE_INHIBITORS`. No GC_IN_PROGRESS interlock
    /// is needed because this guard does not block mark-sweep.
    #[inline]
    pub fn enter() -> Self {
        SESSION_RELEASE_INHIBITORS.fetch_add(1, Ordering::AcqRel);
        GcHoldGuard
    }
}

impl Drop for GcHoldGuard {
    #[inline]
    fn drop(&mut self) {
        let prev = SESSION_RELEASE_INHIBITORS.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            let _lock = INHIBITOR_MUTEX.lock();
            INHIBITOR_CONDVAR.notify_all();
        }
    }
}

/// Read-only accessor for `SESSION_RELEASE_INHIBITORS` (for diagnostics).
#[inline]
pub fn session_release_inhibitors() -> u32 {
    SESSION_RELEASE_INHIBITORS.load(Ordering::Acquire)
}

/// RAII guard that sets `GC_IN_PROGRESS = true` on creation and clears it on drop.
/// Ensures the flag is always cleared, even if the GC snapshot path panics.
///
/// `pub(crate)` (Phase D+E E1-a.2): the index-gc collector
/// (`backend::eval::cesk::index_heap`) holds this across its mark+sweep so a
/// concurrent `EvalGuard::enter` / `reacquire_eval_guard_after_safepoint`
/// (mid-spawn admission) parks until the sweep completes — the dedicated-GC-thread
/// driver's admission handshake.
pub(crate) struct GcInProgressGuard;

impl GcInProgressGuard {
    #[allow(dead_code)]
    pub(super) fn enter() -> Self {
        GC_IN_PROGRESS.store(true, Ordering::Release);
        GcInProgressGuard
    }

    /// Try to enter GC-in-progress state. Returns None if another thread
    /// already holds the guard (e.g., maybe_quiescent_gc() or another
    /// session release cycle).
    pub(crate) fn try_enter() -> Option<Self> {
        if GC_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            Some(GcInProgressGuard)
        } else {
            None
        }
    }
}

impl Drop for GcInProgressGuard {
    fn drop(&mut self) {
        // Must hold mutex when clearing flag to prevent lost wakeups:
        // a thread that checked GC_IN_PROGRESS=true under the mutex and is
        // about to call wait() would miss a notify_all without this.
        {
            let _lock = GC_PROGRESS_MUTEX.lock();
            GC_IN_PROGRESS.store(false, Ordering::Release);
        }
        GC_PROGRESS_CONDVAR.notify_all();
    }
}

/// Check if a GC cycle is currently in flight (snapshot sent, response not processed).
pub fn gc_cycle_in_flight() -> bool {
    GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire)
}

/// Return the current GC sweep epoch.
///
/// Thread-local caches compare their local epoch against this value to detect
/// staleness after a GC cycle frees slab slots.
#[inline]
pub fn gc_sweep_epoch() -> u64 {
    GC_SWEEP_EPOCH.load(Ordering::Acquire)
}

/// Increment the GC sweep epoch after dead slab/index slots have been freed.
///
/// Thread-local caches that store or key by slab pointers / index `Addr`s compare
/// their local epoch against this value and self-invalidate before the next lookup.
/// This is required for work-pool threads that were idle or outside their own
/// safepoint while another thread completed a GC sweep.
///
/// `pub(crate)` (not `pub(super)`) so the index collector (`cesk::index_heap`) can
/// bump it at its dedicated-cycle sweep too — restoring exact slab parity for the
/// cross-thread lazy self-heal (the dedicated GC thread is NOT the threads that
/// hold the stale `VALUE_HASH_CACHE` / MORK / hash-cons entries, so an eager clear
/// on the sweeping thread cannot reach them; the epoch bump does, lazily on read).
#[inline]
pub(crate) fn bump_gc_sweep_epoch() {
    GC_SWEEP_EPOCH.fetch_add(1, Ordering::AcqRel);
}

// ============================================================================
// Backpressure — Graduated Allocation Throttling
// ============================================================================
//
// When allocation rate outpaces GC, the cron monitor (gc_cron.rs) sets a
// backpressure level (0..3) based on committed_bytes / gc_threshold ratio:
//
//   Level 0: committed < threshold          — no throttling
//   Level 1: committed >= threshold         — yield (Tier 1)
//   Level 2: committed >= threshold * 1.5   — sleep(10μs) (Tier 1)
//   Level 3: committed >= threshold * 2.0   — sleep(100μs) (Tier 1)
//                                             + block until GC completes (Tier 2)
//
// Two tiers of application:
//   Tier 1 (apply_backpressure_tier1): Called from SessionContext::maybe_gc()
//     every 256 trampoline iterations. Only active when gc_cycle_in_flight(),
//     otherwise a no-op. Yields or sleeps to slow allocation.
//   Tier 2 (apply_backpressure_tier2): Called from main.rs between top-level
//     expressions. At MAX level, spin-yields until gc_cycle_in_flight() is false.
//
// The cron monitor recomputes the level every ~100ms. ProcessGcResponse also
// immediately re-evaluates backpressure for faster feedback after GC.
//
// Modeled in TLA+ (SlabGC_Quiescent.tla) as:
//   - CronMonitorPoll sets backpressureLevel based on committed/threshold
//   - ContinueEval is guarded by ~(backpressureLevel >= MAX_BP /\ GcCycleInFlight)
//   - ProcessGcResponse decrements backpressureLevel by 1
//   - BackpressureEventuallyRelaxes liveness property verified

/// Current backpressure level (0..MAX_BACKPRESSURE). Set by cron monitor,
/// read by eval threads. Relaxed ordering suffices since this is advisory.
static BACKPRESSURE_LEVEL: AtomicU8 = AtomicU8::new(0);

/// Heartbeat counter incremented each time GC lifecycle is reached
/// (`maybe_quiescent_gc()` or `maybe_process_gc_response()` called).
/// The cron monitor reads this to avoid escalating backpressure when
/// GC is unreachable (no quiescent points being hit), which would cause
/// permanent throttling in library/test code.
static GC_REACHABLE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Maximum backpressure level. At this level, Tier 2 blocks until GC completes.
pub const MAX_BACKPRESSURE: u8 = 3;

/// Get the current backpressure level (0 = none, 3 = maximum).
#[inline]
pub fn backpressure_level() -> u8 {
    BACKPRESSURE_LEVEL.load(Ordering::Relaxed)
}

/// Set the backpressure level (clamped to MAX_BACKPRESSURE).
/// Called from gc_cron.rs execute_memory_monitor() and from
/// maybe_process_gc_response() for faster feedback.
#[inline]
pub fn set_backpressure_level(level: u8) {
    BACKPRESSURE_LEVEL.store(level.min(MAX_BACKPRESSURE), Ordering::Relaxed);
}

/// Bump the GC reachability heartbeat counter.
///
/// Called from `maybe_quiescent_gc()` and `maybe_process_gc_response()` to
/// signal that the GC lifecycle is reachable. The cron monitor uses this to
/// avoid escalating backpressure when GC is unreachable.
#[inline]
fn bump_gc_reachable() {
    GC_REACHABLE_COUNTER.fetch_add(1, Ordering::Relaxed);
}

/// Get the current GC reachability heartbeat counter.
///
/// Read by the cron monitor to determine if GC lifecycle code is being
/// reached. If this counter hasn't advanced between polls, backpressure
/// is not escalated (would cause permanent throttling).
#[inline]
pub fn gc_reachable_counter() -> u64 {
    GC_REACHABLE_COUNTER.load(Ordering::Relaxed)
}

/// Cumulative count of values freed by GC (mark-sweep + session release).
///
/// Incremented in `process_gc_response()` by the number of non-filtered
/// dead values. Used by the GC scaling monitor in `gc_cron.rs` to compute
/// the alloc/free rate ratio for adaptive pool sizing.
static GC_VALUES_FREED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Get the total number of values freed by GC across all cycles.
///
/// Read by the GC cron hill climber to compute free rate for
/// alloc_rate / free_rate adaptive scaling.
#[inline]
pub fn gc_values_freed_total() -> u64 {
    GC_VALUES_FREED_TOTAL.load(Ordering::Relaxed)
}

/// Tier 1 backpressure: graduated yield/sleep during eval.
///
/// Called from `SessionContext::maybe_gc()` every 256 trampoline iterations.
/// Only active when a GC cycle is in flight — if no GC is running, sleeping
/// is pointless (nothing to wait for). This prevents the test-path livelock
/// where cron sets backpressure to MAX but GC never fires because nobody
/// calls `maybe_quiescent_gc()` in tests.
///
/// At level 0, this is a no-op (zero overhead on the hot path).
#[inline]
pub fn apply_backpressure_tier1() {
    // Only apply backpressure when GC is actively running.
    if !gc_cycle_in_flight() {
        return;
    }

    // Phase 9.5: skip-when-progressing. If the previous GC cycle has been
    // freeing memory since this thread's last visit, GC is making forward
    // progress — skip the sleep entirely so the mutator stays at full
    // speed. Only block when GC is stuck (freed-total static).
    //
    // Threshold: 500ms of GC stagnation falls back to the level-2/3 sleep
    // ladder so we don't burn CPU on a truly stuck GC. The check uses a
    // thread-local last-seen counter, so reads are zero-contention.
    use std::cell::Cell;
    use std::time::Instant;
    thread_local! {
        static LAST_FREED_TOTAL: Cell<u64> = const { Cell::new(0) };
        static LAST_PROGRESS_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    }

    let current_freed = gc_values_freed_total();
    let made_progress = LAST_FREED_TOTAL.with(|c| {
        let last = c.get();
        if current_freed > last {
            c.set(current_freed);
            LAST_PROGRESS_AT.with(|t| t.set(Some(Instant::now())));
            true
        } else {
            false
        }
    });

    if made_progress {
        // GC is freeing memory — don't slow the mutator.
        return;
    }

    // GC has not freed memory since our last check. If the stagnation is
    // brief (< 500ms), still skip — the cycle is in flight and progress
    // is imminent. Beyond 500ms, fall back to the level-based sleep
    // ladder to avoid burning CPU on a truly stuck collector.
    let stagnant = LAST_PROGRESS_AT.with(|t| match t.get() {
        Some(instant) => instant.elapsed() > Duration::from_millis(500),
        None => {
            // First-call: seed the timestamp and skip the sleep this once.
            t.set(Some(Instant::now()));
            false
        }
    });

    if !stagnant {
        return;
    }

    match backpressure_level() {
        0 => {} // No backpressure — hot path, zero overhead
        1 => thread::yield_now(),
        2 => thread::sleep(Duration::from_micros(10)),
        _ => thread::sleep(Duration::from_micros(100)),
    }
}

/// Tier 2 backpressure entry point. Under the purely-async GC mandate this is
/// a no-op (see the body): the trampoline and the main thread must never block
/// on GC progress. Retained as a stable public entry point.
#[inline]
pub fn apply_backpressure_tier2() {
    // Phase 9.6: by the purely-async GC mandate, the trampoline thread
    // (and main thread between top-level expressions) must NOT block on
    // GC progress. This function is now a no-op. The original condvar
    // park at MAX backpressure is replaced by reliance on:
    //   1. `apply_backpressure_tier1` (skip-when-progressing) for inline
    //      slowdown on stuck collectors.
    //   2. The memory-pressure workpool's USL/Lyapunov controller
    //      (`[[memory-pressure-workpool]]`) for organic concurrency
    //      scaling in response to memory pressure signals.
    //   3. `maybe_async_gc()` triggered by the cron monitor for prompt
    //      GC cycles.
    //
    // Robot.metta evaluates a single top-level `!` so this branch never
    // fired anyway; the change is hygiene to prevent regressions in
    // multi-expression workloads.
}

/// Trigger GC at a quiescent point (no active evaluators).
///
/// **Phase 9 disposition**: this function is retained as a public entry
/// point for tests and external callers that need strict quiescent-GC
/// semantics. The trampoline NO LONGER uses it — eval paths use
/// `maybe_async_gc` (no `ACTIVE_EVALUATORS == 0` gate, honors the
/// purely-async GC mandate). The legacy `safepoint_wait_for_quiescence`
/// caller has been deleted in Phase 9.3.
///
/// Conditions (unchanged):
/// 1. GC is not disabled (`--no-gc`)
/// 2. `GC_REQUESTED` is set (by cron monitor or manual request)
/// 3. No GC cycle is already in flight (`GC_CYCLE_IN_FLIGHT == false`)
/// 4. No evaluators are active (`ACTIVE_EVALUATORS == 0`)
///
/// Uses `GC_IN_PROGRESS` flag to prevent new evals from starting during
/// the brief snapshot capture (sub-millisecond). The actual mark-sweep
/// runs asynchronously on the GC thread.
///
/// Returns `true` if a GC cycle was triggered.
pub fn maybe_quiescent_gc() -> bool {
    // Signal that the GC lifecycle is reachable (for cron backpressure gating)
    bump_gc_reachable();

    // Fast path: skip if GC is disabled
    if is_gc_disabled() {
        GC_REQUESTED.store(false, Ordering::Relaxed);
        return false;
    }

    // Check if GC was requested
    if !GC_REQUESTED.load(Ordering::Acquire) {
        return false;
    }

    // Don't trigger if a GC cycle is already in flight (snapshot sent, response
    // not yet processed). Aligns with TLA+ `~hasGcRequest /\ ~hasGcResponse /\
    // gcPhase = "idle"` preconditions on TryQuiescentGc_AcquireFlag.
    if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
        return false;
    }

    // Check quiescent state
    if ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0 {
        return false;
    }

    // Consume GC request (CAS to avoid double-trigger)
    if GC_REQUESTED
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }

    // Set GC_IN_PROGRESS to prevent new evals from starting.
    // CAS-based try_enter() ensures mutual exclusion with session GC thread,
    // matching TLA+ `~gcInProgressFlag` precondition on TryQuiescentGc_AcquireFlag.
    // RAII guard ensures the flag is always cleared, even on panic.
    let _gc_guard = match GcInProgressGuard::try_enter() {
        Some(guard) => guard,
        None => {
            // Another GC path (session release) holds the flag — back off.
            // Re-arm GC_REQUESTED so we try again at the next quiescent point.
            GC_REQUESTED.store(true, Ordering::Release);
            return false;
        }
    };

    // Double-check no eval snuck in between our check and the flag set
    if ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0 {
        drop(_gc_guard);
        // Re-set GC_REQUESTED so we try again at the next quiescent point
        GC_REQUESTED.store(true, Ordering::Release);
        return false;
    }

    // F4 R3: the legacy slab pool path was removed; the index collector marks
    // directly, so this quiescent-trigger entry point is a no-op returning false.
    let result = false;

    drop(_gc_guard);
    result
}

/// Trigger GC asynchronously — **without** waiting for evaluator quiescence.
///
/// This is the Phase 9 entry point that honors the purely-async GC mandate.
/// Unlike `maybe_quiescent_gc`, it does NOT require `ACTIVE_EVALUATORS == 0`.
/// The mark-sweep cycle runs on the GC pool against a snapshot of the slab
/// state at the moment `trigger_gc_cycle_via_pool` is called; concurrent
/// mutators do not interfere because:
///
/// - The brief `GC_IN_PROGRESS` window during `build_snapshot` parks *new*
///   `EvalGuard::enter()` calls (`EvalGuard::enter` waits while
///   `GC_IN_PROGRESS` is set). This is the only synchronous coupling and is
///   bounded by snapshot cost (sub-millisecond per page; typical workloads
///   complete in tens of microseconds).
/// - Mutators already mid-eval keep their `EvalGuard` and continue running.
///   They never block waiting for the cycle to complete.
/// - The snapshot captures all reachable values via `collect_all_roots()`,
///   which walks every registered `RootProvider` plus
///   `register_temporary_roots` entries. Phase 6 + Phase 8 root providers
///   cover parallel-dispatch INPUTS and OUTPUTS; per-thread current-iter
///   roots (`current_iter_root::CurrentIterRootProvider`) cover the
///   in-flight value on each evaluator thread. Together these are
///   sufficient — no quiescence required.
/// - Reclaim (`process_gc_response`) is gated by `GcInProgressGuard::try_enter()`
///   for mutex with session-release, which still requires
///   `ACTIVE_EVALUATORS == 0`. That gate is unchanged and remains the only
///   path that legitimately waits for quiescence.
///
/// Returns `true` if a GC cycle was triggered.
pub fn maybe_async_gc() -> bool {
    // Signal that the GC lifecycle is reachable (for cron backpressure gating).
    bump_gc_reachable();

    // Inc 4: the slab collector is inert in index mode (see enqueue_session_release).
    if crate::backend::models::metta_value::gc_mode_is_index() {
        GC_REQUESTED.store(false, Ordering::Relaxed);
        return false;
    }

    if is_gc_disabled() {
        GC_REQUESTED.store(false, Ordering::Relaxed);
        return false;
    }

    if !GC_REQUESTED.load(Ordering::Acquire) {
        return false;
    }

    // At-most-one cycle in flight (matches the existing TLA+ precondition
    // `~hasGcRequest /\ ~hasGcResponse /\ gcPhase = "idle"`).
    if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
        return false;
    }

    // Consume the request.
    if GC_REQUESTED
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }

    // Acquire `GC_IN_PROGRESS` mutex (against session-release GC path).
    // **No `ACTIVE_EVALUATORS == 0` gate**: snapshot construction parks
    // *new* `EvalGuard::enter()` calls via the existing condvar logic, and
    // mutators already mid-eval are safe because the snapshot reads
    // `bump_count` / `epochs[i]` with `Acquire` and gets a consistent
    // prefix view. Phase 9 TLA+ update: weakens the
    // `TryQuiescentGc_AcquireFlag` precondition `activeEvaluators = 0`
    // → just `~gcInProgressFlag /\ ~hasGcRequest /\ ~hasGcResponse /\
    // gcPhase = "idle"`.
    let _gc_guard = match GcInProgressGuard::try_enter() {
        Some(guard) => guard,
        None => {
            // Session-release GC holds the flag — back off; cron will
            // retry on its next tick.
            GC_REQUESTED.store(true, Ordering::Release);
            return false;
        }
    };

    // F4 R3: the legacy slab pool path was removed; this is a no-op returning false.
    let result = false;
    drop(_gc_guard);
    result
}

/// Process any pending GC response from the GC thread.
///
/// Called from `SessionContext::maybe_gc()` every 256 trampoline iterations.
/// This does NOT trigger new GC cycles — it only:
/// 1. Lazily spawns the GC cron manager (idempotent via OnceLock)
/// 2. Processes pending GC responses (epoch-filtered sweep — safe at any time)
/// 3. Updates adaptive threshold
///
/// New GC cycles are triggered exclusively by `maybe_quiescent_gc()` at
/// quiescent points between top-level expressions.
///
/// **Mutual exclusion**: Acquires `GcInProgressGuard` before consuming the
/// response channel, ensuring mutual exclusion with
/// `release_session_with_surviving()` on GC pool worker threads. Both paths
/// free/poison value slots; concurrent execution is a TOCTOU race on slot
/// epoch/content (FlyingRaven ASAN finding: use-after-poison in
/// `collect_dead_data`).
///
/// Returns `true` if a GC response was processed.
pub fn maybe_process_gc_response() -> bool {
    // Fast exit: if no GC cycle is in-flight AND no GC requested, skip all
    // expensive atomics (bump_gc_reachable, OnceLock check, CAS guard).
    // Relaxed ordering: false negatives are harmless — caught next call.
    if !GC_CYCLE_IN_FLIGHT.load(Ordering::Relaxed) && !GC_REQUESTED.load(Ordering::Relaxed) {
        return false;
    }

    // Signal that the GC lifecycle is reachable (for cron backpressure gating)
    bump_gc_reachable();

    // Lazily spawn the GC cron manager (idempotent via OnceLock)
    let _ = global_gc_cron();

    // F4 R3: the legacy slab GC-pool response channel was removed; the index
    // collector does not receive pool responses, but cron init above preserves
    // counter-sync scheduling.
    {
        return false;
    }

}

// ============================================================================
// GC Root Registry — REMOVED (F4 rung R5)
//
// The legacy dynamic RootProvider registry (trait, ROOT_REGISTRY, registration,
// collect_all_roots) is deleted: the index collector reads roots structurally
// via collect_machine_roots ∪ the kept collect_safepoint_roots below.
// ============================================================================

// ============================================================================
// Safepoint Root Registry — Temporary Roots for Intra-Evaluation GC
// ============================================================================
//
// During intra-evaluation safepoints, the trampoline's work_stack and
// continuations hold live MettaValue references that are invisible to the
// GC root collector (they live on the Rust call stack, not in environments).
// Before dropping the EvalGuard, the trampoline registers these values as
// temporary roots so the GC can trace them and avoid freeing reachable objects.
//
// The registry is a global Mutex<Vec<Option<Vec<MettaValue>>>>. Each safepoint
// claims one slot and receives a SafepointRootHandle that releases it on drop.
// collect_all_roots() copies active slots into the root set alongside
// environment roots.

/// Global registry for safepoint temporary roots.
///
/// Each active entry is a Vec<MettaValue> collected from one evaluator's
/// trampoline state (work_stack + continuations). `None` means the slot is
/// free. This keeps an active empty root set distinct from a reusable slot.
static SAFEPOINT_ROOTS: OnceLock<Mutex<Vec<Option<Vec<MettaValue>>>>> = OnceLock::new();

fn safepoint_registry() -> &'static Mutex<Vec<Option<Vec<MettaValue>>>> {
    SAFEPOINT_ROOTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// RAII handle that unregisters safepoint roots when dropped.
///
/// Created by `register_temporary_roots()`. On drop, marks the stored slot as
/// free without removing it, so other handles' indices stay valid.
pub struct SafepointRootHandle {
    /// Index into SAFEPOINT_ROOTS where this handle's roots are stored
    idx: usize,
}

impl Drop for SafepointRootHandle {
    fn drop(&mut self) {
        let registry = safepoint_registry();
        let mut guard = registry.lock();
        if self.idx < guard.len() {
            guard[self.idx] = None;
        }
    }
}

/// Register temporary roots for a GC safepoint.
///
/// The provided `MettaValue` roots will be included in `collect_all_roots()`
/// until the returned `SafepointRootHandle` is dropped. This ensures the GC
/// traces trampoline-resident values as live during intra-evaluation collection.
///
/// # Critical Ordering
///
/// This MUST be called BEFORE dropping the EvalGuard. Otherwise there is a
/// window where `ACTIVE_EVALUATORS == 0` but roots aren't registered, and
/// the GC would free reachable values (use-after-free).
pub fn register_temporary_roots(roots: Vec<MettaValue>) -> SafepointRootHandle {
    let registry = safepoint_registry();
    let mut guard = registry.lock();
    // Reuse a free slot if available. An active empty root set is still Some.
    for (idx, slot) in guard.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(roots);
            return SafepointRootHandle { idx };
        }
    }
    // No free slot: append.
    let idx = guard.len();
    guard.push(Some(roots));
    SafepointRootHandle { idx }
}

/// Collect safepoint roots into the provided Vec.
///
/// Called from `collect_all_roots()` to include trampoline-resident values
/// in the GC root set alongside environment roots.
///
/// CESK A4.4: this is the NARROW driver-transport channel (the plan's "keep
/// SAFEPOINT_ROOTS narrow" — register_temporary_roots publication buffers like
/// the conformance runner's cross-directive result accumulator, plus the
/// thread-local cache snapshot via CACHE_ROOT_HANDLE). The structural reader does
/// NOT replace it; the A4.4 flip + the machine-equivalence oracles read it as a
/// KEPT channel (so dropping it from the live feed would free the driver's
/// accumulated results → UAF). A5.4 narrows it further. `pub(crate)` for those
/// callers. (`pub` to match `collect_all_roots`'s re-export convention; the
/// `models` module is crate-internal.)
pub fn collect_safepoint_roots(roots: &mut Vec<MettaValue>) {
    if let Some(registry) = SAFEPOINT_ROOTS.get() {
        let guard = registry.lock();
        for root_set in guard.iter().flatten() {
            roots.extend(root_set.iter().copied());
        }
    }
}

// ============================================================================
// E1-FLIP / CEX-1 (D2) — Live Parallel-Dispatch Fan-Out Anchor
// ============================================================================
//
// The dedicated-GC-thread rendezvous collector marks from
// `drain(WORKER_ROOT_BUFFER) ∪ collect_safepoint_roots() ∪
// collect_live_dispatch_anchors()`. The buffer carries each PARTICIPATING
// thread's self-published thread-local contribution (D1). But a worker closure's
// dispatch INPUTS (`branch_expr`/`branch_bindings`, moved into the closure) and a
// completed branch's OUTPUTS (`results[slot]`) are reachable from the parent's
// `Continuation::WaitForParallel` fan-out — a SHARED `Arc` the GC thread CAN read.
// A worker that is admission-blocked at `EvalGuard::enter` (parked on
// `GC_PROGRESS_CONDVAR`) or not-yet-started is NOT a rendezvous participant and
// never self-roots, yet holds those live `Addr`s. This anchor walks the fan-out
// structurally — park-timing-independently — closing that hole.
//
// SOUNDNESS (NOT a discovery side-channel): `LIVE_DISPATCHES` is the reification
// of the live parallel-K tree's fork nodes (`WaitForParallel`). Sequential K is
// one native stack (walked by `collect_k_spine`); a forked K is a tree, whose
// pending `(expr, bindings)` are un-entered sub-continuation INPUTS and
// `results[slot]` the completed OUTPUTS — both `σ|_Reachable` of the parallel K.
// The set of live dispatches IS machine state (in-flight parallel continuations),
// read structurally by name, with a bounded shape; nothing opts in except the
// dispatch op, and what it yields is determined entirely by K-structure. It is the
// parallel analogue of `collect_k_spine`'s `SUSPENDED_ACTIVATIONS`. (The slab build
// did exactly this via `ParallelDispatchRoots` in `ROOT_REGISTRY`; A5 deleted
// the registry and never replaced the walk — that omission is the residual bug D2
// fixes. We do NOT reuse `ROOT_REGISTRY`: this is a typed, dispatch-only anchor.)
//
// Modeled byte-for-byte on `SAFEPOINT_ROOTS` above: a global `Mutex<Vec<Option<…>>>`,
// RAII slot-free on `Drop`, free-slot reuse on register. The stored handle is a
// `Weak` so a panicked worker that skips deregistration self-heals (the upgrade
// fails and the slot is pruned), exactly like the slab `ROOT_REGISTRY`'s
// `Weak<dyn RootProvider>`.

/// E1-FLIP / CEX-1 (D2): the GC-thread-readable view of one in-flight parallel
/// dispatch's fan-out (`branches`/`items` INPUTS + `results` OUTPUTS). Implemented
/// by `ParallelDispatchRoots` / `ParallelCollapseRoots` (types.rs),
/// whose bodies are the SAME `collect_roots` the slab `RootProvider` impls run
/// (inputs from the immutable `Arc<Vec<…>>`; outputs via `results.try_lock()`,
/// NEVER `lock` — a contended `results` ⇒ a worker mid-write holding its EvalGuard
/// is a participant who self-rooted that value, so skipping is safe).
pub trait DispatchRoots: Send + Sync {
    fn collect_dispatch_roots(&self, out: &mut Vec<MettaValue>);
}

/// E1-FLIP / CEX-1 (D2): the registry of live parallel-dispatch fan-outs. `None`
/// marks a free slot (kept, not removed, so other handles' indices stay valid —
/// the `SAFEPOINT_ROOTS` discipline). Each entry is a `Weak` so a dispatch handle
/// that drops without deregistering (panic) self-prunes on the next walk.
// NOTE: `std::sync::Weak` is fully-qualified here because the top-level
// `use std::sync::Weak` import was slab-`ROOT_REGISTRY`-only and was removed with it.
static LIVE_DISPATCHES: OnceLock<Mutex<Vec<Option<std::sync::Weak<dyn DispatchRoots>>>>> =
    OnceLock::new();

fn live_dispatches() -> &'static Mutex<Vec<Option<std::sync::Weak<dyn DispatchRoots>>>> {
    LIVE_DISPATCHES.get_or_init(|| Mutex::new(Vec::new()))
}

/// E1-FLIP / CEX-1 (D2): RAII handle that frees its `LIVE_DISPATCHES` slot on drop.
/// Stored in the dispatch handle's `_live_dispatch` field, so the slot is released
/// when the `WaitForParallel`(`Collapse`) continuation is consumed (the dispatch
/// finishes / cancels). Mirrors `SafepointRootHandle`.
pub struct LiveDispatchHandle {
    idx: usize,
}

impl Drop for LiveDispatchHandle {
    fn drop(&mut self) {
        let registry = live_dispatches();
        let mut guard = registry.lock();
        if self.idx < guard.len() {
            guard[self.idx] = None;
        }
    }
}

/// E1-FLIP / CEX-1 (D2): register a live dispatch fan-out so the GC thread can walk
/// its INPUTS/OUTPUTS for the dispatch's lifetime. Stores `Arc::downgrade(d)` (a
/// `Weak` — never extends the lifetime); reuses a free slot or appends. The
/// returned handle frees the slot on drop. Call at dispatch construction
/// (`parallel_dispatch` / `parallel_collapse_dispatch`).
pub fn register_live_dispatch(d: &Arc<dyn DispatchRoots>) -> LiveDispatchHandle {
    let registry = live_dispatches();
    let mut guard = registry.lock();
    let weak = Arc::downgrade(d);
    for (idx, slot) in guard.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(weak);
            return LiveDispatchHandle { idx };
        }
    }
    let idx = guard.len();
    guard.push(Some(weak));
    LiveDispatchHandle { idx }
}

/// E1-FLIP / CEX-1 (D2): walk every live dispatch fan-out into `out`, pruning slots
/// whose `Weak` no longer upgrades (a handle dropped without deregistering — panic).
/// Called by the dedicated GC thread (`gc_driver_rendezvous_cycle`) after draining
/// `WORKER_ROOT_BUFFER` and `collect_safepoint_roots`. Lock order: `LIVE_DISPATCHES`
/// then per-handle `results.try_lock()` (inside `collect_dispatch_roots`, never a
/// blocking `lock`); the GC thread holds no other lock here.
pub fn collect_live_dispatch_anchors(out: &mut Vec<MettaValue>) {
    if let Some(registry) = LIVE_DISPATCHES.get() {
        let mut guard = registry.lock();
        for slot in guard.iter_mut() {
            let prune = match slot {
                Some(weak) => match weak.upgrade() {
                    Some(strong) => {
                        strong.collect_dispatch_roots(out);
                        false
                    }
                    None => true, // dead Weak — prune (handle dropped w/o deregister)
                },
                None => false,
            };
            if prune {
                *slot = None;
            }
        }
    }
}

/// E1-FLIP / CEX-1 (D5 oracle support): snapshot every live dispatch's INPUT∪OUTPUT
/// `Addr`s (as `inner_ptr` usizes) WITHOUT pruning — the witness multiset the
/// rendezvous-union oracle checks `collect_live_dispatch_anchors` covered. Returns
/// the live-handle count too. Debug-only callers.
#[cfg(debug_assertions)]
pub fn snapshot_live_dispatch_witness() -> (Vec<MettaValue>, usize) {
    let mut out = Vec::new();
    let mut live = 0usize;
    if let Some(registry) = LIVE_DISPATCHES.get() {
        let guard = registry.lock();
        for slot in guard.iter() {
            if let Some(weak) = slot {
                if let Some(strong) = weak.upgrade() {
                    live += 1;
                    strong.collect_dispatch_roots(&mut out);
                }
            }
        }
    }
    (out, live)
}


// ============================================================================
// EvalGuard Drop/Reacquire — Safepoint Lifecycle
// ============================================================================
//
// During a safepoint, the trampoline needs to temporarily release its EvalGuard
// (decrement ACTIVE_EVALUATORS) to allow the quiescent GC to fire, then re-acquire
// it to continue evaluation. These functions implement that protocol without
// changing the EvalGuard RAII interface — the original EvalGuard in eval() still
// handles final cleanup.

thread_local! {
    /// Tracks the depth of EvalGuard acquisitions on this thread.
    /// Used by safepoint drop/reacquire to temporarily release without
    /// conflicting with the outer EvalGuard's RAII drop.
    static EVAL_GUARD_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Temporarily release the current thread's EvalGuard for a GC safepoint.
///
/// Decrements `ACTIVE_EVALUATORS` and notifies the quiescent condvar if
/// transitioning to 0. The caller MUST have registered temporary roots
/// before calling this function.
///
/// # Panics
///
/// Panics if called without an active EvalGuard (depth == 0).
pub fn drop_eval_guard_for_safepoint() {
    EVAL_GUARD_DEPTH.with(|d| {
        let depth = d.get();
        assert!(
            depth > 0,
            "drop_eval_guard_for_safepoint called without active guard"
        );
        d.set(depth - 1);
        // §1.2: a parking mutator leaves the active-thread set when its LAST
        // guard level drops. (E1-c upgrades this to a full-depth drain; the
        // depth==1 hook then fires when the drained depth reaches 0.)
        if depth == 1 {
            N_THREADS.fetch_sub(1, Ordering::AcqRel);
        }
    });

    let prev = ACTIVE_EVALUATORS.fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        // Transitioned to quiescent state — notify waiters
        let _lock = QUIESCENT_MUTEX.lock();
        QUIESCENT_CONDVAR.notify_all();
    }
}

/// Current thread's EvalGuard nesting depth (the thread-local `EVAL_GUARD_DEPTH`).
/// Used by the safepoint sites to guard `depth>0` before
/// [`drop_eval_guard_for_safepoint_full`] (a depth==0 caller — e.g. the
/// post-EvalGuard type-fixpoint / MORK path — is not in the active set and must
/// not park; design Part 8).
#[allow(dead_code)]
pub fn eval_guard_depth() -> u32 {
    EVAL_GUARD_DEPTH.with(|d| d.get())
}

/// E1-c (design §Part 9): like [`drop_eval_guard_for_safepoint`] but drains the
/// FULL thread-local guard NESTING in one shot — decrement `ACTIVE_EVALUATORS` by
/// the whole depth, set depth to 0, leave the active-thread set (`N_THREADS--`),
/// and return the drained depth for [`reacquire_eval_guard_after_safepoint_full`]
/// to restore. The one-level [`drop_eval_guard_for_safepoint`] mis-counts a
/// depth>1 worker (it would leave `active`/`N_THREADS` off by `depth-1`), so a
/// parking worker that may be nested MUST use this. Callers MUST guard `depth>0`
/// (depth==0 ⇒ not in the active set ⇒ must not park, Part 8).
///
/// Source-coupled by depth-positive cooperative safepoint parks.
#[allow(dead_code)]
pub fn drop_eval_guard_for_safepoint_full() -> u32 {
    let depth = EVAL_GUARD_DEPTH.with(|d| {
        let v = d.get();
        d.set(0);
        v
    });
    assert!(
        depth > 0,
        "drop_eval_guard_for_safepoint_full called without active guard"
    );
    let prev = ACTIVE_EVALUATORS.fetch_sub(depth, Ordering::AcqRel);
    if prev == depth {
        // Transitioned to quiescent (0 active) — notify waiters (generalized
        // `prev == 1` from the one-level variant).
        let _lock = QUIESCENT_MUTEX.lock();
        QUIESCENT_CONDVAR.notify_all();
    }
    N_THREADS.fetch_sub(1, Ordering::AcqRel);
    depth
}

/// Re-acquire the EvalGuard after a GC safepoint completes.
///
/// Increments `ACTIVE_EVALUATORS`, blocking if `GC_IN_PROGRESS` is set
/// (same protocol as `EvalGuard::enter()`).
pub fn reacquire_eval_guard_after_safepoint() {
    /// Maximum time to wait for GC_IN_PROGRESS to clear before retrying.
    const GC_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    // Same entry protocol as EvalGuard::enter() — CAS loop with GC_IN_PROGRESS check
    loop {
        ACTIVE_EVALUATORS.fetch_add(1, Ordering::AcqRel);
        if !GC_IN_PROGRESS.load(Ordering::Acquire) {
            break;
        }
        // GC snapshot in progress — back off and park
        ACTIVE_EVALUATORS.fetch_sub(1, Ordering::AcqRel);
        let mut lock = GC_PROGRESS_MUTEX.lock();
        while GC_IN_PROGRESS.load(Ordering::Acquire) {
            let result = GC_PROGRESS_CONDVAR.wait_for(&mut lock, GC_WAIT_TIMEOUT);
            if result.timed_out() && GC_IN_PROGRESS.load(Ordering::Acquire) {
                tracing::warn!(
                    "reacquire_eval_guard_after_safepoint(): GC_IN_PROGRESS still set after {:?} wait — retrying",
                    GC_WAIT_TIMEOUT,
                );
                break; // Break inner loop to retry outer loop
            }
        }
        drop(lock);
    }

    EVAL_GUARD_DEPTH.with(|d| {
        let prev = d.get();
        d.set(prev + 1);
        // §1.2: rejoin the active-thread set when resuming from a park. Placed
        // AFTER the admission loop (same discipline as EvalGuard::enter).
        if prev == 0 {
            // ── E1-FLIP Path B V4 — slot RESTAMP (non-full reacquire) ──
            // V4 §"V4 CONVERGED": restamp acquired=current_cycle_gen WITHOUT release
            // (the slot stays occupied from enter to the outermost drop). This
            // non-full reacquire is reached only by the test-only safepoint pair in
            // production (the FANOUT>0 reified parks use the `_full` variant), so the
            // restamp is inert there, but it is wired for lifecycle consistency with
            // the `_full` rejoin. SLAB-BYTE-IDENTICAL: cfg + index-mode gate.
            {
                if dedicated_gc_enabled() {
                    witness_restamp_acquired(current_cycle_gen());
                }
            }
            N_THREADS.fetch_add(1, Ordering::AcqRel);
        }
    });
}

/// E1-c (design §Part 9 + Round-4 F2) + **E1-FLIP Path B V4 straddle re-park**:
/// re-acquire after a full-depth safepoint drain. The base protocol (1) waits until
/// the cycle the worker parked for has ENDED (`gen != my_gen`); (2) passes the
/// `GC_IN_PROGRESS` admission gate ONCE; (3) restores the full depth in a SINGLE
/// `fetch_add(saved_depth)` + rejoins the active-thread set + restores the
/// thread-local depth. The single-shot add eliminates the partial-increment /
/// mis-`n_threads` race. Admission stays on `GC_IN_PROGRESS` (driver-exclusive) —
/// NOT `GC_REQUESTED` (the §9.1 switch was rejected by F2).
///
/// **V4 STRADDLE re-park (the crux — docs/cesk-gc/e1-flip-pathB-v2-impl.md §"V4 —
/// the slot lifecycle" + §"V4 straddle trace"):** a thread T parked for cycle K,
/// snapshotted into K's `WORKER_ROOT_BUFFER` (CLEARED at `end_rendezvous_cycle`),
/// wakes to find cycle K+1 already collecting. T's machine (its `work_stack` on its
/// paused Rust stack, captured in `reparked_roots`) is NOT in K+1's buffer and NOT
/// globally walked ⇒ K+1 would sweep it → UAF on resume. FIX = re-park on EVERY new
/// intervening cycle until none is in flight, with T's witness slot held OCCUPIED
/// THROUGHOUT (V4: the slot was acquired at `EvalGuard::enter` and is NOT released
/// at the safepoint drop — so the driver WAITS for T at every cycle until T
/// re-stamps for it). Each intervening cycle does `witness_restamp_acquired(g)` →
/// `worker_park_and_root_in_cycle(reparked_roots, g)` (re-publish T's machine into
/// g's buffer + `note_reified_park(g)` + wait g-end). NO seam — closes attacks
/// #1/#3/#4 by construction. Correct ONLY because the collector is NON-MOVING
/// (re-publishing the same Addrs across cycles is sound; a surviving Addr stays at
/// its slot). NO release here (the rejoin restamps + N_THREADS++; release is ONLY at
/// the outermost EvalGuard::drop).
///
/// `reparked_roots` is the thread's reified machine roots for THIS park (the
/// in-scope `park_roots`/`my_roots` at the call site; the borrow spans the loop —
/// T runs nothing during the straddle so the snapshot stays complete).
///
/// Source-coupled by cooperative safepoint rejoin and straddle re-park.
#[allow(dead_code)]
pub fn reacquire_eval_guard_after_safepoint_full(
    reparked_roots: &[MettaValue],
    saved_depth: u32,
    my_gen: u64,
) {
    /// Maximum time to wait for GC_IN_PROGRESS to clear before retrying.
    const GC_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    // ── E1-FLIP Path B V4: the STRADDLE re-park loop ──
    // Engaged ONLY when the dedicated GC thread is driving (cfg + dedicated). When
    // dormant, falls through to the unchanged base protocol below (BYTE-IDENTICAL).
    {
        if dedicated_gc_enabled() {
            let _site = GcWaitSiteGuard::enter(&GC_STRADDLE_WAITERS);
            // The slot is ALREADY occupied (acquired at EvalGuard::enter, never
            // released at the safepoint drop) — so the pre-loop acquire is a
            // RESTAMP, not an acquire (V4 §"the slot lifecycle"). `my_reparked_gen`
            // = the gen this thread originally parked for.
            //
            // ── E5 (the straddle-deadlock fix) ──────────────────────────────────
            // docs/cesk-gc/e1-flip-deadlock-straddle-rootcause-2026-06-03.md.
            // The re-park gate now keys on `current_cycle_started()` (the gen of a
            // cycle a LIVE driver has ACTUALLY committed to), NOT `current_cycle_gen()`.
            // The original `gc_in_progress() && gen != my` test could not distinguish
            // "tail of K, gen pre-bumped to K+1, gip still true" (the teardown window
            // between `end_rendezvous_cycle`'s gen-bump and the `_gip` drop) from
            // "K+1 genuinely collecting" → a worker re-parked for a PHANTOM cycle K+1
            // that no driver runs → permanent-false predicate → hang. `started` stays
            // = K through that window (it is bumped to K+1 only at K+1's prologue, by a
            // live driver), so the `started > my` gate does NOT phantom-re-park.
            //
            // THE B-CLOSURE: the gate alone closes the step1→step2 window but leaves a
            // terminal-break mis-skip — A breaks with stale `started=K ∧ !gip`, then the
            // driver starts K+1 (begins waiting on A's occupied,published=K slot), and A
            // sits in a NON-publishing admission wait → driver waits forever. So the
            // re-park gate is RE-EVALUATED at the rejoin tail too (one labeled loop):
            // before the rejoin admission wait, re-read `started`; if it advanced past
            // `my_reparked_gen`, go BACK into the straddle re-park (which republishes via
            // `note_reified_park`), never a terminal occupied-unpublished break (R1).
            let mut my_reparked_gen = my_gen;
            'straddle: loop {
                // ── Straddle re-park phase: `started`-gated (NOT gen-gated) ──
                loop {
                    let started = current_cycle_started();
                    if started > my_reparked_gen {
                        // A REAL, started, later cycle needs this thread. Restamp keeps
                        // the slot OCCUPIED (never released across the straddle); the
                        // re-park re-publishes T's machine into `started`'s buffer +
                        // stamps published=started (gen-gated inside
                        // worker_park_and_root_in_cycle: it publishes only if
                        // GC_CYCLE_GEN==started, the live cycle) + waits `started`-end.
                        witness_restamp_acquired(started);
                        worker_park_and_root_in_cycle(reparked_roots, started);
                        my_reparked_gen = started;
                        continue;
                    }
                    if gc_in_progress() {
                        if current_cycle_gen() == my_reparked_gen {
                            // MY cycle is genuinely DRAINING (gen == my, gip set, no later
                            // started cycle). Wait for it to end (gen advance), then
                            // re-check. Lost-wakeup-safe: gen-gated on RENDEZVOUS_MUTEX
                            // (E5 — the bump in `end_rendezvous_cycle` + this wait share
                            // that mutex/condvar; no cross-mutex window).
                            worker_resume_wait_for_cycle(my_reparked_gen);
                        } else {
                            // TEARDOWN WINDOW: gip still set but gen != my AND
                            // started <= my ⇒ this is the tail of an OLDER cycle whose
                            // gen was pre-bumped but whose `_gip` is not yet cleared (and
                            // no later cycle has STARTED). A `worker_resume_wait_for_cycle`
                            // would return instantly (gen != my) and spin; instead wait on
                            // the gip transition so we do NOT busy-spin. Re-checks both
                            // `gip` AND `started` under GC_PROGRESS_MUTEX so a newly-STARTED
                            // cycle (set_current_cycle_started notifies this condvar) breaks
                            // us out to re-park, and the `_gip` clear (GcInProgressGuard::drop
                            // notifies this condvar under this mutex) wakes us to break.
                            let mut l = GC_PROGRESS_MUTEX.lock();
                            while GC_IN_PROGRESS.load(Ordering::Acquire)
                                && current_cycle_started() <= my_reparked_gen
                            {
                                let result = GC_PROGRESS_CONDVAR.wait_for(&mut l, GC_WAIT_TIMEOUT);
                                if result.timed_out()
                                    && GC_IN_PROGRESS.load(Ordering::Acquire)
                                    && current_cycle_started() <= my_reparked_gen
                                {
                                    tracing::warn!(
                                        "reacquire_eval_guard_after_safepoint_full() [E5 straddle \
                                         teardown-window]: GC_IN_PROGRESS still set (gen {} != my \
                                         {}, started <= my) after {:?} — re-checking",
                                        current_cycle_gen(),
                                        my_reparked_gen,
                                        GC_WAIT_TIMEOUT,
                                    );
                                }
                            }
                        }
                        continue;
                    }
                    // started <= my && !gip ⇒ no started cycle waits on this thread →
                    // proceed to the rejoin tail.
                    break;
                }

                // ── Rejoin tail (V4 + the E5 B-CLOSURE) ──
                // Restamp the slot for the current gen (resumes WITNESSED — the next
                // cycle waits for T until it parks again, steady-state). NO
                // witness_release_slot (the slot stays occupied; release is ONLY at the
                // outermost EvalGuard::drop).
                witness_restamp_acquired(current_cycle_gen());
                loop {
                    // B-CLOSURE (R1): re-read `started` INSIDE the admission loop. If a
                    // driver has STARTED a cycle later than my last reparked gen while we
                    // were about to rejoin, we must NOT sit in this non-publishing wait
                    // (that is the relocated hang — the driver is now waiting on our
                    // occupied,published<started slot). Go BACK into the straddle re-park,
                    // which REPUBLISHES (note_reified_park) for `started`.
                    if current_cycle_started() > my_reparked_gen {
                        continue 'straddle;
                    }
                    if !GC_IN_PROGRESS.load(Ordering::Acquire) {
                        break;
                    }
                    let mut lock = GC_PROGRESS_MUTEX.lock();
                    while GC_IN_PROGRESS.load(Ordering::Acquire)
                        && current_cycle_started() <= my_reparked_gen
                    {
                        let result = GC_PROGRESS_CONDVAR.wait_for(&mut lock, GC_WAIT_TIMEOUT);
                        if result.timed_out()
                            && GC_IN_PROGRESS.load(Ordering::Acquire)
                            && current_cycle_started() <= my_reparked_gen
                        {
                            tracing::warn!(
                                "reacquire_eval_guard_after_safepoint_full() [E5 straddle rejoin]: \
                                 GC_IN_PROGRESS still set after {:?} — re-checking",
                                GC_WAIT_TIMEOUT,
                            );
                            break;
                        }
                    }
                    drop(lock);
                    // Loop re-checks both the B-closure `started` gate and `gip`.
                }
                // Admission passed with no later started cycle and `!gip` (or `!gip` with
                // the B-closure re-check having held) — rejoin for good.
                break 'straddle;
            }
            ACTIVE_EVALUATORS.fetch_add(saved_depth, Ordering::AcqRel);
            N_THREADS.fetch_add(1, Ordering::AcqRel);
            EVAL_GUARD_DEPTH.with(|d| d.set(saved_depth));
            return;
        }
    }

    // ── Base protocol (slab / non-dedicated async path) — UNCHANGED, byte-identical ──
    // F2: wait until MY cycle ended (the gen advanced), then re-admit.
    worker_resume_wait_for_cycle(my_gen);
    // Admission: GC_IN_PROGRESS gate, passed ONCE (no per-level race).
    loop {
        if !GC_IN_PROGRESS.load(Ordering::Acquire) {
            break;
        }
        let mut lock = GC_PROGRESS_MUTEX.lock();
        while GC_IN_PROGRESS.load(Ordering::Acquire) {
            let result = GC_PROGRESS_CONDVAR.wait_for(&mut lock, GC_WAIT_TIMEOUT);
            if result.timed_out() && GC_IN_PROGRESS.load(Ordering::Acquire) {
                tracing::warn!(
                    "reacquire_eval_guard_after_safepoint_full(): GC_IN_PROGRESS still set after {:?} — retrying",
                    GC_WAIT_TIMEOUT,
                );
                break;
            }
        }
        drop(lock);
    }
    ACTIVE_EVALUATORS.fetch_add(saved_depth, Ordering::AcqRel);
    N_THREADS.fetch_add(1, Ordering::AcqRel);
    EVAL_GUARD_DEPTH.with(|d| d.set(saved_depth));
    // `reparked_roots` is only used by the V4 straddle path above; in the dormant/
    // slab path it is intentionally unused (the base protocol does not re-publish).
    let _ = reparked_roots;
}

/// Get the committed bytes from the global allocator's atomic counter.
///
/// Used by `should_safepoint()` to check allocation growth since the last
/// safepoint without acquiring locks.
#[inline]
pub fn committed_bytes_snapshot() -> usize {
    global_allocator()
        .committed_bytes_atomic()
        .load(Ordering::Relaxed)
}

/// Read the global allocation count (number of MettaValueInner allocations).
///
/// This counter increments for EVERY value allocation — both bump-alloc (new
/// pages) and free-list-reuse. Use this for safepoint triggering instead of
/// `committed_bytes_snapshot()` which plateaus after initial page allocation.
#[inline]
pub fn alloc_count_snapshot() -> u64 {
    global_allocator()
        .alloc_count_atomic()
        .load(Ordering::Relaxed)
}

/// Wait for quiescence during a safepoint (condvar-based, NOT spin/yield).
///
/// If `ACTIVE_EVALUATORS > 0` after this evaluator has dropped its guard,
/// parks on `QUIESCENT_CONDVAR` with a 10ms timeout for other evaluators
/// to also reach safepoints. Once quiescent (or timeout), attempts to
/// trigger and process GC.
///
/// Uses condvar parking instead of `yield_now()` to avoid CPU spin and
/// ensure instant wakeup when the last evaluator drops its guard.
///
/// ## GC Cycle Pipeline
///
/// Processing the previous response BEFORE triggering a new cycle ensures
/// every safepoint can complete a full GC round-trip:
/// 1. Process pending response (frees dead slots, clears GC_CYCLE_IN_FLIGHT)
/// 2. Trigger new GC cycle (marks roots, sweeps)
/// 3. Wait briefly for GC thread to complete
/// 4. Process the new response (frees newly dead slots)
///
/// Without this ordering, the previous approach alternated between trigger
/// and process, effectively halving GC throughput.
/// Fast version of `maybe_process_gc_response`: skip channel poll when no GC
/// cycle is in-flight. This avoids 4 atomic operations per call (GcInProgressGuard
/// CAS, channel poll, bump_gc_reachable, OnceLock check) when GC is idle.
///
/// Ultra-fast path: when neither GC_REQUESTED nor GC_CYCLE_IN_FLIGHT is set,
/// skip all atomics (bump_gc_reachable, global_gc_cron). This reduces the
/// per-call cost from ~3 atomics to 2 Relaxed loads (~1ns vs ~10ns).
#[inline]
pub fn maybe_process_gc_response_fast() -> bool {
    // Ultra-fast path: if no GC is requested AND no cycle is in-flight,
    // there's nothing to do — skip reachability bumping and cron check entirely.
    // Uses Relaxed ordering: false negatives (stale read) are harmless — we'll
    // catch the state on the next call (within 4096 trampoline iterations).
    if !GC_REQUESTED.load(Ordering::Relaxed) && !GC_CYCLE_IN_FLIGHT.load(Ordering::Relaxed) {
        return false;
    }
    // GC is active (requested or in-flight): bump reachability and ensure cron is spawned
    bump_gc_reachable();
    let _ = global_gc_cron();
    if !GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
        return false;
    }
    // Slow path: GC is in-flight, check for response
    maybe_process_gc_response()
}

// `safepoint_wait_for_quiescence` (DELETED in Phase 9).
//
// Was: a synchronous convergence-extension loop (up to 4×250 ms = 1 s)
// that parked the trampoline thread until `ACTIVE_EVALUATORS == 0` so a
// quiescent GC could fire. That mechanism violated the purely-async GC
// mandate — under deep parallel-dispatch nesting (Robot.metta PLN, 64+
// worker tasks), the wait would routinely exhaust its 1 s budget,
// producing ~1.3 s `gc-pause` events visible in `trace-analyzer`.
//
// Replaced by:
//   - `ParallelDispatchRoots` / `ParallelCollapseRoots`
//     (Phase 6 commit `429e798` + Phase 8 input coverage in `b4e0ed7`)
//   - `current_iter_root::CurrentIterRootProvider` (Phase 9.1)
//   - `register_temporary_roots` for parent frame snapshots
//   - `refresh_thread_local_cache_roots` for worker thread-locals
//   - `maybe_async_gc()` (Phase 9.2), cron-triggered, no ACTIVE_EVALUATORS gate.
//
// Together these keep all roots visible to async mark-sweep without
// requiring the trampoline to ever wait on GC progress.

/// Register an environment's shared state as a GC root provider.
///
/// Uses `Any` downcasting to conditionally register only when `V = MettaValue`.
/// For other value types, this is a no-op.
///
/// This is called from `GenericEnvironment::new()`, `make_owned()`,
/// `fork_for_nondeterminism()`, `union()`, and `union_all()`.
/// E₀ (the persistent global environment) is read STRUCTURALLY by
/// `collect_persistent_roots`, never through a registry, so environment
/// registration is a no-op. The signature survives because the 5 callers
/// (`GenericEnvironment::new`/`make_owned`/`fork_for_nondeterminism`/`union`/
/// `union_all`) invoke it unconditionally.
#[inline]
pub fn try_register_env_roots<V>(
    _shared: &Arc<crate::backend::environment::GenericEnvironmentShared<V>>,
) where
    V: crate::backend::models::metta_value_trait::MettaValueTrait
        + Clone
        + Send
        + Sync
        + Unpin
        + 'static,
{
    // E₀ is read structurally; no registry registration in the index regime.
}

// (F4 R3) trigger_gc_cycle removed: it submitted Collect work to the legacy
// slab GC pool, which no longer exists. The index collector marks/sweeps
// directly under the index-heap write lock.

// (F4 R3) trigger_gc_cycle_via_pool removed: same legacy slab GC pool path.


// ============================================================================
// GcFactory — MettaValueFactory backed by SlabAllocator
// ============================================================================

/// Factory for allocating values in the slab allocator.
///
/// Implements `MettaValueFactory<MettaValue>` so it can be used as the
/// factory type in `EvalContext` and `GenericEnvironment`.
#[derive(Clone, Copy)]
pub struct GcFactory {
    alloc: &'static SlabAllocator,
}

unsafe impl Send for GcFactory {}
unsafe impl Sync for GcFactory {}

impl Default for GcFactory {
    fn default() -> Self {
        // Construct the concrete `GcFactory` directly. `global_factory()` returns
        // `ActiveFactory` (= `IndexFactory`), which cannot satisfy this concrete
        // `GcFactory` return type, so this calls `GcFactory::new(global_allocator())`
        // directly.
        GcFactory::new(global_allocator())
    }
}

impl GcFactory {
    /// Create a new GcFactory for the given slab allocator.
    #[inline]
    pub fn new(alloc: &'static SlabAllocator) -> Self {
        Self { alloc }
    }

    /// Get the underlying slab allocator.
    #[inline]
    pub fn allocator(&self) -> &'static SlabAllocator {
        self.alloc
    }
}

impl std::fmt::Debug for GcFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcFactory")
            .field("alloc", &(self.alloc as *const SlabAllocator))
            .finish()
    }
}

impl super::metta_value_trait::MettaValueFactory<MettaValue> for GcFactory {
    #[inline]
    fn atom(&self, s: &str) -> MettaValue {
        let has_vars = super::metta_value::is_variable_str(s);
        let s = self.alloc.alloc_str(s);
        let inner = self.alloc.alloc_value(MettaValueInner::Atom(s));
        let flags = if has_vars {
            super::metta_value::FLAG_HAS_VARIABLES as u8
        } else {
            0
        };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn bool(&self, b: bool) -> MettaValue {
        MettaValue::inline_bool(b)
    }

    #[inline]
    fn long(&self, n: i64) -> MettaValue {
        // Inline for values that fit in 48-bit signed range
        if let Some(v) = MettaValue::try_inline_long(n) {
            return v;
        }
        // Fallback: slab-allocate for values outside i48 range
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Long(n)))
    }

    #[inline]
    fn float(&self, f: f64) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Float(f)))
    }

    #[inline]
    fn string(&self, s: &str) -> MettaValue {
        let s = self.alloc.alloc_str(s);
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::String(s)))
    }

    #[inline]
    fn sexpr(&self, items: Vec<MettaValue>) -> MettaValue {
        if items.is_empty() {
            return self.unit();
        }
        let has_vars = items.iter().any(|i| i.has_variables_fast());
        if !has_vars {
            // Hash-consing: deduplicate ground S-expressions
            let key = hash_cons_key(&items);
            if let Some(existing) = hash_cons_lookup(key, &items) {
                return existing;
            }
            let slice = self.alloc.alloc_slice_copy(&items);
            let inner = self.alloc.alloc_value(MettaValueInner::SExpr(slice));
            let result = MettaValue::from_inner(inner); // flags = 0 (no variables)
            hash_cons_insert(key, result);
            result
        } else {
            let slice = self.alloc.alloc_slice_copy(&items);
            let inner = self.alloc.alloc_value(MettaValueInner::SExpr(slice));
            MettaValue::from_inner_tagged(inner, super::metta_value::FLAG_HAS_VARIABLES as u8)
        }
    }

    #[inline]
    fn sexpr_from_slice(&self, items: &[MettaValue]) -> MettaValue {
        if items.is_empty() {
            return self.unit();
        }
        let has_vars = items.iter().any(|i| i.has_variables_fast());
        if !has_vars {
            // Hash-consing: deduplicate ground S-expressions
            let key = hash_cons_key(items);
            if let Some(existing) = hash_cons_lookup(key, items) {
                return existing;
            }
            let slice = self.alloc.alloc_slice_copy(items);
            let inner = self.alloc.alloc_value(MettaValueInner::SExpr(slice));
            let result = MettaValue::from_inner(inner); // flags = 0 (no variables)
            hash_cons_insert(key, result);
            result
        } else {
            let slice = self.alloc.alloc_slice_copy(items);
            let inner = self.alloc.alloc_value(MettaValueInner::SExpr(slice));
            MettaValue::from_inner_tagged(inner, super::metta_value::FLAG_HAS_VARIABLES as u8)
        }
    }

    #[inline]
    fn error(&self, offending: MettaValue, detail: MettaValue) -> MettaValue {
        let inner = self
            .alloc
            .alloc_value(MettaValueInner::Error(offending, detail));
        let flags = if offending.has_variables_fast() || detail.has_variables_fast() {
            super::metta_value::FLAG_HAS_VARIABLES as u8
        } else {
            0
        };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn type_value(&self, value: MettaValue) -> MettaValue {
        let inner = self.alloc.alloc_value(MettaValueInner::Type(value));
        let flags = if value.has_variables_fast() {
            super::metta_value::FLAG_HAS_VARIABLES as u8
        } else {
            0
        };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn conjunction(&self, goals: Vec<MettaValue>) -> MettaValue {
        self.conjunction_from_slice(&goals)
    }

    #[inline]
    fn conjunction_from_slice(&self, goals: &[MettaValue]) -> MettaValue {
        let has_vars = goals.iter().any(|g| g.has_variables_fast());
        let slice = self.alloc.alloc_slice_copy(goals);
        let inner = self.alloc.alloc_value(MettaValueInner::Conjunction(slice));
        let flags = if has_vars {
            super::metta_value::FLAG_HAS_VARIABLES as u8
        } else {
            0
        };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn space(&self, handle: super::SpaceHandle) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Space(handle)))
    }

    #[inline]
    fn state(&self, id: u64) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::State(id)))
    }

    #[inline]
    fn unit(&self) -> MettaValue {
        MettaValue::inline_unit()
    }

    #[inline]
    fn memo(&self, handle: super::MemoHandle) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Memo(handle)))
    }

    #[inline]
    fn quote(&self, value: MettaValue) -> MettaValue {
        let inner = self.alloc.alloc_value(MettaValueInner::Quoted(value));
        let flags = if value.has_variables_fast() {
            super::metta_value::FLAG_HAS_VARIABLES as u8
        } else {
            0
        };
        MettaValue::from_inner_tagged(inner, flags)
    }

    /// PT-canonical Lazy wrapper (2026-05-21).
    ///
    /// Idempotency optimization: if `value` is already Lazy, return it as-is
    /// (avoiding nested Lazy(Lazy(x)) allocations). Since equality/hash/display
    /// treat Lazy as transparent, double-wrapping is semantically a no-op but
    /// wastes a slab slot.
    #[inline]
    fn lazy(&self, value: MettaValue) -> MettaValue {
        // Idempotency: don't double-wrap.
        if value.is_lazy() {
            return value;
        }
        let inner = self.alloc.alloc_value(MettaValueInner::Lazy(value));
        let flags = if value.has_variables_fast() {
            super::metta_value::FLAG_HAS_VARIABLES as u8
        } else {
            0
        };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn spanned(&self, value: MettaValue, span: crate::ir::Span) -> MettaValue {
        let span = self.alloc.alloc_span(span);
        let inner = self
            .alloc
            .alloc_value(MettaValueInner::Spanned(value, span));
        // Propagate variable flag through Spanned wrapper
        let flags = if value.has_variables_fast() {
            super::metta_value::FLAG_HAS_VARIABLES as u8
        } else {
            0
        };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn empty(&self) -> MettaValue {
        MettaValue::inline_empty()
    }

    /// Memoized `NotReducible` atom — Plan S0a (2026-05-13).
    ///
    /// HE-bisimilarity: emitted by `eval` (S4) when the argument is a grounded
    /// scalar at head, a variable-headed expression with no matching equations,
    /// or a `query` with empty result set. See `hyperon-experimental/lib/src/
    /// metta/interpreter.rs:546-548, 634` (`return_not_reducible`).
    ///
    /// Wraps the static `INLINE_NOT_REDUCIBLE_INNER` singleton — no slab
    /// allocation, no `OnceLock` overhead. The variant has a unique tag in
    /// `MettaValueInner` so consumers can use pointer-identity or `view()`
    /// matching for fast NotReducible detection.
    #[inline]
    fn not_reducible(&self) -> MettaValue {
        MettaValue::from_inner_tagged(
            &crate::backend::models::metta_value::INLINE_NOT_REDUCIBLE_INNER,
            0,
        )
    }

    /// Zero-cost identity conversion: V = MettaValue, so no serialization needed.
    #[inline]
    fn from_metta_value(&self, value: MettaValue) -> MettaValue {
        value
    }

    #[inline]
    fn deserialize(&self, bytes: &[u8]) -> Result<(MettaValue, usize), String> {
        deserialize_slab_value(self, bytes)
    }
}

// ============================================================================
// Slab-Native Deserialization (no temporary Bump arena)
// ============================================================================

/// Deserialize an MettaValue from bytes, allocating directly via GcFactory.
///
/// This replaces the old implementation that leaked a `Bump` arena per call.
/// Values are allocated directly into the global slab allocator.
// Generic over the factory (Inc 2): both `GcFactory` (slab) and `IndexFactory`
// (index arena) reuse this — the body calls only `MettaValueFactory` trait
// methods + factory-independent helpers (`read_varint`, handle reconstruction).
pub(crate) fn deserialize_slab_value<F: MettaValueFactory<MettaValue>>(
    factory: &F,
    bytes: &[u8],
) -> Result<(MettaValue, usize), String> {
    if bytes.is_empty() {
        return Err("unexpected end of input".to_string());
    }

    let tag = bytes[0];
    let rest = &bytes[1..];

    match tag {
        ATOM => {
            let (len, varint_size) = read_varint(rest)?;
            let start = varint_size;
            let end = start + len;
            if rest.len() < end {
                return Err("unexpected end of atom data".to_string());
            }
            let s = std::str::from_utf8(&rest[start..end])
                .map_err(|e| format!("invalid UTF-8 in atom: {}", e))?;
            Ok((factory.atom(s), 1 + end))
        }
        BOOL => {
            if rest.is_empty() {
                return Err("unexpected end of bool data".to_string());
            }
            Ok((factory.bool(rest[0] != 0), 2))
        }
        LONG => {
            if rest.len() < 8 {
                return Err("unexpected end of long data".to_string());
            }
            let n = i64::from_le_bytes(rest[..8].try_into().expect("8 bytes for i64"));
            Ok((factory.long(n), 9))
        }
        FLOAT => {
            if rest.len() < 8 {
                return Err("unexpected end of float data".to_string());
            }
            let f = f64::from_le_bytes(rest[..8].try_into().expect("8 bytes for f64"));
            Ok((factory.float(f), 9))
        }
        STRING => {
            let (len, varint_size) = read_varint(rest)?;
            let start = varint_size;
            let end = start + len;
            if rest.len() < end {
                return Err("unexpected end of string data".to_string());
            }
            let s = std::str::from_utf8(&rest[start..end])
                .map_err(|e| format!("invalid UTF-8 in string: {}", e))?;
            Ok((factory.string(s), 1 + end))
        }
        SEXPR => {
            let (count, varint_size) = read_varint(rest)?;
            let mut items = Vec::with_capacity(count);
            let mut offset = 1 + varint_size;
            for _ in 0..count {
                let (item, consumed) = deserialize_slab_value(factory, &bytes[offset..])?;
                items.push(item);
                offset += consumed;
            }
            Ok((factory.sexpr(items), offset))
        }
        UNIT_LEGACY => Ok((factory.unit(), 1)),
        ERROR => {
            // Phase 1.1 PT-canonical Error(Type, Ctx): deserialize in field
            // order — first field is Type, second is Ctx. factory.error()
            // takes (Type, Ctx) verbatim post-Phase 1.1.
            let (error_type, type_consumed) = deserialize_slab_value(factory, rest)?;
            let (ctx_val, ctx_consumed) =
                deserialize_slab_value(factory, &bytes[1 + type_consumed..])?;
            Ok((
                factory.error(error_type, ctx_val),
                1 + type_consumed + ctx_consumed,
            ))
        }
        TYPE => {
            let (inner, consumed) = deserialize_slab_value(factory, rest)?;
            Ok((factory.type_value(inner), 1 + consumed))
        }
        CONJUNCTION => {
            let (count, varint_size) = read_varint(rest)?;
            let mut goals = Vec::with_capacity(count);
            let mut offset = 1 + varint_size;
            for _ in 0..count {
                let (goal, consumed) = deserialize_slab_value(factory, &bytes[offset..])?;
                goals.push(goal);
                offset += consumed;
            }
            Ok((factory.conjunction(goals), offset))
        }
        UNIT => Ok((factory.unit(), 1)),
        QUOTED => {
            let (inner, consumed) = deserialize_slab_value(factory, rest)?;
            Ok((factory.quote(inner), 1 + consumed))
        }
        EMPTY => Ok((factory.empty(), 1)),
        NOT_REDUCIBLE => Ok((factory.not_reducible(), 1)),
        SPACE => {
            if rest.len() < 8 {
                return Err("unexpected end of space id".to_string());
            }
            let id = u64::from_le_bytes(rest[..8].try_into().expect("8 bytes for u64"));
            let mut offset = 9; // 1 (tag) + 8 (id)

            // Read name length and name bytes
            let (name_len, consumed) = read_varint(&rest[8..])?;
            offset += consumed;
            let name_start = 8 + consumed;
            if rest.len() < name_start + name_len {
                return Err("unexpected end of space name".to_string());
            }
            let name = std::str::from_utf8(&rest[name_start..name_start + name_len])
                .map_err(|e| format!("invalid UTF-8 in space name: {}", e))?
                .to_string();
            offset += name_len;

            // Read is_module_space flag
            if rest.len() < name_start + name_len + 1 {
                return Err("unexpected end of space is_module_space flag".to_string());
            }
            let is_module = rest[name_start + name_len] != 0;
            offset += 1;

            let handle = super::SpaceHandle::new_from_serialized(id, name, is_module);
            Ok((factory.space(handle), offset))
        }
        STATE => {
            if rest.len() < 8 {
                return Err("unexpected end of state id".to_string());
            }
            let id = u64::from_le_bytes(rest[..8].try_into().expect("8 bytes for u64"));
            Ok((factory.state(id), 9))
        }
        MEMO => {
            if rest.len() < 8 {
                return Err("unexpected end of memo id".to_string());
            }
            let _id = u64::from_le_bytes(rest[..8].try_into().expect("8 bytes for u64"));
            // Note: We can only deserialize the ID, not the full MemoHandle
            Ok((factory.unit(), 9)) // Placeholder - real impl needs handle registry
        }
        _ => Err(format!("unknown tag byte: 0x{:02X}", tag)),
    }
}

// ============================================================================
// Tests
// ============================================================================
// ============================================================================
// D1.2 — loom model of the Phase-D cooperative rendezvous protocol
// ============================================================================
//
// Compiled ONLY under `--cfg loom` (a dedicated capped lane — never in the
// normal build/test graph, so it never perturbs the lib-warnings gate). It
// model-checks the §D1 protocol (1 requestor + 2 workers) over loom's
// instrumented atomics/mutex/condvar, exploring ALL interleavings within
// `LOOM_MAX_PREEMPTIONS` to prove four properties:
//
//   (1) no_mark_before_all_parked   MARKING set ⇒ every worker is parked
//                                    (parked == NUM_WORKERS). This is the
//                                    CESK-completeness precondition: the
//                                    requestor only marks once every machine has
//                                    self-rooted (HB2 / TLA+ BeginMark).
//   (2) no_lost_wakeup              both workers eventually resume (their park
//                                    loops exit) after the requestor clears the
//                                    request + notifies (HB4). A lost wakeup
//                                    would leave a worker blocked → its
//                                    `join()` never returns → loom flags a
//                                    deadlock.
//   (3) no_self_root_after_mark     no worker is mid-self-root while MARKING is
//                                    set (the `self_rooting` in-progress count is
//                                    0 when MARKING flips true).
//   (4) requestor_exclusion         (separate sub-model) with two requestors
//                                    racing the `false→true` CAS, EXACTLY one
//                                    observes `true`.
//
// loom-MODEL adaptations (each load-bearing; mirror `index_arena.rs:1840`):
//   * loom surrogate state passed by `Arc` — NOT the real `static`s. loom can
//     reset only state it allocates inside `loom::model(..)`; a real `static`
//     would carry corruption across the (thousands of) explored executions.
//   * loom `Mutex::lock()` returns `LockResult` (std-shaped) ⇒ `.expect(..)`;
//     loom `Condvar::wait(guard)` CONSUMES and RETURNS the guard. loom's
//     `wait_timeout` does NOT actually time out (it just calls `wait`), so we
//     use `wait` inside a `while predicate` loop — loom explores the notify.
//   * `loom::thread::yield_now()` (not `hint::spin_loop()`) at the few spin
//     points so the blocked role yields to loom's scheduler.
//   * STRONG `compare_exchange` (not `_weak`): a weak CAS in a loom spin can fail
//     spuriously unboundedly within ONE execution and overflow the coroutine
//     stack. The protocol proof is identical (weak only adds benign retries).
//   * Small fixed sizes (2 workers, 1-element root append, one poll point per
//     worker) keep the schedule tree tractable under `LOOM_MAX_PREEMPTIONS=2`.
//
// VERIFIED-GREEN command (capped lane; `-C target-cpu=native` re-added because
// setting RUSTFLAGS overrides .cargo/config.toml's gxhash AES/SSE2 flags; the
// large crate's DEBUG frames overflow loom's coroutine stack ⇒ `--release`):
//   RUSTFLAGS="--cfg loom -C target-cpu=native" LOOM_MAX_PREEMPTIONS=2 \
//     systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0 \
//       -p CPUQuota=1200% \
//     cargo test --release --lib \
//       backend::models::gc_allocator::loom_rendezvous -- --nocapture
#[cfg(loom)]
mod loom_rendezvous {
    use loom::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use loom::sync::{Arc, Condvar, Mutex};
    use loom::thread;

    /// Loom surrogate for the real rendezvous statics (which loom cannot reset
    /// between iterations). One instance per `loom::model` execution, shared by
    /// `Arc`. Field roles mirror the production primitives 1:1.
    struct Rdv {
        /// Surrogate for `GC_REQUESTED` (request/resume signal).
        gc_requested: AtomicBool,
        /// Surrogate for `WORKERS_PARKED_FOR_GC` (buffer-HB carrier + parked
        /// observability). Incremented by a worker AFTER it appends its root.
        parked: AtomicU32,
        /// Surrogate for `WORKER_ROOT_BUFFER` (worker self-rooted handles — here
        /// just sentinel `u32`s, one per worker).
        buffer: Mutex<Vec<u32>>,
        /// In-progress self-root count: a worker holds it >0 across {append,
        /// parked.fetch_add}. Used to assert (3) no_self_root_after_mark.
        self_rooting: AtomicU32,
        /// Set by the requestor once `parked == NUM_WORKERS`; the moment it marks.
        marking: AtomicBool,
        /// Surrogate for `RENDEZVOUS_MUTEX`/`CONDVAR` (requestor waits parked).
        rdv_mutex: Mutex<()>,
        rdv_cond: Condvar,
        /// Surrogate for `RESUME_MUTEX`/`CONDVAR` (worker waits resume).
        resume_mutex: Mutex<()>,
        resume_cond: Condvar,
    }

    impl Rdv {
        fn new() -> Self {
            Rdv {
                gc_requested: AtomicBool::new(false),
                parked: AtomicU32::new(0),
                buffer: Mutex::new(Vec::new()),
                self_rooting: AtomicU32::new(0),
                marking: AtomicBool::new(false),
                rdv_mutex: Mutex::new(()),
                rdv_cond: Condvar::new(),
                resume_mutex: Mutex::new(()),
                resume_cond: Condvar::new(),
            }
        }
    }

    /// WORKER body — surrogate of `worker_park_and_root` PLUS its poll-point
    /// guard. `tag` is this worker's sentinel root.
    ///
    /// Precondition (matches "a safepoint observes an ALREADY-published request"
    /// — HB1): the requestor sets `gc_requested=true` BEFORE the workers run, so
    /// each worker's poll point observes the request and parks. (A worker that
    /// observed `false` would simply not park — not the rendezvous under test.)
    fn worker(r: &Arc<Rdv>, tag: u32) {
        // Poll point: observe the request (HB1: requestor's Release → this
        // Acquire). It is already true by construction.
        if r.gc_requested.load(Ordering::Acquire) {
            // --- self-root window: open across the buffer append ONLY ---
            // Models the real worker's `collect_machine_roots_live` +
            // `WORKER_ROOT_BUFFER.lock().extend(..)` — the append is FULLY
            // complete before the worker announces it is parked.
            r.self_rooting.fetch_add(1, Ordering::AcqRel);
            // (1) publish my root.
            r.buffer.lock().expect("buffer lock").push(tag);
            // Self-root window CLOSES here — BEFORE `parked` is bumped. This
            // mirrors the real ordering `append → drop_eval_guard_for_safepoint()
            // (active--) → WORKERS_PARKED_FOR_GC.fetch_add(1)`: a worker finishes
            // self-rooting (and drops its EvalGuard) BEFORE the signal the
            // requestor gates on. Consequently, once the requestor observes the
            // gate satisfied (here `parked == NUM_WORKERS`; in production
            // `active_evaluator_count()==0`), NO worker can still be self-rooting,
            // so the `self_rooting == 0` assert below holds. (loom found that
            // closing this window AFTER `parked++` was a MODEL bug — the gate
            // could open while a worker's `fetch_sub` was still pending; it does
            // NOT reflect the real protocol, where the append precedes the gate
            // signal. This is exactly the kind of ordering subtlety loom exists to
            // surface.)
            r.self_rooting.fetch_sub(1, Ordering::AcqRel);
            // (2) signal parked; AcqRel release-fences the append (HB2).
            r.parked.fetch_add(1, Ordering::AcqRel);
            // (3) wake the requestor — lost-wakeup-safe: mutex held across notify.
            {
                let _g = r.rdv_mutex.lock().expect("rdv lock");
                r.rdv_cond.notify_all();
            }
            // (4) park until the requestor clears gc_requested (HB4). loom's
            // `wait` consumes+returns the guard; the `while` re-checks the
            // predicate so loom must schedule the requestor's notify to exit.
            let mut g = r.resume_mutex.lock().expect("resume lock");
            while r.gc_requested.load(Ordering::Acquire) {
                g = r.resume_cond.wait(g).expect("resume wait");
            }
        }
    }

    /// REQUESTOR body — surrogate of `requestor_wait_for_parked` + the §D1
    /// mark/resume tail. `num_workers` is the rendezvous size.
    fn requestor(r: &Arc<Rdv>, num_workers: u32) {
        // Wait until all workers have parked. We key on `parked` (the surrogate
        // for the buffer-HB carrier) rather than an `active==0` surrogate to keep
        // the model self-contained; the lost-wakeup-safe handshake is identical
        // (rdv_mutex held across predicate + wait, matching the worker's notify).
        {
            let mut g = r.rdv_mutex.lock().expect("rdv lock");
            while r.parked.load(Ordering::Acquire) < num_workers {
                g = r.rdv_cond.wait(g).expect("rdv wait");
            }
        }

        // (1) no_mark_before_all_parked: by the loop predicate, all parked now.
        assert_eq!(
            r.parked.load(Ordering::Acquire),
            num_workers,
            "requestor may mark only once every worker has parked (HB2 / BeginMark)"
        );
        // (3) no_self_root_after_mark: no worker may be mid-self-root as we mark.
        assert_eq!(
            r.self_rooting.load(Ordering::Acquire),
            0,
            "no worker may be self-rooting while the requestor marks"
        );

        // Mark window: flip MARKING, drain the union, then clear it. The
        // assertions above bracket the instant MARKING becomes observable.
        r.marking.store(true, Ordering::Release);

        // Drain the worker roots (∪ E₀ ∪ driver-C in production). Every worker's
        // sentinel must be present — the union is complete (CESK completeness).
        let drained = {
            let mut buf = r.buffer.lock().expect("buffer lock");
            let v: Vec<u32> = buf.drain(..).collect();
            v
        };
        assert_eq!(
            drained.len() as u32,
            num_workers,
            "the marked union must contain every parked worker's root"
        );

        r.marking.store(false, Ordering::Release);

        // (2) Resume: clear gc_requested + notify, BOTH under resume_mutex
        // (lost-wakeup-safe — HB4: matches the worker holding resume_mutex across
        // its `while is_gc_requested { wait }`).
        {
            let _g = r.resume_mutex.lock().expect("resume lock");
            r.gc_requested.store(false, Ordering::Release);
            r.resume_cond.notify_all();
        }
    }

    /// 1 requestor + 2 workers: the full rendezvous. Proves (1), (2), (3).
    #[test]
    fn loom_rendezvous_requestor_two_workers() {
        loom::model(|| {
            const NUM_WORKERS: u32 = 2;
            let r = Arc::new(Rdv::new());

            // Publish the GC request up-front (HB1: a safepoint observes an
            // already-set request), then spawn the participants.
            r.gc_requested.store(true, Ordering::Release);

            let w1 = {
                let r = r.clone();
                thread::spawn(move || worker(&r, 0xA))
            };
            let w2 = {
                let r = r.clone();
                thread::spawn(move || worker(&r, 0xB))
            };
            let req = {
                let r = r.clone();
                thread::spawn(move || requestor(&r, NUM_WORKERS))
            };

            // (2) no_lost_wakeup: every join must return. If a worker missed its
            // resume notification it would block on `resume_cond.wait` forever and
            // loom would report a deadlock here.
            req.join().expect("requestor");
            w1.join().expect("worker 1 — possible lost wakeup");
            w2.join().expect("worker 2 — possible lost wakeup");

            // Final state: request cleared, both parked, marking finished.
            assert!(
                !r.gc_requested.load(Ordering::Acquire),
                "gc_requested must be cleared after the cycle"
            );
            assert_eq!(
                r.parked.load(Ordering::Acquire),
                NUM_WORKERS,
                "both workers must have parked exactly once"
            );
            assert!(
                !r.marking.load(Ordering::Acquire),
                "MARKING must be cleared after the mark window"
            );
        });
    }

    /// (4) requestor_exclusion: two threads race the `false→true` CAS (the
    /// surrogate of `begin_gc_rendezvous`); EXACTLY one wins. Mirrors the
    /// production `GC_REQUESTOR_ACTIVE.compare_exchange(false, true, AcqRel,
    /// Acquire)`. Loom explores both orderings.
    #[test]
    fn loom_rendezvous_requestor_exclusion() {
        loom::model(|| {
            let owner = Arc::new(AtomicBool::new(false));
            let winners = Arc::new(AtomicU32::new(0));

            let t1 = {
                let owner = owner.clone();
                let winners = winners.clone();
                thread::spawn(move || {
                    // STRONG CAS (see module adaptations).
                    if owner
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        winners.fetch_add(1, Ordering::AcqRel);
                    }
                })
            };
            let t2 = {
                let owner = owner.clone();
                let winners = winners.clone();
                thread::spawn(move || {
                    if owner
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        winners.fetch_add(1, Ordering::AcqRel);
                    }
                })
            };

            t1.join().expect("requestor 1");
            t2.join().expect("requestor 2");

            assert_eq!(
                winners.load(Ordering::Acquire),
                1,
                "exactly one requestor may win the begin_gc_rendezvous CAS"
            );
        });
    }
}

// ============================================================================
// LOOM MODEL — E5 the STRADDLE re-park deadlock + the Fix-B B-CLOSURE
// ============================================================================
//
// Compiled ONLY under `--cfg loom` (a dedicated capped lane; never in the normal
// build/test graph). Companion to `loom_rendezvous` above — that model checks the
// §D1 rendezvous (1 requestor + N fungible-count workers, ONE cycle, no witness
// slots / gen / straddle), which STRUCTURALLY cannot reach the E5 bug. This model
// adds exactly what the E5 root-cause needs (per
// docs/cesk-gc/e1-flip-deadlock-straddle-rootcause-2026-06-03.md §"loom"):
//
//   (i)   a PER-SLOT witness `{acq, pub_, occ}` (one slot — worker A) with the
//         production strict-`>` predicate `published>=cur_gen OR acquired>cur_gen`.
//   (ii)  `GC_CYCLE_GEN` bumped at cycle END (NOT at start) — the load-bearing
//         premise; `GC_CYCLE_STARTED` set at the driver prologue (Fix-B variants).
//   (iii) the driver's 3-store / 3-lock cycle-end teardown as 3 SEPARATE steps in
//         the BUGGY order: (1) gen-bump under RENDEZVOUS_MUTEX, (2) gip-clear under
//         GC_PROGRESS_MUTEX, (3) resume under RESUME_MUTEX.
//   (iv)  the straddle re-park loop VERBATIM (the three production variants gated by
//         `VARIANT`): BUG-REPRO (gen-gated, no `started`), Fix-B-without-closure
//         (`started`-gated re-park but the OLD non-publishing rejoin tail), and
//         Fix-B-with-closure (the real fix — `started`-gated + the B-closure
//         re-read in the rejoin tail).
//   (v)   `GC_CYCLE_STARTED` for the Fix-B variants.
//
// CRITICAL loom adaptation (load-bearing, per the doc): every production
// `wait_for(_, 5s)` is modelled as a plain `wait` (NO timeout). The real 5 s
// timeout is a benign liveness backstop that re-checks the predicate; modelling it
// as `wait` makes loom report a deadlock IFF a predicate is PERMANENTLY false (the
// E5 bug / the R1 relocated-hang), ignoring benign 5-s-recoverable lost-wakeups.
//
// SCENARIO (two cycles K, K+1 — see the doc §"loom MUST assert R1"):
//   * Worker A has ALREADY parked for and been released from cycle K=1 (occupied,
//     published=1, acquired=1, my_reparked_gen=1) and is now in the straddle.
//   * Cycle 1 (= "K", the cycle A parked for): the driver does its TEARDOWN (the
//     buggy 3 steps). A's straddle observes the teardown window.
//   * Cycle 2 (= "K+1", a genuinely NEW triggered cycle): runs ONLY in the Fix-B
//     variants (where the bug is fixed and A reaches a sane state). In BUG-REPRO the
//     driver idles after cycle-1 teardown (faithful: "the driver returns to idle"),
//     so A's phantom re-park for gen=2 — a cycle no driver ever runs — is PERMANENT.
//
// EXPECTED loom outcomes (the user's witness protocol — this IS the gate):
//   * BUG-REPRO              → manual expected-fail model: `a.join()` MUST DEADLOCK
//                              (confirms the model captures the bug; loom finds the
//                              gen-read-in-teardown-window schedule and the phantom
//                              park hangs forever).
//   * Fix-B-without-closure  → manual expected-fail model: R1 MUST FAIL (the relocated
//                              hang: A breaks, cycle 2 starts, A sits in the
//                              non-publishing admission wait while the driver waits on
//                              A's occupied,published<2 slot → both block forever).
//   * Fix-B-with-closure     → green model: ALL joins return + R1 holds
//                              (driver-waiting-on-A ⟹ A
//                              re-observes started≥2 and REPUBLISHES) + the safety
//                              co-assertion (no sweep while A occupied∧published<cur)
//                              + the RENDEZVOUS→RESUME lock-order assertion.
//
// VERIFIED-GREEN command (capped lane; `-C target-cpu=native` re-added because
// setting RUSTFLAGS overrides .cargo/config.toml's gxhash AES/SSE2 flags; the large
// crate's DEBUG frames overflow loom's coroutine stack ⇒ `--release`):
//   RUSTFLAGS="--cfg loom -C target-cpu=native" LOOM_MAX_PREEMPTIONS=3 \
//     systemd-run --user --scope -q -p MemoryMax=22G -p MemorySwapMax=0 \
//       -p CPUQuota=1600% \
//     cargo test --release --lib loom_straddle -- --nocapture
// The two expected-fail variants are `#[ignore]` because loom's deadlock report can
// abort during cleanup instead of unwinding cleanly through `#[should_panic]`.
#[cfg(loom)]
mod loom_straddle {
    use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use loom::sync::{Arc, Condvar, Mutex};
    use loom::thread;

    /// Which straddle implementation the worker runs. The driver + statics are
    /// IDENTICAL across variants; only the worker's straddle body + whether cycle 2
    /// is driven change — so the model isolates exactly the fix's effect.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Variant {
        /// The BUG: straddle re-park gates on `GC_CYCLE_GEN` (the teardown-window
        /// gen), with NO `GC_CYCLE_STARTED`. Driver idles after cycle-1 teardown.
        BugRepro,
        /// Fix-B `started`-gate in the re-park phase, but the OLD non-publishing
        /// rejoin tail (NO B-closure re-read). Driver runs cycle 2 → exposes R1.
        FixBNoClosure,
        /// The REAL fix: `started`-gate + the B-closure re-read in the rejoin tail.
        /// Driver runs cycle 2; everything must converge.
        FixBWithClosure,
    }

    /// Loom surrogate for the real E5 statics + the single witness slot (worker A's).
    /// One instance per `loom::model` execution, shared by `Arc`. Field roles mirror
    /// the production primitives 1:1.
    struct St {
        /// Surrogate for `GC_CYCLE_GEN` (bumped at cycle END only — the premise).
        gen: AtomicU64,
        /// Surrogate for `GC_CYCLE_STARTED` (set at the driver prologue; Fix-B).
        started: AtomicU64,
        /// Surrogate for `GC_IN_PROGRESS` (the `_gip` guard's flag).
        gip: AtomicBool,

        // ── Worker A's single witness slot ──
        slot_acq: AtomicU64,
        slot_pub: AtomicU64,
        slot_occ: AtomicBool,

        /// Set true the instant the driver SWEEPS cycle 2 (after its witness wait).
        /// Read by the safety co-assertion.
        swept_cur_gen: AtomicU64,

        // ── Locks (production names) ──
        /// `RENDEZVOUS_MUTEX` — guards the gen bump + the gen-bump notify + the
        /// witness wait + the gen-gated resume wait (E5: the resume wait MOVED onto
        /// this mutex/condvar so the gen-bump and the gen-wait are Mesa-correct on the
        /// SAME lock — no cross-mutex lost-wakeup).
        rdv_mutex: Mutex<()>,
        rdv_cond: Condvar,
        /// `GC_PROGRESS_MUTEX` — guards the gip clear + the straddle admission/else
        /// waits + the `set_current_cycle_started` notify.
        gip_mutex: Mutex<()>,
        gip_cond: Condvar,

        /// E5 lock-order witness (NOW expected to stay 0): incremented while a thread
        /// HOLDS rdv_mutex inside ANOTHER lock's critical section (a nest), so the model
        /// can assert NO such nest exists after the E5 fix eliminated the cross-mutex
        /// RENDEZVOUS↔RESUME edge. (Pre-E5 this tracked RESUME holders for the
        /// RENDEZVOUS→RESUME nest; that nest is gone, so it now documents the absence of
        /// ANY rdv-inside-other nest.) A thread WAITING on rdv_cond has RELEASED
        /// rdv_mutex, so it does NOT count — only genuine holders inside a nest do.
        rdv_nest_held: AtomicU64,
    }

    impl St {
        fn new() -> Self {
            St {
                gen: AtomicU64::new(1),     // cycle K = 1
                started: AtomicU64::new(0), // no cycle started yet
                gip: AtomicBool::new(false),
                // A parked for cycle 1 and was released: occupied, published=1.
                slot_acq: AtomicU64::new(1),
                slot_pub: AtomicU64::new(1),
                slot_occ: AtomicBool::new(true),
                swept_cur_gen: AtomicU64::new(0),
                rdv_mutex: Mutex::new(()),
                rdv_cond: Condvar::new(),
                gip_mutex: Mutex::new(()),
                gip_cond: Condvar::new(),
                rdv_nest_held: AtomicU64::new(0),
            }
        }

        /// Production `witness_slot_satisfied`: `published>=cur_gen OR acquired>cur_gen`.
        fn slot_satisfied(&self, cur_gen: u64) -> bool {
            self.slot_pub.load(Ordering::Acquire) >= cur_gen
                || self.slot_acq.load(Ordering::Acquire) > cur_gen
        }
    }

    /// Surrogate for `set_current_cycle_started`: Release store + notify gip_cond
    /// (lost-wakeup-safe wake of the straddle teardown-window else-arm). NEVER under
    /// rdv_mutex (the straddle body locks rdv_mutex non-reentrantly).
    fn set_started(s: &Arc<St>, g: u64) {
        s.started.store(g, Ordering::Release);
        let _g = s.gip_mutex.lock().expect("gip lock");
        s.gip_cond.notify_all();
    }

    /// Surrogate for `GcInProgressGuard::drop`: clear gip under gip_mutex + notify
    /// gip_cond (the production Drop does exactly this).
    fn drop_gip(s: &Arc<St>) {
        {
            let _g = s.gip_mutex.lock().expect("gip lock");
            s.gip.store(false, Ordering::Release);
        }
        s.gip_cond.notify_all();
    }

    /// Surrogate for `worker_resume_wait_for_cycle(my)` (E5 Mesa-correct) — gen-gated
    /// wait under `rdv_mutex`/`rdv_cond` (the SAME mutex/condvar the gen bump +
    /// gen-bump-notify hold in `driver_cycle`'s teardown), plain `wait` (NO timeout, so
    /// a genuine lost-wakeup shows as a PERMANENT deadlock). This waiter holds NO other
    /// lock and, while in `wait`, has RELEASED `rdv_mutex` — so it does NOT touch
    /// `rdv_nest_held` (it is the OUTERMOST lock here, not a nest). Mesa discipline: the
    /// `while gen==my` predicate is re-checked under the lock on each wake, absorbing any
    /// spurious wake (e.g. a parker's `rdv_cond` notify).
    fn worker_resume_wait_for_cycle(s: &Arc<St>, my: u64) {
        let mut g = s.rdv_mutex.lock().expect("rdv lock");
        while s.gen.load(Ordering::Acquire) == my {
            g = s.rdv_cond.wait(g).expect("rdv wait");
        }
        drop(g);
    }

    /// Surrogate for `worker_park_and_root_in_cycle(roots, g)`: under rdv_mutex,
    /// gen-gated publish (`note_reified_park`: stamp published=g IFF gen==g), then
    /// the resume-wait. A straggler whose cycle already ended (gen!=g) drops its
    /// roots and does NOT stamp (production straggler exclusion).
    fn worker_park_and_root_in_cycle(s: &Arc<St>, g: u64) {
        {
            let _l = s.rdv_mutex.lock().expect("rdv lock");
            if s.gen.load(Ordering::Acquire) == g {
                // THE STAMP — the sole published-setter (gen-gated, occupied&&acq==g).
                if s.slot_occ.load(Ordering::Acquire) && s.slot_acq.load(Ordering::Acquire) == g {
                    s.slot_pub.store(g, Ordering::Release);
                }
                // wake the requestor's witness wait (under rdv_mutex — lost-wakeup-safe).
                s.rdv_cond.notify_all();
            }
        }
        worker_resume_wait_for_cycle(s, g);
    }

    /// WORKER A — the FAITHFUL lifecycle: (i) park for cycle 1 (publish + resume-wait
    /// until cycle 1 ENDS), THEN (ii) the straddle body VERBATIM from
    /// `reacquire_eval_guard_after_safepoint_full` (gc_allocator.rs), parameterised by
    /// `VARIANT`, THEN (iii) RELEASE the witness slot (the outermost `EvalGuard::drop` →
    /// `witness_release_slot`). Returns when A fully rejoins+finishes (its `join()`
    /// returning = no deadlock).
    ///
    /// ── FIDELITY (E5, 2026-06-03): why (i) + (iii) are LOAD-BEARING ──────────────────
    /// An earlier model started A mid-straddle (`my_reparked_gen=1`, gip=false) and let A
    /// RETURN with its slot still occupied. Both were UNFAITHFUL and SPURIOUSLY
    /// deadlocked Fix-B-with-closure:
    ///   (i) In production A reaches the straddle ONLY after parking for cycle 1 and being
    ///       released by cycle 1's teardown (gen already advanced to 2) — A never
    ///       "straddles cycle 1 from scratch with gip=false". Without (i), loom schedules
    ///       A's entire straddle BEFORE the driver even takes cycle 1's gip: A breaks
    ///       immediately (started=0, gip=false) and terminates with published=1, then
    ///       cycle 2 blocks on its stale slot — a phantom the B-closure cannot catch
    ///       (started was 0 the whole time A ran).
    ///  (iii) In production a worker that FINISHES drops its outermost `EvalGuard` →
    ///       `witness_release_slot` clears `occupied` + notifies (the LOST-NOTIFY FIX),
    ///       removing it from the driver's witness predicate. Without (iii), a returned A
    ///       is an immortal occupied-but-unpublished slot that hangs every later cycle —
    ///       the witness-sole-gate proof's S2 ("finished+released") is simply missing.
    /// With BOTH, Fix-B-with-closure converges (B-closure republishes OR the release
    /// satisfies the wait) while Fix-B-no-closure STILL fails R1 for the RIGHT reason: A
    /// blocks in the non-publishing admission wait (cycle 2 holds gip) while the driver
    /// blocks on A's occupied,published=1<2 slot — neither's release runs (mutual block).
    fn worker_a(s: &Arc<St>, variant: Variant) {
        // (i) PARK for cycle 1 (faithful entry): publish=1 (already the init state) and
        // resume-wait until cycle 1 ENDS (gen advances to 2). Production: A reaches the
        // straddle via `worker_park_and_root_in_cycle(1)`'s trailing resume-wait
        // returning. Only THEN does A run the straddle — in the teardown window or after
        // a later cycle has started, never from a pristine gip=false/started=0 state.
        worker_park_and_root_in_cycle(s, 1);

        let mut my_reparked_gen: u64 = 1;
        'straddle: loop {
            // ── Straddle re-park phase ──
            loop {
                match variant {
                    Variant::BugRepro => {
                        // THE BUG: gen-gated (no `started`).
                        let g = s.gen.load(Ordering::Acquire);
                        if s.gip.load(Ordering::Acquire) && g != my_reparked_gen {
                            // restamp acquired=g (occupied stays true)
                            s.slot_acq.store(g, Ordering::Release);
                            worker_park_and_root_in_cycle(s, g);
                            my_reparked_gen = g;
                            continue;
                        } else if !s.gip.load(Ordering::Acquire) {
                            break;
                        } else {
                            worker_resume_wait_for_cycle(s, my_reparked_gen);
                            continue;
                        }
                    }
                    Variant::FixBNoClosure | Variant::FixBWithClosure => {
                        // Fix B: `started`-gated re-park.
                        let started = s.started.load(Ordering::Acquire);
                        if started > my_reparked_gen {
                            s.slot_acq.store(started, Ordering::Release); // restamp acquired
                            worker_park_and_root_in_cycle(s, started);
                            my_reparked_gen = started;
                            continue;
                        }
                        if s.gip.load(Ordering::Acquire) {
                            if s.gen.load(Ordering::Acquire) == my_reparked_gen {
                                worker_resume_wait_for_cycle(s, my_reparked_gen);
                            } else {
                                // teardown-window else-arm: wait on gip transition (not spin),
                                // re-checking gip AND started under gip_mutex (plain wait).
                                let mut l = s.gip_mutex.lock().expect("gip lock");
                                while s.gip.load(Ordering::Acquire)
                                    && s.started.load(Ordering::Acquire) <= my_reparked_gen
                                {
                                    l = s.gip_cond.wait(l).expect("gip wait");
                                }
                            }
                            continue;
                        }
                        break;
                    }
                }
            }

            // ── Rejoin tail ──
            // restamp acquired = current gen (V4; occupied stays true). NOTE: this does
            // NOT publish — only a genuine park does.
            s.slot_acq.store(s.gen.load(Ordering::Acquire), Ordering::Release);

            match variant {
                // BUG-REPRO + Fix-B-WITHOUT-closure: the OLD non-publishing admission
                // wait (no B-closure re-read of `started`).
                Variant::BugRepro | Variant::FixBNoClosure => loop {
                    if !s.gip.load(Ordering::Acquire) {
                        break;
                    }
                    let mut l = s.gip_mutex.lock().expect("gip lock");
                    while s.gip.load(Ordering::Acquire) {
                        l = s.gip_cond.wait(l).expect("gip wait");
                    }
                    drop(l);
                },
                // Fix-B-WITH-closure: re-read `started` INSIDE the admission loop; if it
                // advanced past my_reparked_gen, go BACK into the straddle re-park
                // (republish). THE B-CLOSURE (R1).
                Variant::FixBWithClosure => loop {
                    if s.started.load(Ordering::Acquire) > my_reparked_gen {
                        continue 'straddle;
                    }
                    if !s.gip.load(Ordering::Acquire) {
                        break;
                    }
                    let mut l = s.gip_mutex.lock().expect("gip lock");
                    while s.gip.load(Ordering::Acquire)
                        && s.started.load(Ordering::Acquire) <= my_reparked_gen
                    {
                        l = s.gip_cond.wait(l).expect("gip wait");
                    }
                    drop(l);
                },
            }
            break 'straddle;
        }
        // (iii) A has fully rejoined and now FINISHES its whole evaluation → the outermost
        // `EvalGuard::drop` → `witness_release_slot`: clear `occupied` (Release) + notify
        // `rdv_cond` under `rdv_mutex` (the production LOST-NOTIFY FIX). A driver blocked
        // in its witness wait on A's (now-released) slot wakes, re-walks, SKIPS the
        // un-occupied slot, and proceeds — so A's finish can never strand a later cycle.
        // Faithful to the V4 slot lifecycle: release is ONCE, here, at the outermost drop.
        s.slot_occ.store(false, Ordering::Release);
        {
            let _l = s.rdv_mutex.lock().expect("rdv lock");
            s.rdv_cond.notify_all();
        }
    }

    /// Run ONE driver cycle `cyc` (= cur_gen): prologue (set started + gip already
    /// held), witness-wait on A's slot, sweep, then the BUGGY-ORDER teardown
    /// (gen-bump → gip-clear → resume). `fix_b` ⇒ publish `GC_CYCLE_STARTED`.
    /// Precondition: the caller has already taken the `_gip` guard (set gip=true).
    fn driver_cycle(s: &Arc<St>, cyc: u64, fix_b: bool) {
        // (3) prologue: publish that a LIVE driver STARTED `cyc`. (Fix-B only.)
        debug_assert!(s.gip.load(Ordering::Acquire), "gip must be set at prologue");
        if fix_b {
            set_started(s, cyc);
        }
        // (4) WITNESS WAIT: block until A's occupied slot satisfies the strict-`>`
        // predicate for `cyc` (lost-wakeup-safe: rdv_mutex held across predicate +
        // wait, matching the parker's stamp+notify; plain `wait`, no timeout).
        {
            let mut g = s.rdv_mutex.lock().expect("rdv lock");
            while s.slot_occ.load(Ordering::Acquire) && !s.slot_satisfied(cyc) {
                g = s.rdv_cond.wait(g).expect("rdv wait");
            }
        }
        // SAFETY co-assertion: at the sweep instant, A's slot is NOT
        // (occupied ∧ published<cyc). The witness wait guarantees it.
        assert!(
            !(s.slot_occ.load(Ordering::Acquire) && s.slot_pub.load(Ordering::Acquire) < cyc),
            "SAFETY: the driver swept cur_gen {} while worker A's slot was occupied and \
             published<cur_gen (published={}) — an unpublished live machine would be \
             freed (UAF). The witness wait must not have held.",
            cyc,
            s.slot_pub.load(Ordering::Acquire),
        );
        s.swept_cur_gen.store(cyc, Ordering::Release);

        // ── BUGGY-ORDER 3-store teardown (the production order; gc_driver.rs:268-270) ──
        // The gen (naming the NEXT cycle) becomes visible at step (1), BEFORE gip is
        // cleared at step (2) — the ordering inversion the straddle `started`-gate must
        // tolerate. This order is preserved verbatim (the fix is in the straddle/wait,
        // NOT in re-ordering the safety-critical teardown).
        // (1) end_rendezvous_cycle (E5 Mesa-correct): bump gen under rdv_mutex AND notify
        //     rdv_cond under the SAME lock, after the bump — so W2's gen-gated resume wait
        //     (now on rdv_mutex/rdv_cond) cannot miss the wake. NO resume_mutex nest: the
        //     E5 fix moved the gen-wait onto rdv_mutex, ELIMINATING the cross-mutex
        //     RENDEZVOUS→RESUME edge the pre-E5 model asserted against.
        {
            // LOCK-ORDER witness (now expected 0): no thread holds rdv_mutex inside
            // another lock's critical section while we take it here — i.e. no rdv-inside-
            // other nest exists. A thread WAITING on rdv_cond has released rdv_mutex, so it
            // does not count. (Pre-E5 this guarded RENDEZVOUS→RESUME; that nest is gone.)
            let _l = s.rdv_mutex.lock().expect("rdv lock");
            assert_eq!(
                s.rdv_nest_held.load(Ordering::Acquire),
                0,
                "LOCK ORDER (E5): no thread may hold rdv_mutex INSIDE another lock while \
                 the teardown acquires it; the E5 fix removed the cross-mutex \
                 RENDEZVOUS↔RESUME nest, so this must stay 0 (a thread merely WAITING on \
                 rdv_cond has released the mutex and does not count)."
            );
            s.gen.fetch_add(1, Ordering::AcqRel);
            // E5: notify rdv_cond (the gen-resume-waiters' condvar) under THIS lock, after
            // the bump. Mesa-correct: a woken W2 waiter re-checks `gen != my` under the
            // same mutex; the driver's own witness wait (also rdv_cond) has already
            // returned, and would re-check its predicate anyway.
            s.rdv_cond.notify_all();
        }
        // (2) drop _gip: clear gip under gip_mutex + notify gip_cond.
        drop_gip(s);
        // (3) resume_workers: in production this clears GC_REQUESTED + notifies
        //     RESUME_CONDVAR for the WorkerEnter gate (`worker_wait_for_resume`), a
        //     DISTINCT waiter NOT modelled here (this model has only worker A's
        //     gen-resume path, which step (1) already woke via rdv_cond). Modelled as a
        //     no-op to keep the 3-step teardown shape faithful.
    }

    /// The full driver: run cycle 1's TEARDOWN-relevant cycle, then (Fix-B variants
    /// only) a genuinely-new cycle 2. In BUG-REPRO the driver IDLES after cycle 1
    /// (faithful: "the driver returns to idle"), so A's phantom re-park for gen=2 is
    /// permanent.
    fn driver(s: &Arc<St>, variant: Variant) {
        let fix_b = variant != Variant::BugRepro;

        // Cycle K=1: A already parked+published for it. The driver takes the `_gip`
        // guard for cycle 1 (CAS false→true), runs the cycle (witness already
        // satisfied: A published=1), and tears it down in the buggy order. A's
        // straddle observes the teardown window (gen 1→2 visible before gip clears).
        let entered = s
            .gip
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok();
        assert!(entered, "driver must win the cycle-1 gip CAS (sole collector)");
        driver_cycle(s, 1, fix_b);

        // Cycle K+1=2: a genuinely NEW triggered cycle — ONLY in the Fix-B variants
        // (the R1 stressor). After cycle 1 fully ended, gen==2; cycle 2's prologue
        // reads cur_gen==2. A must (with the B-closure) republish for gen 2.
        if fix_b {
            let entered2 = loop {
                match s
                    .gip
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                {
                    Ok(_) => break true,
                    Err(_) => thread::yield_now(),
                }
            };
            assert!(entered2, "driver must win the cycle-2 gip CAS");
            let cur_gen2 = s.gen.load(Ordering::Acquire); // == 2 (cycle 1 end-bumped)
            driver_cycle(s, cur_gen2, true);
        }
    }

    /// BUG-REPRO: the gen-gated straddle (no `started`) + driver idling after cycle-1
    /// teardown. `a.join()` MUST DEADLOCK — A slips its gen read into cycle-1's
    /// teardown window (gen=2 ∧ gip=true), phantom-parks for gen=2 (a cycle no driver
    /// runs), and waits for gen!=2 forever. Loom reports the deadlock (confirms the
    /// model is NON-VACUOUS — it captures the real bug).
    #[test]
    #[ignore = "manual expected-fail loom model: deadlocks when the bug is present"]
    fn loom_straddle_bug_repro_deadlocks() {
        loom::model(|| {
            let s = Arc::new(St::new());
            let a = {
                let s = s.clone();
                thread::spawn(move || worker_a(&s, Variant::BugRepro))
            };
            let d = {
                let s = s.clone();
                thread::spawn(move || driver(&s, Variant::BugRepro))
            };
            // If A phantom-parks, its join never returns → loom flags a DEADLOCK here
            // (the expected, model-validating outcome). The driver always returns.
            d.join().expect("driver");
            a.join().expect("worker A");
        });
    }

    /// Fix-B-WITHOUT-closure: the `started`-gate closes the original teardown-window
    /// phantom, but the OLD non-publishing rejoin tail RELOCATES the hang (R1). A
    /// breaks in cycle-1's teardown window, cycle 2 starts, and A sits in the
    /// non-publishing admission wait (gip=true held by cycle-2's driver) while the
    /// driver's witness wait blocks on A's occupied,published=1<2 slot → both forever.
    /// R1 MUST FAIL ⇒ loom reports the deadlock (PROVES the B-closure is necessary).
    #[test]
    #[ignore = "manual expected-fail loom model: deadlocks without the B-closure"]
    fn loom_straddle_fix_b_no_closure_fails_r1() {
        loom::model(|| {
            let s = Arc::new(St::new());
            let a = {
                let s = s.clone();
                thread::spawn(move || worker_a(&s, Variant::FixBNoClosure))
            };
            let d = {
                let s = s.clone();
                thread::spawn(move || driver(&s, Variant::FixBNoClosure))
            };
            // EXPECTED: at least one schedule deadlocks (the post-break-K+1-starts R1
            // schedule). Loom reports it — the gate's "Fix-B-without-closure fails R1".
            d.join().expect("driver");
            a.join().expect("worker A");
        });
    }

    /// Fix-B-WITH-closure (THE REAL FIX): `started`-gate + the B-closure re-read in
    /// the rejoin tail. ALL joins MUST return; the safety co-assertion (no sweep while
    /// A occupied∧published<cur — inside `driver_cycle`) and the lock-order assertion
    /// hold across EVERY interleaving. If THIS deadlocks or fails an assertion, the
    /// fix has a bug.
    #[test]
    fn loom_straddle_fix_b_with_closure_converges() {
        loom::model(|| {
            let s = Arc::new(St::new());
            let a = {
                let s = s.clone();
                thread::spawn(move || worker_a(&s, Variant::FixBWithClosure))
            };
            let d = {
                let s = s.clone();
                thread::spawn(move || driver(&s, Variant::FixBWithClosure))
            };
            // (R1 + no-lost-wakeup) both joins MUST return for EVERY schedule. If the
            // driver were stuck waiting on A's slot (R1) or A missed its resume, loom
            // would deadlock here.
            d.join().expect("driver — possible R1 (driver stuck on A's slot)");
            a.join().expect("worker A — possible relocated hang / lost wakeup");

            // Cycle 2 must have actually swept at cur_gen 2 (liveness: the fix does not
            // merely avoid the hang by skipping the cycle).
            assert_eq!(
                s.swept_cur_gen.load(Ordering::Acquire),
                2,
                "the fix must let cycle 2 actually sweep (cur_gen 2), not deadlock-avoid \
                 by never collecting"
            );
            // R1-CLOSED witness: A must have ended in a state that could not strand cycle
            // 2 — EITHER it republished its machine for cycle 2 (`published>=2`, the
            // B-closure path) OR it FINISHED and released its slot (`!occupied`, the
            // S2/LOST-NOTIFY-FIX path). BOTH are valid non-hang outcomes (which one a
            // given interleaving takes depends on whether cycle 2 had started while A was
            // still straddling). The forbidden terminal — the relocated hang — is
            // `occupied ∧ published<2`, which would have DEADLOCKED the driver's witness
            // wait above (so we'd never reach here); this is the structural cross-check.
            let occ = s.slot_occ.load(Ordering::Acquire);
            let pubd = s.slot_pub.load(Ordering::Acquire);
            assert!(
                pubd >= 2 || !occ,
                "R1: A must end republished-for-2 (published>=2) OR released (!occupied), \
                 never left occupied,published<2 (the relocated hang). Got occupied={}, \
                 published={}.",
                occ,
                pubd,
            );
        });
    }
}
