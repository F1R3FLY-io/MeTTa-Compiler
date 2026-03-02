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

use std::alloc::Layout;
use std::any::Any;
use std::cell::Cell;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::thread;
use std::time::Duration;

use parking_lot::{Condvar, Mutex, RwLock};
use portable_atomic::AtomicU128;

use super::metta_value::read_varint;
use super::metta_value::serialize_tags::*;
use super::metta_value::{MettaValue, MettaValueInner};
use super::metta_value_trait::MettaValueFactory;

// ============================================================================
// Fibonacci Hash for Pointer HashSets
// ============================================================================

/// Fast identity-like hasher for slab-allocated pointers.
/// Uses Fibonacci hashing to spread aligned pointers across hash table buckets.
/// SipHash (~50 cycles/hash) is overkill for pointer keys; this costs ~3 cycles.
pub(crate) struct PtrHasher(u64);

impl std::hash::Hasher for PtrHasher {
    #[inline(always)]
    fn finish(&self) -> u64 { self.0 }
    #[inline(always)]
    fn write(&mut self, _bytes: &[u8]) { unreachable!("PtrHasher only supports usize") }
    #[inline(always)]
    fn write_usize(&mut self, i: usize) {
        // Fibonacci hashing: multiply by golden ratio constant, then use
        // upper bits (which have maximal entropy after multiplication).
        self.0 = (i as u64).wrapping_mul(0x517cc1b727220a95);
    }
}

#[derive(Clone, Default)]
pub(crate) struct PtrBuildHasher;

impl std::hash::BuildHasher for PtrBuildHasher {
    type Hasher = PtrHasher;
    #[inline(always)]
    fn build_hasher(&self) -> PtrHasher { PtrHasher(0) }
}

/// HashSet for slab pointers using Fibonacci hashing instead of SipHash.
pub(crate) type PtrHashSet = std::collections::HashSet<*const u8, PtrBuildHasher>;

// ============================================================================
// GC Trace Flag (METTA_GC_TRACE environment variable)
// ============================================================================

/// Returns `true` if `METTA_GC_TRACE` env var is set. Cached after first check.
fn gc_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("METTA_GC_TRACE").is_ok())
}

/// Returns `true` if `METTA_GC_QUARANTINE` env var is set. Cached after first check.
/// When enabled, freed value slots go to a quarantine list (fully ASAN-poisoned)
/// instead of the Treiber free list, preventing slot reuse and enabling ASAN to
/// report full use-after-free with allocation/deallocation stacks.
fn gc_quarantine_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("METTA_GC_QUARANTINE").is_ok())
}

/// Return a human-readable discriminant name for a MettaValueInner variant.
/// Used only for GC trace logging — not on any hot path.
fn discriminant_name(inner: &MettaValueInner) -> &'static str {
    match inner {
        MettaValueInner::Atom(_) => "Atom",
        MettaValueInner::Bool(_) => "Bool",
        MettaValueInner::Long(_) => "Long",
        MettaValueInner::Float(_) => "Float",
        MettaValueInner::String(_) => "String",
        MettaValueInner::Unit => "Unit",
        MettaValueInner::SExpr(_) => "SExpr",
        MettaValueInner::Error(_, _) => "Error",
        MettaValueInner::Type(_) => "Type",
        MettaValueInner::Quoted(_) => "Quoted",
        MettaValueInner::Conjunction(_) => "Conjunction",
        MettaValueInner::Space(_) => "Space",
        MettaValueInner::State(_) => "State",
        MettaValueInner::Memo(_) => "Memo",
        MettaValueInner::Empty => "Empty",
        MettaValueInner::Spanned(_, _) => "Spanned",
    }
}

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

/// GC growth factor — next threshold = live_bytes * GROWTH_FACTOR.
const GC_GROWTH_FACTOR: f64 = 2.0;

/// Null sentinel for Treiber stack (no free slots).
const TREIBER_NULL: u128 = 0;

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
            "mmap failed for {} bytes", size
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
    /// GC mark bitmap: bit i = 1 means slot i is marked (reachable).
    /// Uses atomic u64 words for concurrent mark operations.
    marks: Vec<AtomicU64>,
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
}

impl ValuePage {
    /// Create a new value page for the given slot size.
    fn new(slot_size: usize) -> Self {
        let capacity = PAGE_SIZE / slot_size;
        let mark_words = (capacity + 63) / 64;
        let data = MmapPage::new(PAGE_SIZE);
        let marks: Vec<AtomicU64> = (0..mark_words).map(|_| AtomicU64::new(0)).collect();
        let epochs: Vec<AtomicU64> = (0..capacity).map(|_| AtomicU64::new(0)).collect();
        let context_ids: Vec<AtomicU32> = (0..capacity).map(|_| AtomicU32::new(0)).collect();
        let exec_counts: Vec<AtomicU32> = (0..capacity).map(|_| AtomicU32::new(0)).collect();
        Self {
            data,
            bump_count: AtomicUsize::new(0),
            capacity,
            live_count: AtomicIsize::new(0),
            marks,
            epochs,
            context_ids,
            exec_counts,
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

    /// Set the mark bit for a slot (atomic).
    #[inline]
    fn set_mark(&self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        self.marks[word].fetch_or(1u64 << bit, Ordering::Relaxed);
    }

    /// Check if a slot is marked (atomic).
    #[inline]
    fn is_marked(&self, idx: usize) -> bool {
        let word = idx / 64;
        let bit = idx % 64;
        (self.marks[word].load(Ordering::Relaxed) & (1u64 << bit)) != 0
    }

    /// Clear all mark bits (atomic).
    #[inline]
    fn clear_marks(&self) {
        for word in &self.marks {
            word.store(0, Ordering::Relaxed);
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

    /// Get slot context ID (atomic).
    #[inline]
    fn context_id(&self, idx: usize) -> u32 {
        self.context_ids[idx].load(Ordering::Acquire)
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

    /// Atomically swap the exec_count for a slot, returning the old value (Relaxed).
    #[inline]
    pub(crate) fn exec_count_swap(&self, idx: usize, val: u32) -> u32 {
        self.exec_counts[idx].swap(val, Ordering::Relaxed)
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
            unsafe { (*node).next.store(old_head, Ordering::Release); }
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

    /// Push a batch of freed slots as a linked chain with a single CAS.
    ///
    /// Builds the chain locally (zero contention), then atomically splices
    /// the entire chain onto the stack head. Reduces N individual CAS ops to 1.
    ///
    /// # Safety
    /// All pointers must be valid slab slots with at least `size_of::<FreeNode>()` bytes.
    fn push_batch(&self, ptrs: &[*mut u8]) {
        if ptrs.is_empty() {
            return;
        }

        // Single-element optimization: fall back to regular push
        if ptrs.len() == 1 {
            self.push(ptrs[0]);
            return;
        }

        // Build internal chain: ptrs[0] → ptrs[1] → ... → ptrs[N-2]
        // No atomics needed for internal links — this is thread-local data.
        for i in 0..ptrs.len() - 1 {
            debug_assert!(
                !ptrs[i].is_null() && (ptrs[i] as usize) % SLOT_ALIGN == 0,
                "TreiberStack::push_batch: invalid pointer {:?} at index {}",
                ptrs[i], i,
            );
            let node = ptrs[i] as *mut FreeNode;
            let next_packed = treiber_pack(ptrs[i + 1], 0);
            // Relaxed is fine: these internal links are invisible to other threads
            // until the CAS below publishes the chain head.
            unsafe {
                (*node).next.store(next_packed, Ordering::Relaxed);
            }
        }

        debug_assert!(
            !ptrs.last().unwrap().is_null()
                && (*ptrs.last().unwrap() as usize) % SLOT_ALIGN == 0,
            "TreiberStack::push_batch: invalid last pointer {:?}",
            ptrs.last().unwrap(),
        );

        let first = ptrs[0];
        let last = ptrs[ptrs.len() - 1] as *mut FreeNode;

        // CAS loop: splice chain onto head
        loop {
            let old_head = self.head.load(Ordering::Acquire);
            Self::validate_packed(old_head, "push_batch(old_head)");
            // Last node in chain points to current stack head
            unsafe {
                (*last).next.store(old_head, Ordering::Release);
            }
            let old_counter = treiber_unpack_counter(old_head);
            let new_head =
                treiber_pack(first, old_counter.wrapping_add(ptrs.len() as u64));
            Self::validate_packed(new_head, "push_batch(new_head)");
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

    /// Atomically drain the entire free list, returning the old packed head.
    ///
    /// After this call, the stack is empty. Concurrent `pop()` returns `None`
    /// (alloc falls through to bump alloc). Concurrent `push()` starts a new
    /// chain from `TREIBER_NULL`. The caller owns the returned chain exclusively
    /// and can walk it via `FreeNode::next` pointers.
    fn drain(&self) -> u128 {
        self.head.swap(TREIBER_NULL, Ordering::AcqRel)
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

/// Poison the ENTIRE slab slot, including the FreeNode header.
/// Used only by quarantine mode — slots in quarantine are never on the Treiber
/// stack, so the FreeNode header doesn't need to stay accessible.
#[inline(always)]
#[allow(unused_variables)]
unsafe fn asan_poison_slab_slot_full(ptr: *mut u8, slot_size: usize) {
    #[cfg(sanitize = "address")]
    {
        extern "C" {
            fn __asan_poison_memory_region(addr: *const std::ffi::c_void, size: usize);
        }
        __asan_poison_memory_region(ptr as *const std::ffi::c_void, slot_size);
    }
}

// ============================================================================
// GC Slot Quarantine (METTA_GC_QUARANTINE environment variable)
// ============================================================================
//
// When METTA_GC_QUARANTINE is enabled, freed value slots are diverted to a
// quarantine list instead of the Treiber free list. The entire slot (including
// the FreeNode header area) is ASAN-poisoned, so any stale MettaValue reference
// that reads the discriminant byte triggers an immediate ASAN heap-use-after-free
// report with full allocation/deallocation/use stacks.
//
// Without quarantine, asan_poison_slab_slot() leaves the first 16 bytes
// (FreeNode header) unpoisoned because the Treiber stack needs to read/write
// the FreeNode.next field. The MettaValueInner discriminant lives in the first
// byte — exactly within this unpoisoned region — so stale reads go undetected.

/// Metadata for a quarantined (freed but not yet reusable) value slot.
/// When METTA_GC_QUARANTINE is enabled, freed slots are added here instead
/// of the Treiber free list. The full slot is ASAN-poisoned, so any stale
/// reference triggers an ASAN heap-use-after-free report.
#[allow(dead_code)] // Fields are diagnostic metadata — read during ASAN analysis
struct QuarantineEntry {
    /// Pointer to the freed slot (in slab page).
    ptr: *mut u8,
    /// Slot size (for unpoisoning when eventually released to free list).
    slot_size: usize,
    /// Which page this slot belongs to (for diagnostic output).
    page_idx: usize,
    /// Slot index within the page (for diagnostic output).
    slot_idx: usize,
    /// The variant name at time of free (for diagnostic output).
    variant: &'static str,
    /// GC cycle number that freed this slot.
    gc_cycle: u64,
}

// SAFETY: QuarantineEntry holds raw pointer but is only accessed under Mutex.
unsafe impl Send for QuarantineEntry {}

/// Global quarantine list for freed value slots.
/// Protected by Mutex — only accessed during GC (not on hot alloc path).
static GC_QUARANTINE: OnceLock<Mutex<Vec<QuarantineEntry>>> = OnceLock::new();

fn gc_quarantine() -> &'static Mutex<Vec<QuarantineEntry>> {
    GC_QUARANTINE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Global GC cycle counter (incremented each time process_gc_response runs).
static GC_CYCLE_COUNT: AtomicU64 = AtomicU64::new(0);

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
                    unsafe { asan_unpoison_slab_slot(ptr, self.slot_size); }
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
        let ptr = page.bump_alloc(self.slot_size)
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
        unsafe { asan_poison_slab_slot(ptr, self.slot_size); }
        self.free_list.push(ptr);
    }

    /// Free a batch of slots with O(D log P) page lookups.
    /// Builds sorted page index once, amortizing across all pointers.
    fn free_batch(&self, ptrs: &[*mut u8]) {
        if ptrs.is_empty() { return; }
        let pages = self.pages.read();
        let index = DataPageIndex::new(&pages);
        for &ptr in ptrs {
            if let Some(page_idx) = index.find_page(&pages, ptr as *const u8, self.slot_size) {
                pages[page_idx].live_count.fetch_sub(1, Ordering::Relaxed);
            }
            // ASAN: poison the freed slot BEFORE push (skip FreeNode header used by Treiber stack).
            unsafe { asan_poison_slab_slot(ptr, self.slot_size); }
            self.free_list.push(ptr);
        }
    }

    /// Total bytes committed by this size class.
    fn committed_bytes(&self) -> usize {
        let pages = self.pages.read();
        pages.len() * PAGE_SIZE
    }

    /// Release empty pages via atomic drain + filter + rebuild of the free list.
    ///
    /// Same mechanism as `ValueAllocator::release_empty_pages()`:
    /// 1. Quick read-lock check for any empty pages (common case: no-op)
    /// 2. Write-lock to identify empty pages (excluding current_page)
    /// 3. Atomically drain the Treiber stack free list (single XCHG, O(1))
    /// 4. Walk the drained chain, push back survivors not in released pages
    /// 5. swap_remove empty pages (triggers munmap via MmapPage::Drop)
    ///
    /// Race safety: `alloc()` validates free-list pointers against the pages
    /// vector under a read-lock after `pop()`. Stale pointers to munmapped
    /// pages are discarded, falling through to bump allocation.
    fn release_empty_pages(&self) {
        // Phase 1: Quick check with read lock (common case: no empty pages)
        {
            let pages = self.pages.read();
            let has_empty = pages.iter().any(|page| {
                page.live_count.load(Ordering::Relaxed) <= 0
                    && page.bump_count.load(Ordering::Relaxed) > 0
            });
            if !has_empty {
                return;
            }
        }

        // Phase 2: Write lock — identify empty pages, excluding current_page
        let mut pages = self.pages.write();
        let current_page_ptr = self.current_page.load(Ordering::Acquire);

        // Collect address ranges of empty pages (for fast membership check during walk)
        let mut release_ranges: Vec<(usize, usize)> = Vec::new();
        for page in pages.iter() {
            let page_ptr = &**page as *const DataPage as *mut DataPage;
            if page.live_count.load(Ordering::Relaxed) <= 0
                && page.bump_count.load(Ordering::Relaxed) > 0
                && page_ptr != current_page_ptr
            {
                let start = page.data.as_ptr() as usize;
                release_ranges.push((start, start + PAGE_SIZE));
            }
        }

        if release_ranges.is_empty() {
            return;
        }

        // Sort for O(log R) binary search instead of O(R) linear scan
        release_ranges.sort_unstable_by_key(|&(start, _)| start);

        // Phase 3: Drain free list (atomic swap, O(1))
        let old_head = self.free_list.drain();

        // Phase 4: Walk drained chain, push back survivors
        let mut current = old_head;
        while current != TREIBER_NULL {
            let ptr = treiber_unpack_ptr(current);
            // Read next BEFORE any munmap — slot memory is still mapped here.
            // Relaxed is sufficient: chain is exclusively owned after drain().
            let next = unsafe { (*(ptr as *const FreeNode)).next.load(Ordering::Relaxed) };

            let addr = ptr as usize;
            let in_released = {
                let pos = release_ranges.partition_point(|&(start, _)| start <= addr);
                pos > 0 && addr < release_ranges[pos - 1].1
            };

            if !in_released {
                self.free_list.push(ptr);
            }

            current = next;
        }

        // Phase 5: Bump data cache generation BEFORE releasing pages.
        // All thread-local data caches will discard their pointers on next access.
        DATA_CACHE_GENERATION.fetch_add(1, Ordering::Release);

        // Phase 6: Remove empty pages (triggers munmap via MmapPage::Drop)
        // Iterate in reverse so swap_remove indices remain valid.
        let mut i = pages.len();
        while i > 0 {
            i -= 1;
            let page_start = pages[i].data.as_ptr() as usize;
            if release_ranges.binary_search_by_key(&page_start, |&(start, _)| start).is_ok() {
                pages.swap_remove(i);
            }
        }

        // Phase 7: Update current_page if all pages were released
        if pages.is_empty() {
            self.current_page.store(ptr::null_mut(), Ordering::Release);
        }
        // Otherwise current_page is still valid — we excluded it from release,
        // and Box<DataPage> heap address is stable across swap_remove.
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

    /// Drain all cached slot pointers (as raw `*mut u8`) for pushing back
    /// to the global TreiberStack. After this call, the cache is empty.
    #[inline]
    fn drain_ptrs(&mut self) -> impl Iterator<Item = *mut u8> + '_ {
        let len = self.len;
        self.len = 0;
        self.slots[..len].iter().map(|s| s.ptr)
    }

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

/// Flush the thread-local value cache back to the global free list.
/// Called on thread exit via `Drop` and on generation mismatch.
fn flush_value_cache_to_global(cache: &mut ThreadFreeCache, free_list: &TreiberStack) {
    for ptr in cache.drain_ptrs() {
        free_list.push(ptr);
    }
}

// ============================================================================
// Thread-Local Data Free-List Cache (DataClassAllocator Contention Reduction)
// ============================================================================

/// Maximum number of cached pointers per data size class.
const DATA_CACHE_CAPACITY: usize = 32;

/// Number of slots to batch-pop from global TreiberStack when the data cache is empty.
const DATA_CACHE_REFILL: usize = 16;

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
                // Pages were munmapped — discard all cached pointers and refill
                flush_value_cache_to_global(&mut cache, &self.free_list);
                cache.sync_generation();
            }

            if let Some(slot) = cache.pop() {
                cell.set(cache);
                // SAFETY: Generation check above guarantees the page hasn't been freed.
                // Pages are append-only within a generation — only release_empty_pages()
                // modifies the Vec, which bumps generation and invalidates all caches first.
                let page = unsafe { &*slot.page };
                let new_epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
                page.set_slot_epoch(slot.slot_idx as usize, new_epoch);
                page.set_context_id(slot.slot_idx as usize, ctx_id);
                page.live_count.fetch_add(1, Ordering::Relaxed);
                unsafe { asan_unpoison_slab_slot(slot.ptr, slot_size); }
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
                    unsafe { asan_unpoison_slab_slot(slot.ptr, slot_size); }
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
                    unsafe { asan_unpoison_slab_slot(ptr, slot_size); }
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
        let (ptr, idx) = page.bump_alloc(self.slot_size)
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
        unsafe { asan_poison_slab_slot(ptr, self.slot_size); }
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

    /// Release empty pages via atomic drain + filter + rebuild of the free list.
    ///
    /// Safe page release mechanism:
    /// 1. Quick read-lock check for any empty pages (common case: no-op)
    /// 2. Write-lock to identify empty pages (excluding current_page)
    /// 3. Atomically drain the Treiber stack free list (single XCHG, O(1))
    /// 4. Walk the drained chain, push back survivors not in released pages
    /// 5. swap_remove empty pages (triggers munmap via MmapPage::Drop)
    ///
    /// **Hot path impact: ZERO** — alloc/free are unchanged. During the brief
    /// drain window, `pop()` returns `None` and alloc falls through to bump
    /// alloc (correct, lock-free).
    ///
    /// Race safety: `alloc()` validates free-list pointers against the pages
    /// vector under a read-lock after `pop()`. Stale pointers to munmapped
    /// pages are discarded, falling through to bump allocation.
    fn release_empty_pages(&self) {
        // Phase 1: Quick check with read lock (common case: no empty pages)
        {
            let pages = self.pages.read();
            let has_empty = pages.iter().any(|page| {
                page.live_count.load(Ordering::Relaxed) <= 0
                    && page.bump_count.load(Ordering::Relaxed) > 0
            });
            if !has_empty {
                return;
            }
        }

        // Phase 2: Write lock — identify empty pages, excluding current_page
        let mut pages = self.pages.write();
        let current_page_ptr = self.current_page.load(Ordering::Acquire);

        // Collect address ranges of empty pages (for fast membership check during walk)
        let mut release_ranges: Vec<(usize, usize)> = Vec::new();
        for page in pages.iter() {
            let page_ptr = &**page as *const ValuePage as *mut ValuePage;
            if page.live_count.load(Ordering::Relaxed) <= 0
                && page.bump_count.load(Ordering::Relaxed) > 0
                && page_ptr != current_page_ptr
            {
                let start = page.data.as_ptr() as usize;
                release_ranges.push((start, start + PAGE_SIZE));
            }
        }

        if release_ranges.is_empty() {
            return;
        }

        // Sort for O(log R) binary search instead of O(R) linear scan
        release_ranges.sort_unstable_by_key(|&(start, _)| start);

        // Phase 3: Drain free list (atomic swap, O(1))
        let old_head = self.free_list.drain();

        // Phase 4: Walk drained chain, push back survivors
        let mut current = old_head;
        while current != TREIBER_NULL {
            let ptr = treiber_unpack_ptr(current);
            // Read next BEFORE any munmap — slot memory is still mapped here.
            // Relaxed is sufficient: chain is exclusively owned after drain().
            let next = unsafe { (*(ptr as *const FreeNode)).next.load(Ordering::Relaxed) };

            let addr = ptr as usize;
            let in_released = {
                let pos = release_ranges.partition_point(|&(start, _)| start <= addr);
                pos > 0 && addr < release_ranges[pos - 1].1
            };

            if !in_released {
                self.free_list.push(ptr);
            }

            current = next;
        }

        // Phase 5: Remove empty pages (triggers munmap via MmapPage::Drop)
        // Iterate in reverse so swap_remove indices remain valid.
        let mut i = pages.len();
        while i > 0 {
            i -= 1;
            let page_start = pages[i].data.as_ptr() as usize;
            if release_ranges.binary_search_by_key(&page_start, |&(start, _)| start).is_ok() {
                pages.swap_remove(i);
            }
        }

        // Phase 6: Invalidate thread-local caches.
        // Any cached pointers to munmapped pages are now stale. Bumping the
        // generation counter causes all threads to discard their caches on
        // the next allocation attempt.
        CACHE_GENERATION.fetch_add(1, Ordering::Release);

        // Phase 7: Update current_page if all pages were released
        if pages.is_empty() {
            self.current_page.store(ptr::null_mut(), Ordering::Release);
        }
        // Otherwise current_page is still valid — we excluded it from release,
        // and Box<ValuePage> heap address is stable across swap_remove.
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
            (*(dst as *const FreeNode)).next.store(first_chunk, Ordering::Release);
            // Non-atomically copy remaining bytes
            let remaining = len - header_size;
            if remaining > 0 {
                ptr::copy_nonoverlapping(
                    src.add(header_size),
                    dst.add(header_size),
                    remaining,
                );
            }
        } else if len > 0 {
            // Source is < 16 bytes: zero-pad to 16, store atomically
            let mut buf = [0u8; 16];
            ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len);
            let chunk: u128 = u128::from_ne_bytes(buf);
            (*(dst as *const FreeNode)).next.store(chunk, Ordering::Release);
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
            self.committed_bytes_atomic.store(
                self.committed_bytes(),
                Ordering::Relaxed,
            );
            // NOTE: We intentionally do NOT call request_gc() here. GC must only
            // be triggered at safe points (the trampoline's maybe_gc()) where the
            // root set is complete. The cron manager's threshold/rate checks handle
            // setting GC_REQUESTED; the trampoline picks it up at the next safe point.
        }

        unsafe {
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
        }
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
                                unsafe { asan_unpoison_slab_slot(ptr, class_size); }
                                return Some(ptr);
                            }
                        }
                        // Stale pointer (page munmapped between cache and validation)
                        // — discard and fall through
                        return None;
                    }

                    // Cache empty — batch-refill from global TreiberStack
                    let mut batch_len = 0usize;
                    let mut batch: [*mut u8; DATA_CACHE_REFILL] = [ptr::null_mut(); DATA_CACHE_REFILL];
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
                                    unsafe { asan_unpoison_slab_slot(ptr, class_size); }
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
        let layout = Layout::from_size_align(size, SLOT_ALIGN)
            .expect("invalid layout for large allocation");
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
                unsafe { asan_poison_slab_slot(ptr, class_size); }

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
            unsafe { std::alloc::dealloc(ptr, layout); }
        }
    }

    /// Free a batch of data slots with O(D log P) page lookups per size class.
    /// Groups dead data by size class, then calls `free_batch` per class.
    fn free_data_slots_batch(&self, dead_data: Vec<(*mut u8, usize)>) {
        if dead_data.is_empty() { return; }

        // Group by size class (9 classes + large)
        let mut by_class: [Vec<*mut u8>; 9] = Default::default();
        let mut large_ptrs: Vec<(*mut u8, usize)> = Vec::new();

        for (ptr, size) in dead_data {
            if size == 0 { continue; }
            match DATA_SIZE_CLASSES.iter().position(|&cs| size <= cs) {
                Some(i) => by_class[i].push(ptr),
                None => large_ptrs.push((ptr, size)),
            }
        }

        for (i, ptrs) in by_class.iter().enumerate() {
            if !ptrs.is_empty() {
                self.data_classes[i].free_batch(ptrs);
            }
        }

        if !large_ptrs.is_empty() {
            let mut large = self.large_allocs.lock();
            for (ptr, _) in large_ptrs {
                if let Some(pos) = large.iter().position(|(p, _)| *p == ptr) {
                    let (ptr, layout) = large.swap_remove(pos);
                    unsafe { std::alloc::dealloc(ptr, layout); }
                }
            }
        }
    }

    /// Total committed bytes (all pages).
    pub fn committed_bytes(&self) -> usize {
        let value_bytes = self.values.committed_bytes();
        let data_bytes: usize = self.data_classes.iter().map(|dc| dc.committed_bytes()).sum();
        let large_bytes: usize = self.large_allocs.lock()
            .iter().map(|(_, l)| l.size()).sum();
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
            unsafe { std::alloc::dealloc(ptr, layout); }
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

/// Get a factory backed by the global allocator.
pub fn global_factory() -> GcFactory {
    GcFactory::new(global_allocator())
}

// ============================================================================
// Global GC Thread + Coordination
// ============================================================================

// GLOBAL_GC_THREAD removed — replaced by AdaptiveGcPool (gc_pool.rs).
// GcThread is still available for direct use in tests.

/// Flag set by the cron manager or allocation pressure to request a GC cycle.
/// Checked by `maybe_gc()` in the trampoline loop (every 256 iterations).
/// `pub(crate)` for test observability (clearing between tests).
pub(crate) static GC_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request a GC cycle. Sets the `gc_requested` flag which will be picked up
/// by the next `maybe_gc()` call from the trampoline loop.
pub fn request_gc() {
    GC_REQUESTED.store(true, Ordering::Release);
}

/// Check whether a GC cycle has been requested (test observability).
pub fn is_gc_requested() -> bool {
    GC_REQUESTED.load(Ordering::Acquire)
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

    let pool = super::gc_pool::global_gc_pool();
    pool.submit_low(super::gc_pool::GcWorkItem::SessionRelease {
        context_ids: vec![context_id],
    });
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
/// from 1→0. Used by gc_pool workers to wait for safe root tracing points.
pub(super) static QUIESCENT_MUTEX: Mutex<()> = Mutex::new(());
pub(super) static QUIESCENT_CONDVAR: Condvar = Condvar::new();

/// Set when a GC snapshot is sent to the GC thread, cleared when the response
/// is processed. Prevents queueing multiple snapshots in the mpsc channel.
///
/// Aligns with TLA+ model's `~hasGcRequest /\ gcPhase = "idle"` preconditions
/// on `TryQuiescentGc_AcquireFlag`. Without this, `maybe_quiescent_gc()` could
/// send multiple snapshots while the GC thread is still processing a previous
/// one, wasting memory and CPU on redundant GC cycles.
static GC_CYCLE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Read-locked by GC mark/sweep workers (shared access to page data).
/// Write-locked by `release_empty_pages()` (exclusive, may munmap pages).
///
/// Prevents SIGSEGV from concurrent munmap during mark/sweep traversal:
/// a GC worker executing mark_snapshot dereferences `*const MettaValueInner`
/// pointers into slab pages. If another worker concurrently calls
/// `release_empty_pages()` → `MmapPage::Drop` → munmap, the first worker
/// faults on an unmapped address. The RwLock ensures release_empty_pages
/// waits for all in-flight mark/sweep operations to complete.
pub(crate) static PAGE_LIFECYCLE_LOCK: parking_lot::RwLock<()> =
    parking_lot::RwLock::new(());

// ============================================================================
// Session GC Statistics — counters for diagnosing memory growth
// ============================================================================

/// Total number of sessions released (incremented in release_session_with_surviving)
static SESSION_RELEASES_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Total values freed across all session releases
static SESSION_VALUES_FREED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Total values promoted (context_id → 0) across all session releases
static SESSION_VALUES_PROMOTED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Total values scanned during session releases
static SESSION_VALUES_SCANNED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Size of the last surviving set (root trace result)
static LAST_SURVIVING_SET_SIZE: AtomicU64 = AtomicU64::new(0);

/// Public accessor for session GC stats (used by diagnostics module).
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
        EVAL_GUARD_DEPTH.with(|d| d.set(d.get() + 1));
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

/// RAII guard that sets `GC_IN_PROGRESS = true` on creation and clears it on drop.
/// Ensures the flag is always cleared, even if the GC snapshot path panics.
pub(super) struct GcInProgressGuard;

impl GcInProgressGuard {
    #[allow(dead_code)]
    pub(super) fn enter() -> Self {
        GC_IN_PROGRESS.store(true, Ordering::Release);
        GcInProgressGuard
    }

    /// Try to enter GC-in-progress state. Returns None if another thread
    /// already holds the guard (e.g., maybe_quiescent_gc() or another
    /// session release cycle).
    pub(super) fn try_enter() -> Option<Self> {
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

/// Mutex + Condvar pair for Tier 2 backpressure blocking.
/// Replaces 1ms polling loop with instant wakeup when GC cycle completes.
/// Notified from `maybe_process_gc_response()` after clearing `GC_CYCLE_IN_FLIGHT`.
static GC_CYCLE_MUTEX: Mutex<()> = Mutex::new(());
static GC_CYCLE_CONDVAR: Condvar = Condvar::new();

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
    // Only apply backpressure when GC is actively running. If no GC cycle
    // is in flight, sleeping wastes time without benefit — there's nothing
    // to wait for. Cost: one atomic load (~1-2 ns), but avoids the
    // backpressure_level() load entirely when GC is idle (net win).
    if !gc_cycle_in_flight() {
        return;
    }
    match backpressure_level() {
        0 => {} // No backpressure — hot path, zero overhead
        1 => thread::yield_now(),
        2 => thread::sleep(Duration::from_micros(10)),
        _ => thread::sleep(Duration::from_micros(100)),
    }
}

/// Tier 2 backpressure: block at max level until GC cycle completes.
///
/// Called from main.rs between top-level expressions and from the eval()
/// return path. At MAX level, parks on `GC_CYCLE_CONDVAR` until
/// `gc_cycle_in_flight()` returns false (notified from
/// `maybe_process_gc_response()` after clearing `GC_CYCLE_IN_FLIGHT`).
///
/// The caller should also call `maybe_process_gc_response()` as a
/// standalone action before this function (matching TLA+ ProcessGcResponse
/// at "between" phase), so the common case of an already-available
/// response is handled without entering the loop.
///
/// At levels below MAX, this is a no-op.
///
/// Uses a condvar with 100ms timeout instead of polling (zero CPU while
/// waiting, instant wakeup on GC completion, timeout as safety net against
/// lost notifications).
#[inline]
pub fn apply_backpressure_tier2() {
    if backpressure_level() >= MAX_BACKPRESSURE && gc_cycle_in_flight() {
        let mut lock = GC_CYCLE_MUTEX.lock();
        // Re-check under lock (double-checked locking pattern)
        while backpressure_level() >= MAX_BACKPRESSURE && gc_cycle_in_flight() {
            // Timeout prevents infinite wait if GC response notification is lost.
            // 100ms matches cron monitor poll interval — at worst we retry at
            // the same cadence as before.
            GC_CYCLE_CONDVAR.wait_for(&mut lock, Duration::from_millis(100));
        }
    }
}

/// Trigger GC at a quiescent point (no active evaluators).
///
/// Called from eval loops between top-level expressions. Only triggers if:
/// 1. GC is not disabled (`--no-gc`)
/// 2. `GC_REQUESTED` is set (by cron monitor or manual request)
/// 3. No GC cycle is already in flight (`GC_CYCLE_IN_FLIGHT == false`)
/// 4. No evaluators are active (`ACTIVE_EVALUATORS == 0`)
///
/// Uses `GC_IN_PROGRESS` flag to prevent new evals from starting during
/// the brief snapshot capture (sub-millisecond). The actual mark-sweep
/// runs asynchronously on the GC thread.
///
/// The `GC_CYCLE_IN_FLIGHT` check (step 3) ensures at most one GC cycle
/// is in flight at a time, aligning with TLA+ `~hasGcRequest /\ ~hasGcResponse
/// /\ gcPhase = "idle"` preconditions on `TryQuiescentGc_AcquireFlag`.
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

    // Safe: no evaluators active, build snapshot and submit to GC pool.
    let result = trigger_gc_cycle_via_pool();

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
    // Signal that the GC lifecycle is reachable (for cron backpressure gating)
    bump_gc_reachable();

    // Lazily spawn the GC cron manager (idempotent via OnceLock)
    let _ = global_gc_cron();

    // Acquire GC_IN_PROGRESS for mutual exclusion with
    // release_session_with_surviving() on GC pool worker threads.
    // Both paths free/poison value slots — concurrent execution is a
    // TOCTOU race on slot epoch/content (FlyingRaven ASAN finding).
    //
    // try_enter() is non-blocking: if session release holds the guard,
    // we skip — the response stays on the channel for the next call.
    // Must acquire BEFORE try_recv_response() so we don't consume a
    // response we can't safely process.
    let _gc_guard = match GcInProgressGuard::try_enter() {
        Some(guard) => guard,
        None => return false,
    };

    let pool = super::gc_pool::global_gc_pool();
    let alloc = global_allocator();

    match pool.try_recv_response() {
        Some(response) => {
            alloc.process_gc_response(&response);
            // Page release is handled inside process_gc_response() (Phase 5).

            // Adaptive threshold: next_threshold = max(live_bytes * GROWTH_FACTOR, MIN_GC_THRESHOLD)
            let new_threshold = (response.live_bytes as f64 * GC_GROWTH_FACTOR) as usize;
            alloc.set_gc_threshold(new_threshold.max(MIN_GC_THRESHOLD));

            // Clear in-flight flag — aligns with TLA+ `hasGcResponse' = FALSE`
            // in ProcessGcResponse. A new GC cycle can now be triggered.
            GC_CYCLE_IN_FLIGHT.store(false, Ordering::Release);

            // Wake any thread blocked in apply_backpressure_tier2().
            // Must notify AFTER clearing GC_CYCLE_IN_FLIGHT so the woken thread
            // sees the updated flag when it re-checks the while condition.
            GC_CYCLE_CONDVAR.notify_all();

            // Immediate backpressure feedback — don't wait for next cron poll (100ms).
            // Re-evaluate backpressure level based on current committed/threshold ratio.
            // Models TLA+ ProcessGcResponse: backpressureLevel' = max(bp - 1, 0).
            let committed = alloc.committed_bytes_atomic().load(Ordering::Relaxed);
            let threshold = alloc.gc_threshold_atomic().load(Ordering::Relaxed);
            let new_level = if threshold > 0 {
                if committed >= threshold * 2 { 3 }
                else if committed >= threshold * 3 / 2 { 2 }
                else if committed >= threshold { 1 }
                else { 0 }
            } else { 0 };
            set_backpressure_level(new_level);

            return true;
        }
        None => {}
    }
    false
}

// ============================================================================
// GC Root Registry — Global Root Collection for Concurrent GC
// ============================================================================

/// Trait for objects that hold GC-managed values and can provide their live roots.
///
/// Implementors register themselves with the global root registry on creation
/// and unregister on drop. The GC thread calls `collect_all_roots()` to gather
/// roots from all registered providers across all threads.
///
/// # Thread Safety
///
/// `collect_roots` may be called from any thread (typically the GC thread).
/// Implementations must be thread-safe.
pub trait RootProvider: Send + Sync {
    /// Collect all live GC root values from this provider.
    ///
    /// Implementations should push all `MettaValue` values that are
    /// currently reachable from this provider into the `roots` vector.
    fn collect_roots(&self, roots: &mut Vec<MettaValue>);
}

/// Global registry of active root providers using weak references.
///
/// Uses `Weak<dyn RootProvider>` so that environments are automatically cleaned
/// up when they go out of scope — no explicit unregistration needed. Dead entries
/// are pruned during `collect_all_roots()`.
///
/// Uses `RwLock` because registration is infrequent (environment creation), while
/// `collect_all_roots()` only runs during GC cycles (not on the allocation hot path).
static ROOT_REGISTRY: OnceLock<RwLock<Vec<Weak<dyn RootProvider>>>> = OnceLock::new();

fn root_registry() -> &'static RwLock<Vec<Weak<dyn RootProvider>>> {
    ROOT_REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register a root provider with the global GC root registry.
///
/// Stores a `Weak` reference — the provider is automatically removed from the
/// registry when all strong `Arc` references are dropped.
pub fn register_root_provider(provider: &Arc<dyn RootProvider>) {
    let mut registry = root_registry().write();
    registry.push(Arc::downgrade(provider));
}

/// Collect roots from all registered providers.
///
/// Called by the GC integration to build a complete root set before constructing
/// a `GcSnapshot`. This gathers live values from all active environments, VMs,
/// and other root sources across all threads.
///
/// Dead (dropped) providers are automatically pruned during collection.
///
/// # Lock Protocol
///
/// Splits into two phases to minimize `ROOT_REGISTRY` hold time and prevent
/// writer starvation (which previously deadlocked when many test threads
/// tried to register new environments concurrently with GC root collection):
///
/// 1. **Snapshot phase** — Briefly acquires `ROOT_REGISTRY.write()` to upgrade
///    all `Weak` refs to `Arc`, prune dead entries, and release the lock.
/// 2. **Collection phase** — Iterates the local `Vec<Arc>` without holding any
///    registry lock, calling `collect_roots()` on each provider.
pub fn collect_all_roots() -> Vec<MettaValue> {
    // Phase 1: Snapshot — briefly hold write lock to upgrade Weak refs and prune dead entries.
    // This takes O(N * weak_upgrade) time, NOT O(N * collect_roots) time.
    let providers: Vec<Arc<dyn RootProvider>> = {
        let mut registry = root_registry().write();
        let mut live = Vec::with_capacity(registry.len());
        registry.retain(|weak| {
            if let Some(strong) = weak.upgrade() {
                live.push(strong);
                true
            } else {
                false // Provider was dropped — remove from registry
            }
        });
        live
        // ROOT_REGISTRY write lock released here
    };

    // Phase 2: Collect roots from each provider WITHOUT holding ROOT_REGISTRY.
    // New environments can register freely during this phase.
    let mut roots = Vec::with_capacity(providers.len() * 64); // heuristic pre-alloc
    for provider in &providers {
        provider.collect_roots(&mut roots);
    }

    // Also collect safepoint roots from trampoline state
    collect_safepoint_roots(&mut roots);
    roots
}

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
// The registry is a global Mutex<Vec<Vec<MettaValue>>>. Each safepoint pushes
// a root set and receives a SafepointRootHandle that clears it on drop.
// collect_all_roots() drains these into the root set alongside environment roots.

/// Global registry for safepoint temporary roots.
///
/// Each entry is a Vec<MettaValue> collected from one evaluator's trampoline
/// state (work_stack + continuations). Entries are cleared (not removed) on
/// drop to preserve indices for concurrent handles.
static SAFEPOINT_ROOTS: OnceLock<Mutex<Vec<Vec<MettaValue>>>> = OnceLock::new();

fn safepoint_registry() -> &'static Mutex<Vec<Vec<MettaValue>>> {
    SAFEPOINT_ROOTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// RAII handle that unregisters safepoint roots when dropped.
///
/// Created by `register_temporary_roots()`. On drop, clears the root set
/// at the stored index (the Vec slot is zeroed, not removed, to avoid
/// invalidating other handles' indices).
pub struct SafepointRootHandle {
    /// Index into SAFEPOINT_ROOTS where this handle's roots are stored
    idx: usize,
}

impl Drop for SafepointRootHandle {
    fn drop(&mut self) {
        let registry = safepoint_registry();
        let mut guard = registry.lock();
        if self.idx < guard.len() {
            guard[self.idx].clear();
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
    // Reuse an empty slot if available (from a previous handle that was dropped)
    for (idx, slot) in guard.iter_mut().enumerate() {
        if slot.is_empty() {
            *slot = roots;
            return SafepointRootHandle { idx };
        }
    }
    // No empty slot — append
    let idx = guard.len();
    guard.push(roots);
    SafepointRootHandle { idx }
}

/// Collect safepoint roots into the provided Vec.
///
/// Called from `collect_all_roots()` to include trampoline-resident values
/// in the GC root set alongside environment roots.
fn collect_safepoint_roots(roots: &mut Vec<MettaValue>) {
    if let Some(registry) = SAFEPOINT_ROOTS.get() {
        let guard = registry.lock();
        for root_set in guard.iter() {
            roots.extend(root_set.iter().copied());
        }
    }
}

/// Build the transitive closure of all currently registered safepoint roots.
///
/// Returns `Some(HashSet)` if safepoint roots exist (i.e., at least one
/// evaluator is paused at a safepoint with registered temporary roots).
/// Returns `None` if no safepoint roots are registered (fast path — no
/// filtering needed).
///
/// This is used by `process_gc_response()` to guard against freeing values
/// that are dead per a **previous** GC cycle's mark-sweep but are now live
/// in a concurrent evaluator's trampoline state. The previous cycle's roots
/// (R_prev) may differ from the current safepoint roots (R_current), so a
/// value can be:
/// - Dead per R_prev (not reachable from the snapshot's root set)
/// - Live per R_current (reachable from the current trampoline state)
///
/// Without this check, `process_gc_response()` frees such values during the
/// safepoint pause, and the trampoline crashes with use-after-poison when it
/// resumes and accesses them (FlyingRaven ASAN finding: `MettaValue::is_error`
/// in `process_collected_sexpr_generic`).
///
/// The DFS traversal matches `trace_surviving_set()` but operates only on
/// safepoint roots (not all environment roots), keeping cost proportional
/// to the trampoline's working set rather than the entire live heap.
fn trace_safepoint_live_set() -> Option<PtrHashSet> {
    let registry_ref = SAFEPOINT_ROOTS.get()?;
    let safepoint_roots: Vec<MettaValue> = {
        let guard = registry_ref.lock();
        let total: usize = guard.iter().map(|s| s.len()).sum();
        if total == 0 {
            return None;
        }
        let mut roots = Vec::with_capacity(total);
        for root_set in guard.iter() {
            roots.extend(root_set.iter().copied());
        }
        roots
    };

    let mut live_set = PtrHashSet::with_capacity_and_hasher(safepoint_roots.len() * 2, PtrBuildHasher);
    let mut worklist: Vec<*const MettaValueInner> = Vec::with_capacity(safepoint_roots.len());

    // Seed worklist with safepoint root inner pointers
    for root in &safepoint_roots {
        let ptr = root.inner_ptr();
        if live_set.insert(ptr as *const u8) {
            worklist.push(ptr);
        }
    }

    // DFS traversal — same variant matching as trace_surviving_set()
    while let Some(ptr) = worklist.pop() {
        // SAFETY: ptr points to a live MettaValueInner in a slab page.
        // Safepoint roots were registered BEFORE dropping EvalGuard, so
        // these slots are guaranteed to not have been freed yet.
        match unsafe { &*ptr } {
            MettaValueInner::SExpr(children) => {
                for child in children.iter() {
                    let child_ptr = child.inner_ptr();
                    if live_set.insert(child_ptr as *const u8) {
                        worklist.push(child_ptr);
                    }
                }
            }
            MettaValueInner::Conjunction(goals) => {
                for goal in goals.iter() {
                    let goal_ptr = goal.inner_ptr();
                    if live_set.insert(goal_ptr as *const u8) {
                        worklist.push(goal_ptr);
                    }
                }
            }
            MettaValueInner::Error(_, details) => {
                let details_ptr = details.inner_ptr();
                if live_set.insert(details_ptr as *const u8) {
                    worklist.push(details_ptr);
                }
            }
            MettaValueInner::Type(inner) | MettaValueInner::Quoted(inner) => {
                let inner_ptr = inner.inner_ptr();
                if live_set.insert(inner_ptr as *const u8) {
                    worklist.push(inner_ptr);
                }
            }
            MettaValueInner::Space(handle) => {
                let mut space_values = Vec::new();
                handle.collect_gc_values(&mut space_values);
                for val in &space_values {
                    let val_ptr = val.inner_ptr();
                    if live_set.insert(val_ptr as *const u8) {
                        worklist.push(val_ptr);
                    }
                }
            }
            MettaValueInner::Spanned(v, _) => {
                let inner_ptr = v.inner_ptr();
                if live_set.insert(inner_ptr as *const u8) {
                    worklist.push(inner_ptr);
                }
            }
            MettaValueInner::Atom(_)
            | MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_)
            | MettaValueInner::Unit
            | MettaValueInner::Empty
            | MettaValueInner::State(_)
            | MettaValueInner::Memo(_) => {}
        }
    }

    if gc_trace_enabled() {
        eprintln!(
            "[GC-SAFEPOINT-LIVE] traced {} safepoint root values → {} transitive live pointers",
            safepoint_roots.len(),
            live_set.len(),
        );
    }

    Some(live_set)
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
        assert!(depth > 0, "drop_eval_guard_for_safepoint called without active guard");
        d.set(depth - 1);
    });

    let prev = ACTIVE_EVALUATORS.fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        // Transitioned to quiescent state — notify waiters
        let _lock = QUIESCENT_MUTEX.lock();
        QUIESCENT_CONDVAR.notify_all();
    }
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

    EVAL_GUARD_DEPTH.with(|d| d.set(d.get() + 1));
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

pub fn safepoint_wait_for_quiescence() {
    // Phase 1: Process any pending response from a previous GC cycle.
    // This clears GC_CYCLE_IN_FLIGHT, enabling a new trigger below.
    // Use fast version: skips expensive CAS+channel poll when GC is idle.
    maybe_process_gc_response_fast();

    // Phase 2: Trigger a new GC cycle (if quiescent and requested)
    maybe_quiescent_gc();

    // Phase 3: If a GC cycle was just triggered, wait briefly for the
    // GC thread to complete mark+sweep and send a response.
    if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
        // Try to process response immediately (mark+sweep is typically < 1ms
        // for bounded live sets like PLN's task/belief queues)
        if !maybe_process_gc_response() {
            // Park on condvar — woken by maybe_process_gc_response() calling
            // GC_CYCLE_CONDVAR.notify_all() when response arrives.
            let mut lock = GC_CYCLE_MUTEX.lock();
            if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
                let _result = GC_CYCLE_CONDVAR.wait_for(&mut lock, Duration::from_millis(50));
            }
            drop(lock);
            maybe_process_gc_response();
        }
    }

    // If GC is still requested (quiescence not yet reached because other
    // evaluators are active), park on condvar with timeout
    if GC_REQUESTED.load(Ordering::Acquire)
        && ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0
    {
        let mut lock = QUIESCENT_MUTEX.lock();
        // Wait up to 10ms for other evaluators to reach safepoints.
        // QUIESCENT_CONDVAR is notified by EvalGuard::drop() and
        // drop_eval_guard_for_safepoint() when transitioning to 0.
        let _result = QUIESCENT_CONDVAR.wait_for(&mut lock, Duration::from_millis(10));
        drop(lock);

        // Early exit: if GC_REQUESTED was cleared while we waited,
        // another thread handled it — no need to retry.
        if !GC_REQUESTED.load(Ordering::Acquire) {
            return;
        }

        // Retry after wakeup (either quiescent or timeout)
        maybe_process_gc_response_fast();
        maybe_quiescent_gc();

        // Wait for the new GC response via condvar parking
        if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
            if !maybe_process_gc_response() {
                let mut lock = GC_CYCLE_MUTEX.lock();
                if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
                    let _result = GC_CYCLE_CONDVAR.wait_for(&mut lock, Duration::from_millis(10));
                }
                drop(lock);
                maybe_process_gc_response();
            }
        }
    }
}

/// Register an environment's shared state as a GC root provider.
///
/// Uses `Any` downcasting to conditionally register only when `V = MettaValue`.
/// For other value types, this is a no-op.
///
/// This is called from `GenericEnvironment::new()`, `make_owned()`,
/// `fork_for_nondeterminism()`, `union()`, and `union_all()`.
pub fn try_register_env_roots<V>(shared: &Arc<crate::backend::environment::GenericEnvironmentShared<V>>)
where
    V: crate::backend::models::metta_value_trait::MettaValueTrait
        + Clone + Send + Sync + Unpin + 'static,
{
    // Skip registration when GC is disabled.
    if is_gc_disabled() {
        return;
    }
    // NOTE: Environments must always be registered regardless of GC pool state.
    // The GC pool's session release workers call collect_all_roots() →
    // trace_surviving_set(). If environments are not registered, the surviving
    // set is empty and ALL session-allocated values (including RuleEntry.lhs/rhs)
    // are freed, causing use-after-free when match_rules_native() dereferences
    // freed slab slots.

    // Clone the Arc and try to downcast to the concrete MettaValue type
    let any: Arc<dyn Any + Send + Sync> = shared.clone();
    if let Ok(arena_shared) = any.downcast::<crate::backend::environment::GenericEnvironmentShared<MettaValue>>() {
        // GenericEnvironmentShared<MettaValue> implements RootProvider
        let provider: Arc<dyn RootProvider> = arena_shared;
        register_root_provider(&provider);
    }
}

/// Trigger a GC cycle using the global allocator, root registry, and GC pool.
///
/// This is the primary GC entry point. It:
/// 1. Processes any pending GC response from a previous cycle
/// 2. Collects roots from all registered providers
/// 3. Builds a snapshot from the global allocator
/// 4. Submits the snapshot to the adaptive GC pool (HIGH priority)
///
/// Returns `true` if a GC cycle was initiated.
pub fn trigger_gc_cycle() -> bool {
    let pool = super::gc_pool::global_gc_pool();
    let alloc = global_allocator();

    // Process pending response only if we can acquire mutual exclusion
    // with release_session_with_surviving() on GC pool worker threads.
    // Both paths free/poison value slots — concurrent execution is a
    // TOCTOU race (FlyingRaven ASAN finding).
    //
    // Check GC_IN_PROGRESS before try_recv_response() so we don't
    // consume a response we can't safely process.
    if !GC_IN_PROGRESS.load(Ordering::Acquire) {
        if let Some(response) = pool.try_recv_response() {
            if let Some(_gc_guard) = GcInProgressGuard::try_enter() {
                alloc.process_gc_response(&response);
                // Page release is handled inside process_gc_response() (Phase 5).
                GC_CYCLE_IN_FLIGHT.store(false, Ordering::Release);
                // _gc_guard drops here → clears GC_IN_PROGRESS
            }
            // If guard fails after consuming: response is lost, but dead slots
            // will be re-discovered in the next GC cycle (conservative, safe).
        }
    }

    // Don't queue another cycle if one is already in flight
    if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
        return false;
    }

    // Collect roots from all registered providers
    let roots = collect_all_roots();

    // Build snapshot and submit to GC pool (HIGH priority channel)
    let snapshot = alloc.build_snapshot(roots);
    GC_CYCLE_IN_FLIGHT.store(true, Ordering::Release);
    pool.submit_high(super::gc_pool::GcWorkItem::Collect(snapshot));
    true
}

/// Trigger a GC cycle via the pool without processing pending responses.
///
/// Used by `maybe_quiescent_gc()` where responses are processed separately
/// by `maybe_process_gc_response()`.
///
/// Sets `GC_CYCLE_IN_FLIGHT` before submitting the snapshot to prevent
/// queueing multiple snapshots. Aligns with TLA+ `hasGcRequest' = TRUE`
/// in `TryQuiescentGc_SnapshotOK`.
fn trigger_gc_cycle_via_pool() -> bool {
    let pool = super::gc_pool::global_gc_pool();
    let alloc = global_allocator();
    let roots = collect_all_roots();
    let snapshot = alloc.build_snapshot(roots);
    GC_CYCLE_IN_FLIGHT.store(true, Ordering::Release);
    pool.submit_high(super::gc_pool::GcWorkItem::Collect(snapshot));
    true
}

// ============================================================================
// GC Mark-Sweep Support
// ============================================================================

/// Statistics from a GC sweep pass.
#[derive(Debug, Clone, Default)]
pub struct SweepStats {
    pub freed_values: usize,
    pub freed_bytes: usize,
    pub live_values: usize,
    pub live_bytes: usize,
}

/// Captures the allocation watermark at snapshot time.
#[derive(Debug, Clone, Copy)]
pub struct AllocationWatermark {
    pub value_page_count: usize,
    pub last_page_bump_count: usize,
}

/// Dead set produced by the GC sweep phase.
#[derive(Debug, Default)]
pub struct DeadSet {
    pub dead_values: Vec<*mut u8>,
    pub dead_data: Vec<(*mut u8, usize)>,
    pub live_bytes: usize,
    pub live_values: usize,
}

unsafe impl Send for DeadSet {}

// ============================================================================
// GcSnapshot — Immutable Snapshot for GC Thread
// ============================================================================

/// Frozen snapshot of a single value page.
pub struct PageSnapshot {
    pub data_ptr: *const u8,
    pub bump_count: usize,
    pub capacity: usize,
    /// Snapshot of per-slot epochs at snapshot time.
    /// Slots with epoch == u64::MAX are freed by a prior GC cycle and must
    /// be skipped during sweep (their content is FreeNode data, not valid
    /// MettaValueInner).
    pub epochs: Vec<u64>,
}

/// Immutable snapshot sent to the GC thread for mark-sweep collection.
pub struct GcSnapshot {
    pub page_snapshots: Vec<PageSnapshot>,
    pub slot_size: usize,
    pub free_set: PtrHashSet,
    pub snapshot_epoch: u64,
    pub roots: Vec<MettaValue>,
    pub marks: Vec<Vec<u64>>,
    pub total_committed_bytes: usize,
}

unsafe impl Send for GcSnapshot {}

/// Response from the GC thread.
#[derive(Debug)]
pub struct GcResponse {
    pub dead_values: Vec<*mut u8>,
    pub snapshot_epoch: u64,
    pub live_bytes: usize,
    pub live_values: usize,
}

unsafe impl Send for GcResponse {}

/// Sorted page index for O(log P) pointer-to-page lookup.
/// Pages are non-overlapping mmap regions; sorting by start address
/// enables binary search via `partition_point`.
struct PageIndex {
    /// (page_data_start_addr, page_vec_index), sorted by start addr.
    sorted: Vec<(usize, usize)>,
}

impl PageIndex {
    /// Build sorted index from pages. O(P log P), done once per GC response.
    fn new(pages: &[Box<ValuePage>]) -> Self {
        let mut sorted: Vec<(usize, usize)> = pages.iter().enumerate()
            .map(|(i, page)| (page.data.as_ptr() as usize, i))
            .collect();
        sorted.sort_unstable_by_key(|&(start, _)| start);
        Self { sorted }
    }

    /// Find (page_vec_index, slot_index) for a pointer. O(log P).
    #[inline]
    fn find(&self, pages: &[Box<ValuePage>], ptr: *const u8, slot_size: usize)
        -> Option<(usize, usize)>
    {
        let addr = ptr as usize;
        // partition_point returns the first index where start > addr,
        // so pos - 1 is the last page whose start <= addr.
        let pos = self.sorted.partition_point(|&(start, _)| start <= addr);
        if pos == 0 { return None; }
        let (_, page_idx) = self.sorted[pos - 1];
        pages[page_idx].slot_index(ptr, slot_size).map(|slot_idx| (page_idx, slot_idx))
    }
}

/// Sorted data page index for O(log P) pointer-to-page lookup.
/// Same approach as `PageIndex` but for `DataPage` (no per-slot epochs).
struct DataPageIndex {
    /// (page_data_start_addr, page_vec_index), sorted by start addr.
    sorted: Vec<(usize, usize)>,
}

impl DataPageIndex {
    /// Build sorted index from data pages. O(P log P), done once per batch.
    fn new(pages: &[Box<DataPage>]) -> Self {
        let mut sorted: Vec<(usize, usize)> = pages.iter().enumerate()
            .map(|(i, page)| (page.data.as_ptr() as usize, i))
            .collect();
        sorted.sort_unstable_by_key(|&(start, _)| start);
        Self { sorted }
    }

    /// Find the page index containing the given pointer. O(log P).
    #[inline]
    fn find_page(&self, pages: &[Box<DataPage>], ptr: *const u8, slot_size: usize)
        -> Option<usize>
    {
        let addr = ptr as usize;
        // partition_point returns the first index where start > addr,
        // so pos - 1 is the last page whose start <= addr.
        let pos = self.sorted.partition_point(|&(start, _)| start <= addr);
        if pos == 0 { return None; }
        let (_, page_idx) = self.sorted[pos - 1];
        if pages[page_idx].contains(ptr, slot_size) {
            Some(page_idx)
        } else {
            None
        }
    }
}

/// Dead value with pre-resolved page/slot location from Phase 1.
/// Caches the binary search result so Phase 3 needs zero page lookups.
struct ResolvedDead {
    ptr: *mut u8,
    page_idx: usize,
    slot_idx: usize,
}

impl SlabAllocator {
    /// Build a snapshot for the GC thread.
    pub fn build_snapshot(&self, roots: Vec<MettaValue>) -> GcSnapshot {
        let pages = self.values.pages.read();
        let slot_size = self.values.slot_size;

        let page_snapshots: Vec<PageSnapshot> = pages.iter().map(|page| {
            let bump_count = page.bump_count.load(Ordering::Acquire);
            // Snapshot per-slot epochs so the sweep can skip freed slots
            // (epoch == u64::MAX sentinel). Only capture up to bump_count
            // since slots beyond that haven't been allocated.
            let epochs: Vec<u64> = (0..bump_count)
                .map(|i| page.epochs[i].load(Ordering::Acquire))
                .collect();
            PageSnapshot {
                data_ptr: page.data.as_ptr(),
                bump_count,
                capacity: page.capacity,
                epochs,
            }
        }).collect();

        // free_set is no longer needed — the sweep now uses epoch snapshots
        // to skip freed slots (epoch == u64::MAX) instead of relying on a
        // drain of the Treiber stack.
        let free_set = PtrHashSet::with_hasher(PtrBuildHasher);

        let marks: Vec<Vec<u64>> = page_snapshots.iter().map(|ps| {
            let mark_words = (ps.capacity + 63) / 64;
            vec![0u64; mark_words]
        }).collect();

        GcSnapshot {
            page_snapshots,
            slot_size,
            free_set,
            snapshot_epoch: self.values.epoch.load(Ordering::Acquire),
            roots,
            marks,
            total_committed_bytes: self.committed_bytes(),
        }
    }

    /// Process a GC response with epoch-based TOCTOU filtering.
    ///
    /// Three-phase structure to avoid reading from freed slots:
    /// 1. Epoch filter: identify non-filtered dead values (binary search, O(D log P))
    /// 2. Collect dead data from non-filtered values (slot content still valid)
    /// 3. Free value slots (push to free list + ASAN poison, O(D') with cached indices)
    /// 4. Free data slots
    /// 5. Release empty pages
    ///
    /// Phases 1-3 share a single read lock on the page array.
    pub fn process_gc_response(&self, response: &GcResponse) {
        let mut non_filtered_dead: Vec<ResolvedDead> = Vec::with_capacity(response.dead_values.len());

        // Declare outside the block so it outlives the read lock.
        // It's an owned Vec<(*mut u8, usize)> — no borrows on pages.
        let dead_data_to_free: Vec<(*mut u8, usize)>;

        // === Phase 0: Build safepoint live set (if any safepoint roots exist) ===
        //
        // When an evaluator is paused at a GC safepoint, its trampoline state
        // (work_stack + continuations) holds live values that were registered as
        // temporary roots. These roots may differ from the roots used by the
        // PREVIOUS GC cycle's mark-sweep (which produced `response.dead_values`).
        //
        // A value can be:
        // - Dead per the previous cycle's roots (in `response.dead_values`)
        // - Live per the current safepoint roots (reachable from trampoline state)
        //
        // Without this filter, we'd free such values and the trampoline would
        // crash with use-after-poison when it resumes (FlyingRaven ASAN finding:
        // `MettaValue::is_error` in `process_collected_sexpr_generic`, thread T0).
        //
        // The epoch filter (Phase 1) catches values allocated AFTER the snapshot,
        // but not values that became reachable through DIFFERENT root sets between
        // GC cycles. This safepoint filter is the safety net for that case.
        let safepoint_live = trace_safepoint_live_set();

        // Single read lock across Phases 1-3. Safe because:
        // - No pages added (alloc_new_page takes write lock)
        // - No pages removed (release_empty_pages is Phase 5, after this block)
        // - collect_dead_data (Phase 2) only reads slot content in mmap pages
        // - free_list.push (Phase 3) is lock-free Treiber stack, doesn't touch pages
        {
            let pages = self.values.pages.read();
            let page_index = PageIndex::new(&pages);

            // === Phase 1: Epoch + safepoint filtering — O(D log P) ===
            // Separate dead values into filtered (re-allocated after snapshot OR
            // currently reachable from safepoint roots) and non-filtered (genuinely
            // dead, safe to reclaim).
            let mut safepoint_rescued = 0u64;
            for &ptr in &response.dead_values {
                if let Some((page_idx, slot_idx)) = page_index.find(
                    &pages, ptr as *const u8, self.values.slot_size,
                ) {
                    if pages[page_idx].slot_epoch(slot_idx) > response.snapshot_epoch {
                        // Slot re-allocated after snapshot — skip (not genuinely dead)
                    } else if safepoint_live.as_ref()
                        .is_some_and(|live| live.contains(&(ptr as *const u8)))
                    {
                        // Value is dead per previous cycle but live in current
                        // safepoint roots — skip to prevent use-after-poison.
                        safepoint_rescued += 1;
                    } else {
                        non_filtered_dead.push(ResolvedDead { ptr, page_idx, slot_idx });
                    }
                }
                // else: ptr not in any page (released in prior cycle) — skip
            }

            if safepoint_rescued > 0 && gc_trace_enabled() {
                eprintln!(
                    "[GC-SAFEPOINT-RESCUE] rescued {} values from previous cycle's dead list \
                     (dead per R_prev, live per R_current)",
                    safepoint_rescued,
                );
            }

            // === Phase 2: Collect dead data BEFORE freeing value slots ===
            // The slot content is still valid here — we haven't pushed to the free list yet.
            // This fixes a bug in the previous code where the filtered-values branch read
            // from slots that had already been pushed to the free list (use-after-free).
            // Always derive dead data here (under GcInProgressGuard), not in
            // sweep_snapshot(). sweep_snapshot() runs on a GC pool worker without
            // mutual exclusion — reading dead slot content there races with
            // release_session_with_surviving() / process_gc_response() which may
            // concurrently poison those slots (FlyingRaven ASAN finding).
            dead_data_to_free = {
                let mut data = Vec::with_capacity(non_filtered_dead.len());
                for entry in &non_filtered_dead {
                    let inner_val = unsafe { &*(entry.ptr as *const MettaValueInner) };
                    collect_dead_data(inner_val, &mut data);
                }
                data
            };

            // === Phase 2.5: Flush exec_counts for dead values ===
            // Before freeing slots, collect non-zero exec_counts from dead values
            // and merge them into the global TieredCache. This ensures execution
            // data from short-lived hot expressions is not lost.
            //
            // Acquires COUNTER_FLUSH_LOCK to block any in-progress periodic sync
            // from reading dead slot content concurrently.
            {
                use crate::backend::bytecode::tiered_cache::global_tiered_cache;
                use crate::backend::models::metta_value_trait::MettaValueTrait as _;
                use super::gc_cron::COUNTER_FLUSH_LOCK;

                let _flush_guard = COUNTER_FLUSH_LOCK.lock();

                let cache = global_tiered_cache();
                for entry in &non_filtered_dead {
                    let page = &pages[entry.page_idx];
                    let count = page.exec_count_swap(entry.slot_idx, 0);
                    if count > 0 {
                        // SAFETY: slot content is still valid — not yet freed.
                        let value = unsafe { MettaValue::from_inner_ptr(entry.ptr as *const MettaValueInner) };
                        let state = cache.get_or_create_state(&value);
                        state.execution_count.fetch_add(count, Ordering::Relaxed);
                        cache.total_executions.fetch_add(count as u64, Ordering::Relaxed);
                        let new_count = state.execution_count.load(Ordering::Relaxed);
                        cache.maybe_trigger_bytecode(&value, &state, new_count);
                        cache.maybe_trigger_jit1(&state, new_count);
                        cache.maybe_trigger_jit2(&state, new_count);
                    }
                }
            }

            // === Phase 3: Free value slots — O(D'), zero page lookups ===
            let trace = gc_trace_enabled();
            let quarantine = gc_quarantine_enabled();
            let cycle = GC_CYCLE_COUNT.fetch_add(1, Ordering::Relaxed);

            // Collect pointers for batch push (standard mode only)
            let mut batch_ptrs: Vec<*mut u8> = if quarantine {
                Vec::new()
            } else {
                Vec::with_capacity(non_filtered_dead.len())
            };

            for entry in &non_filtered_dead {
                // SAFETY: slot content is still valid — not yet pushed to free list.
                let variant = if trace || quarantine {
                    let inner_val = unsafe { &*(entry.ptr as *const MettaValueInner) };
                    let v = discriminant_name(inner_val);
                    if trace {
                        eprintln!(
                            "[GC-FREE] ptr={:p} variant={} page={} slot={} cycle={}",
                            entry.ptr, v, entry.page_idx, entry.slot_idx, cycle
                        );
                    }
                    v
                } else {
                    ""
                };
                let page = &pages[entry.page_idx];
                page.live_count.fetch_sub(1, Ordering::Relaxed);
                // Sentinel epoch: prevents double-free by future GC cycles.
                page.set_slot_epoch(entry.slot_idx, u64::MAX);

                if quarantine {
                    // Quarantine mode: fully poison the slot (including FreeNode header)
                    // and add to quarantine list instead of free list. Any stale
                    // MettaValue reference reading the discriminant byte will trigger
                    // an ASAN heap-use-after-free report.
                    unsafe { asan_poison_slab_slot_full(entry.ptr, self.values.slot_size); }
                    gc_quarantine().lock().push(QuarantineEntry {
                        ptr: entry.ptr,
                        slot_size: self.values.slot_size,
                        page_idx: entry.page_idx,
                        slot_idx: entry.slot_idx,
                        variant,
                        gc_cycle: cycle,
                    });
                } else {
                    // Standard mode: poison after FreeNode header, collect for batch push.
                    unsafe { asan_poison_slab_slot(entry.ptr, self.values.slot_size); }
                    batch_ptrs.push(entry.ptr);
                }
            }

            // Batch push: splice all freed slots onto the free list with a single CAS
            // instead of N individual CAS operations. Reduces contention significantly
            // when hundreds/thousands of slots are freed per GC cycle.
            if !batch_ptrs.is_empty() {
                self.values.free_list.push_batch(&batch_ptrs);
            }
        } // read lock released

        // Track freed values for GC scaling monitor
        GC_VALUES_FREED_TOTAL.fetch_add(non_filtered_dead.len() as u64, Ordering::Relaxed);

        // === Phase 4: Free dead data slots — O(D_data log P_data) ===
        // Batch by size class, build sorted page index once per class.
        self.free_data_slots_batch(dead_data_to_free);

        // === Phase 5: Release empty pages ===
        // Safe: free-list entries from released pages are filtered out via
        // atomic drain + rebuild in release_empty_pages().
        // PAGE_LIFECYCLE_LOCK prevents races with mark_snapshot() and other
        // page-traversing operations (matches release_session_with_surviving).
        {
            let _page_guard = PAGE_LIFECYCLE_LOCK.write();
            self.values.release_empty_pages();
            for dc in &self.data_classes {
                dc.release_empty_pages();
            }
        }

        // Update committed bytes
        self.committed_bytes_atomic.store(self.committed_bytes(), Ordering::Relaxed);

        // Drain quarantine entries older than max_age cycles back to free list.
        // For FlyingRaven diagnosis: use u64::MAX (never drain) since the
        // program runs for bounded time. For production: 3-5 cycles.
        self.drain_quarantine(u64::MAX);
    }

    /// Drain quarantine entries older than `max_age` GC cycles back to the free list.
    /// Called at end of process_gc_response() to prevent unbounded quarantine growth
    /// in long-running programs.
    fn drain_quarantine(&self, max_age: u64) {
        if !gc_quarantine_enabled() {
            return;
        }
        let current_cycle = GC_CYCLE_COUNT.load(Ordering::Relaxed);
        let mut quarantine = gc_quarantine().lock();
        let initial_len = quarantine.len();
        let mut drained = 0usize;
        let mut i = 0;
        while i < quarantine.len() {
            if current_cycle.saturating_sub(quarantine[i].gc_cycle) >= max_age {
                let entry = quarantine.swap_remove(i);
                // Unpoison entire slot, then re-poison with FreeNode header accessible.
                unsafe {
                    asan_unpoison_slab_slot(entry.ptr, entry.slot_size);
                    asan_poison_slab_slot(entry.ptr, entry.slot_size);
                }
                self.values.free_list.push(entry.ptr);
                drained += 1;
            } else {
                i += 1;
            }
        }
        if gc_trace_enabled() && drained > 0 {
            eprintln!(
                "[GC-QUARANTINE] drained {}/{} entries (max_age={}, current_cycle={})",
                drained, initial_len, max_age, current_cycle
            );
        }
    }

    // ========================================================================
    // Session-Based GC — Bulk Release by Context ID
    // ========================================================================

    /// Release all values allocated during a session (identified by `context_id`),
    /// except those reachable from registered GC roots (surviving set).
    ///
    /// Traces roots first, then delegates to `release_session_with_surviving()`.
    #[inline]
    pub fn release_session(&self, context_id: u32) {
        if context_id == 0 {
            return; // Never release persistent values
        }
        let surviving = self.trace_surviving_set();
        self.release_session_with_surviving(context_id, &surviving);
    }

    /// Release all values allocated during a session, using a pre-computed
    /// surviving set. Used by the async session release thread for batching:
    /// trace roots once, then release multiple sessions.
    ///
    /// Protocol:
    /// 1. Scan all ValuePages for slots where `context_id == target_id`
    /// 2. For each matching slot:
    ///    - If ptr is in surviving set → **promote** to persistent (context_id=0), skip
    ///    - If epoch is `u64::MAX` → already freed, skip
    ///    - Otherwise → dead: collect data, free value slot
    /// 3. Free dead data slots in batch
    /// 4. Release empty pages
    /// 5. Update committed_bytes
    pub fn release_session_with_surviving(
        &self,
        context_id: u32,
        surviving: &PtrHashSet,
    ) {
        if context_id == 0 {
            return; // Never release persistent values
        }

        let slot_size = self.values.slot_size;
        let mut non_filtered_dead: Vec<ResolvedDead> = Vec::new();
        let dead_data_to_free: Vec<(*mut u8, usize)>;
        let mut promoted_count: u64 = 0;
        let mut scanned_count: u64 = 0;

        {
            let pages = self.values.pages.read();

            // Scan all pages for slots matching this context_id
            for (page_idx, page) in pages.iter().enumerate() {
                let bump = page.bump_count.load(Ordering::Acquire);
                for slot_idx in 0..bump {
                    if page.context_id(slot_idx) != context_id {
                        continue;
                    }

                    scanned_count += 1;

                    // Check if already freed (epoch sentinel)
                    if page.slot_epoch(slot_idx) == u64::MAX {
                        continue;
                    }

                    let ptr = page.slot_ptr(slot_idx, slot_size);

                    if surviving.contains(&(ptr as *const u8)) {
                        // Promote: value is reachable from roots, make persistent
                        page.set_context_id(slot_idx, 0);
                        promoted_count += 1;
                    } else {
                        // Dead: collect for freeing
                        non_filtered_dead.push(ResolvedDead {
                            ptr,
                            page_idx,
                            slot_idx,
                        });
                    }
                }
            }

            // Collect dead data BEFORE freeing value slots (slot content still valid)
            dead_data_to_free = {
                let mut data = Vec::new();
                for entry in &non_filtered_dead {
                    let inner_val = unsafe { &*(entry.ptr as *const MettaValueInner) };
                    let mut data_entries = Vec::new();
                    collect_dead_data(inner_val, &mut data_entries);
                    data.extend(data_entries);
                }
                data
            };

            // Free value slots — O(D'), zero page lookups (indices cached)
            let trace = gc_trace_enabled();
            let quarantine = gc_quarantine_enabled();
            let cycle = GC_CYCLE_COUNT.fetch_add(1, Ordering::Relaxed);

            for entry in &non_filtered_dead {
                let variant = if trace || quarantine {
                    let inner_val = unsafe { &*(entry.ptr as *const MettaValueInner) };
                    let v = discriminant_name(inner_val);
                    if trace {
                        eprintln!(
                            "[GC-SESSION-FREE] ptr={:p} variant={} page={} slot={} ctx={} cycle={}",
                            entry.ptr, v, entry.page_idx, entry.slot_idx, context_id, cycle
                        );
                    }
                    v
                } else {
                    ""
                };
                let page = &pages[entry.page_idx];
                page.live_count.fetch_sub(1, Ordering::Relaxed);
                // Sentinel epoch: prevents double-free by future GC cycles
                page.set_slot_epoch(entry.slot_idx, u64::MAX);

                if quarantine {
                    // Quarantine mode: fully poison the slot (including FreeNode header)
                    unsafe { asan_poison_slab_slot_full(entry.ptr, slot_size); }
                    gc_quarantine().lock().push(QuarantineEntry {
                        ptr: entry.ptr,
                        slot_size,
                        page_idx: entry.page_idx,
                        slot_idx: entry.slot_idx,
                        variant,
                        gc_cycle: cycle,
                    });
                } else {
                    // Standard mode: poison after FreeNode header, push to free list
                    unsafe { asan_poison_slab_slot(entry.ptr, slot_size); }
                    self.values.free_list.push(entry.ptr);
                }
            }
        } // read lock released

        // Phase 3: Free dead data slots in batch
        self.free_data_slots_batch(dead_data_to_free);

        // Phase 4: Release empty pages (munmap).
        // Safe because:
        // 1. Root tracing happened at quiescent state (ACTIVE_EVALUATORS == 0),
        //    capturing all reachable values. Surviving values were promoted to
        //    persistent (context_id=0), keeping their pages alive (live_count > 0).
        // 2. alloc() validates free-list pointers against the pages vector under
        //    read-lock. If a page was munmapped, the stale pointer is discarded.
        // 3. PAGE_LIFECYCLE_LOCK write ensures no concurrent mark/sweep is
        //    traversing page pointers that munmap would invalidate.
        {
            let _page_guard = PAGE_LIFECYCLE_LOCK.write();
            self.values.release_empty_pages();
            for dc in &self.data_classes {
                dc.release_empty_pages();
            }
        }

        // Phase 5: Update committed bytes
        self.committed_bytes_atomic.store(self.committed_bytes(), Ordering::Relaxed);

        // Phase 6: Update session GC statistics
        SESSION_RELEASES_TOTAL.fetch_add(1, Ordering::Relaxed);
        SESSION_VALUES_FREED_TOTAL.fetch_add(non_filtered_dead.len() as u64, Ordering::Relaxed);
        SESSION_VALUES_PROMOTED_TOTAL.fetch_add(promoted_count, Ordering::Relaxed);
        SESSION_VALUES_SCANNED_TOTAL.fetch_add(scanned_count, Ordering::Relaxed);
        LAST_SURVIVING_SET_SIZE.store(surviving.len() as u64, Ordering::Relaxed);
    }

    /// Trace the surviving set: DFS from all registered GC roots.
    ///
    /// Returns a `PtrHashSet` of all value slot pointers reachable
    /// from the root registry. Same traversal logic as `mark_snapshot()` but
    /// builds a HashSet instead of setting mark bits.
    ///
    /// Public because the async session release thread calls this to batch
    /// multiple session releases with a single root trace.
    pub fn trace_surviving_set(&self) -> PtrHashSet {
        let trace = gc_trace_enabled();
        let roots = collect_all_roots();
        if trace {
            eprintln!("[GC-TRACE] trace_surviving_set: {} root values collected", roots.len());
        }
        let mut surviving = PtrHashSet::with_capacity_and_hasher(roots.len() * 4, PtrBuildHasher);
        let mut worklist: Vec<*const MettaValueInner> = Vec::with_capacity(1024);

        // Seed worklist with root inner pointers
        for root in &roots {
            let ptr = root.inner_ptr();
            if surviving.insert(ptr as *const u8) {
                worklist.push(ptr);
            }
        }

        // DFS traversal — same variant matching as mark_snapshot()
        while let Some(ptr) = worklist.pop() {
            match unsafe { &*ptr } {
                MettaValueInner::SExpr(children) => {
                    for child in children.iter() {
                        let child_ptr = child.inner_ptr();
                        if surviving.insert(child_ptr as *const u8) {
                            worklist.push(child_ptr);
                        }
                    }
                }
                MettaValueInner::Conjunction(goals) => {
                    for goal in goals.iter() {
                        let goal_ptr = goal.inner_ptr();
                        if surviving.insert(goal_ptr as *const u8) {
                            worklist.push(goal_ptr);
                        }
                    }
                }
                MettaValueInner::Error(_, details) => {
                    let details_ptr = details.inner_ptr();
                    if surviving.insert(details_ptr as *const u8) {
                        worklist.push(details_ptr);
                    }
                }
                MettaValueInner::Type(inner) | MettaValueInner::Quoted(inner) => {
                    let inner_ptr = inner.inner_ptr();
                    if surviving.insert(inner_ptr as *const u8) {
                        worklist.push(inner_ptr);
                    }
                }
                MettaValueInner::Space(handle) => {
                    let mut space_values = Vec::new();
                    handle.collect_gc_values(&mut space_values);
                    for val in &space_values {
                        let val_ptr = val.inner_ptr();
                        if surviving.insert(val_ptr as *const u8) {
                            worklist.push(val_ptr);
                        }
                    }
                }
                MettaValueInner::Spanned(v, _) => {
                    let inner_ptr = v.inner_ptr();
                    if surviving.insert(inner_ptr as *const u8) {
                        worklist.push(inner_ptr);
                    }
                }
                MettaValueInner::Atom(_)
                | MettaValueInner::Bool(_)
                | MettaValueInner::Long(_)
                | MettaValueInner::Float(_)
                | MettaValueInner::String(_)
                | MettaValueInner::Unit
                | MettaValueInner::Empty
                | MettaValueInner::State(_)
                | MettaValueInner::Memo(_) => {}
            }
        }

        if trace {
            eprintln!("[GC-SURVIVING] count={}", surviving.len());
        }

        surviving
    }

    /// Take a watermark snapshot.
    pub fn watermark(&self) -> AllocationWatermark {
        let pages = self.values.pages.read();
        AllocationWatermark {
            value_page_count: pages.len(),
            last_page_bump_count: pages.last()
                .map(|p| p.bump_count.load(Ordering::Acquire))
                .unwrap_or(0),
        }
    }

    /// Mark a value pointer as reachable.
    pub fn mark_value(&self, ptr: *const u8) -> bool {
        let pages = self.values.pages.read();
        for page in pages.iter() {
            if let Some(idx) = page.slot_index(ptr, self.values.slot_size) {
                if page.is_marked(idx) {
                    return false;
                }
                page.set_mark(idx);
                return true;
            }
        }
        false
    }

    /// Check if a value pointer is marked.
    pub fn is_value_marked(&self, ptr: *const u8) -> bool {
        let pages = self.values.pages.read();
        for page in pages.iter() {
            if let Some(idx) = page.slot_index(ptr, self.values.slot_size) {
                return page.is_marked(idx);
            }
        }
        false
    }

    /// Clear all mark bits.
    pub fn clear_marks(&self) {
        let pages = self.values.pages.read();
        for page in pages.iter() {
            page.clear_marks();
        }
    }

    /// Process a dead set.
    pub fn process_dead_set(&self, dead_set: &DeadSet) {
        for &ptr in &dead_set.dead_values {
            unsafe { self.free_value(ptr); }
        }
        for &(ptr, size) in &dead_set.dead_data {
            self.free_data_slot(ptr, size);
        }
        self.release_empty_pages();
    }

    /// Release empty pages across all allocators via atomic drain + filter + rebuild.
    ///
    /// Each sub-allocator atomically drains its Treiber stack free list, walks
    /// the disconnected chain to filter out entries from empty pages, pushes
    /// survivors back, then `swap_remove`s the empty pages (triggering `munmap`
    /// via `MmapPage::Drop`). This returns physical memory to the OS immediately.
    ///
    /// **Hot path impact: ZERO** — alloc/free paths are unchanged. During the
    /// brief drain window, `pop()` returns `None` and alloc falls through to
    /// bump allocation (correct, lock-free).
    ///
    /// Race safety: `alloc()` validates free-list pointers against the pages
    /// vector under a read-lock after `pop()`. Stale pointers to munmapped
    /// pages are discarded, falling through to bump allocation.
    pub fn release_empty_pages(&self) {
        // Acquire exclusive lock: blocks until all in-flight mark/sweep
        // operations (which hold read locks) complete. Prevents SIGSEGV
        // from munmapping pages that mark_snapshot is currently traversing.
        let _page_guard = PAGE_LIFECYCLE_LOCK.write();
        self.values.release_empty_pages();
        for dc in &self.data_classes {
            dc.release_empty_pages();
        }
    }
}

// ============================================================================
// Snapshot-Based Mark-Sweep
// ============================================================================

/// Mark phase operating on a GcSnapshot.
pub fn mark_snapshot(snapshot: &mut GcSnapshot) {
    let slot_size = snapshot.slot_size;
    let mut worklist: Vec<*const MettaValueInner> = Vec::with_capacity(1024);

    let root_ptrs: Vec<*const MettaValueInner> = snapshot.roots
        .iter()
        .map(|root| root.inner_ptr())
        .collect();

    for ptr in root_ptrs {
        if snapshot_mark_value(snapshot, ptr as *const u8, slot_size) {
            worklist.push(ptr);
        }
    }

    while let Some(ptr) = worklist.pop() {
        match unsafe { &*ptr } {
            MettaValueInner::SExpr(children) => {
                for child in children.iter() {
                    let child_ptr = child.inner_ptr();
                    if snapshot_mark_value(snapshot, child_ptr as *const u8, slot_size) {
                        worklist.push(child_ptr);
                    }
                }
            }
            MettaValueInner::Conjunction(goals) => {
                for goal in goals.iter() {
                    let goal_ptr = goal.inner_ptr();
                    if snapshot_mark_value(snapshot, goal_ptr as *const u8, slot_size) {
                        worklist.push(goal_ptr);
                    }
                }
            }
            MettaValueInner::Error(_, details) => {
                let details_ptr = details.inner_ptr();
                if snapshot_mark_value(snapshot, details_ptr as *const u8, slot_size) {
                    worklist.push(details_ptr);
                }
            }
            MettaValueInner::Type(inner) | MettaValueInner::Quoted(inner) => {
                let inner_ptr = inner.inner_ptr();
                if snapshot_mark_value(snapshot, inner_ptr as *const u8, slot_size) {
                    worklist.push(inner_ptr);
                }
            }
            MettaValueInner::Space(handle) => {
                // Traverse into SpaceHandle to mark rule LHS/RHS and module atoms.
                // Without this, values stored as rules inside spaces would be
                // incorrectly classified as dead by the GC sweep phase.
                let mut space_values = Vec::new();
                handle.collect_gc_values(&mut space_values);
                for val in &space_values {
                    let val_ptr = val.inner_ptr();
                    if snapshot_mark_value(snapshot, val_ptr as *const u8, slot_size) {
                        worklist.push(val_ptr);
                    }
                }
            }
            MettaValueInner::Spanned(v, _) => {
                let inner_ptr = v.inner_ptr();
                if snapshot_mark_value(snapshot, inner_ptr as *const u8, slot_size) {
                    worklist.push(inner_ptr);
                }
            }
            MettaValueInner::Atom(_)
            | MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_)
            | MettaValueInner::Unit
            | MettaValueInner::Empty
            | MettaValueInner::State(_)
            | MettaValueInner::Memo(_) => {}
        }
    }
}

/// Mark a value in the snapshot's mark bitmaps.
///
/// Skips freed slots (epoch == u64::MAX) and slots allocated after the
/// snapshot epoch — their content is FreeNode data or post-snapshot values,
/// not valid MettaValueInner for this GC cycle. Dereferencing them as
/// MettaValueInner would be UB (invalid enum discriminant from FreeNode
/// data), potentially corrupting the mark bitmaps and causing live values
/// to be swept.
fn snapshot_mark_value(snapshot: &mut GcSnapshot, ptr: *const u8, slot_size: usize) -> bool {
    for (page_idx, ps) in snapshot.page_snapshots.iter().enumerate() {
        let offset = (ptr as usize).wrapping_sub(ps.data_ptr as usize);
        if offset < ps.capacity * slot_size {
            let idx = offset / slot_size;
            if idx < ps.bump_count {
                // Skip slots freed by a prior GC cycle (epoch == u64::MAX)
                // or allocated after the snapshot (epoch > snapshot_epoch).
                // These slots contain FreeNode data, not valid MettaValueInner.
                let slot_epoch = ps.epochs[idx];
                if slot_epoch == u64::MAX || slot_epoch > snapshot.snapshot_epoch {
                    return false;
                }
                let word = idx / 64;
                let bit = idx % 64;
                if (snapshot.marks[page_idx][word] & (1u64 << bit)) != 0 {
                    return false;
                }
                snapshot.marks[page_idx][word] |= 1u64 << bit;
                return true;
            }
        }
    }
    false
}

fn snapshot_is_marked(snapshot: &GcSnapshot, page_idx: usize, slot_idx: usize) -> bool {
    let word = slot_idx / 64;
    let bit = slot_idx % 64;
    (snapshot.marks[page_idx][word] & (1u64 << bit)) != 0
}

/// Sweep phase operating on a GcSnapshot.
pub fn sweep_snapshot(snapshot: &GcSnapshot) -> GcResponse {
    let slot_size = snapshot.slot_size;
    let mut response = GcResponse {
        dead_values: Vec::new(),
        snapshot_epoch: snapshot.snapshot_epoch,
        live_bytes: 0,
        live_values: 0,
    };

    for (page_idx, ps) in snapshot.page_snapshots.iter().enumerate() {
        for slot_idx in 0..ps.bump_count {
            // Skip slots freed by a prior GC cycle (epoch sentinel u64::MAX).
            // These slots have FreeNode.next overwriting their MettaValueInner
            // header and must not be read as MettaValueInner (UB: invalid
            // enum discriminant). Also skip slots allocated after the snapshot
            // (epoch > snapshot_epoch) — they weren't visible at snapshot time.
            let slot_epoch = ps.epochs[slot_idx];
            if slot_epoch == u64::MAX || slot_epoch > snapshot.snapshot_epoch {
                continue;
            }

            let ptr = unsafe { ps.data_ptr.add(slot_idx * slot_size) as *mut u8 };

            if snapshot_is_marked(snapshot, page_idx, slot_idx) {
                response.live_values += 1;
                response.live_bytes += slot_size;
                let inner_val = unsafe { &*(ptr as *const MettaValueInner) };
                response.live_bytes += data_size_of(inner_val);
            } else {
                response.dead_values.push(ptr);
                // Dead data collection deferred to process_gc_response() which
                // runs under GcInProgressGuard — reading dead slot content here
                // races with release_session_with_surviving() / process_gc_response()
                // which may concurrently poison these slots (FlyingRaven ASAN finding).
            }
        }
    }

    response
}

/// Legacy mark phase.
pub fn mark_from_roots(
    roots: impl Iterator<Item = MettaValue>,
    alloc: &SlabAllocator,
) {
    let mut worklist: Vec<*const MettaValueInner> = Vec::with_capacity(1024);

    for root in roots {
        let ptr = root.inner_ptr();
        if alloc.mark_value(ptr as *const u8) {
            worklist.push(ptr);
        }
    }

    while let Some(ptr) = worklist.pop() {
        match unsafe { &*ptr } {
            MettaValueInner::SExpr(children) => {
                for child in children.iter() {
                    let child_ptr = child.inner_ptr();
                    if alloc.mark_value(child_ptr as *const u8) {
                        worklist.push(child_ptr);
                    }
                }
            }
            MettaValueInner::Conjunction(goals) => {
                for goal in goals.iter() {
                    let goal_ptr = goal.inner_ptr();
                    if alloc.mark_value(goal_ptr as *const u8) {
                        worklist.push(goal_ptr);
                    }
                }
            }
            MettaValueInner::Error(_, details) => {
                let details_ptr = details.inner_ptr();
                if alloc.mark_value(details_ptr as *const u8) {
                    worklist.push(details_ptr);
                }
            }
            MettaValueInner::Type(inner) | MettaValueInner::Quoted(inner) => {
                let inner_ptr = inner.inner_ptr();
                if alloc.mark_value(inner_ptr as *const u8) {
                    worklist.push(inner_ptr);
                }
            }
            MettaValueInner::Space(handle) => {
                let mut space_values = Vec::new();
                handle.collect_gc_values(&mut space_values);
                for val in &space_values {
                    let val_ptr = val.inner_ptr();
                    if alloc.mark_value(val_ptr as *const u8) {
                        worklist.push(val_ptr);
                    }
                }
            }
            MettaValueInner::Spanned(v, _) => {
                let inner_ptr = v.inner_ptr();
                if alloc.mark_value(inner_ptr as *const u8) {
                    worklist.push(inner_ptr);
                }
            }
            MettaValueInner::Atom(_)
            | MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_)
            | MettaValueInner::Unit
            | MettaValueInner::Empty
            | MettaValueInner::State(_)
            | MettaValueInner::Memo(_) => {}
        }
    }
}

/// Legacy sweep phase.
pub fn sweep(alloc: &SlabAllocator, watermark: &AllocationWatermark) -> DeadSet {
    let pages = alloc.values.pages.read();
    let slot_size = alloc.values.slot_size;

    let mut dead = DeadSet::default();
    let mut live_values = 0usize;
    let mut live_bytes = 0usize;

    // The free set is empty since we can't iterate the Treiber stack.
    // This is conservative — some already-freed slots may be reported as dead,
    // but process_dead_set will just push them back to the free list (harmless).

    for (page_idx, page) in pages.iter().enumerate() {
        let sweep_limit = if page_idx < watermark.value_page_count.saturating_sub(1) {
            page.bump_count.load(Ordering::Acquire)
        } else if page_idx == watermark.value_page_count.saturating_sub(1) {
            watermark.last_page_bump_count.min(page.bump_count.load(Ordering::Acquire))
        } else {
            continue;
        };

        for slot_idx in 0..sweep_limit {
            let ptr = page.slot_ptr(slot_idx, slot_size);

            if page.is_marked(slot_idx) {
                live_values += 1;
                live_bytes += slot_size;
                let inner_val = unsafe { &*(ptr as *const MettaValueInner) };
                live_bytes += data_size_of(inner_val);
            } else {
                dead.dead_values.push(ptr);
                let inner_val = unsafe { &*(ptr as *const MettaValueInner) };
                collect_dead_data(inner_val, &mut dead.dead_data);
            }
        }
    }

    dead.live_values = live_values;
    dead.live_bytes = live_bytes;
    dead
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Compute variable-length data size associated with a value.
fn data_size_of(inner: &MettaValueInner) -> usize {
    match inner {
        MettaValueInner::Atom(s) => s.len(),
        MettaValueInner::String(s) => s.len(),
        MettaValueInner::SExpr(children) => children.len() * mem::size_of::<MettaValue>(),
        MettaValueInner::Conjunction(goals) => goals.len() * mem::size_of::<MettaValue>(),
        MettaValueInner::Error(msg, _) => msg.len(),
        MettaValueInner::Spanned(_, _) => mem::size_of::<crate::ir::Span>(),
        _ => 0,
    }
}

/// Collect dead data pointers from a dead value.
fn collect_dead_data(inner: &MettaValueInner, dead_data: &mut Vec<(*mut u8, usize)>) {
    match inner {
        MettaValueInner::Atom(s) if !s.is_empty() => {
            dead_data.push((s.as_ptr() as *mut u8, s.len()));
        }
        MettaValueInner::String(s) if !s.is_empty() => {
            dead_data.push((s.as_ptr() as *mut u8, s.len()));
        }
        MettaValueInner::SExpr(children) if !children.is_empty() => {
            let byte_len = children.len() * mem::size_of::<MettaValue>();
            dead_data.push((children.as_ptr() as *mut u8, byte_len));
        }
        MettaValueInner::Conjunction(goals) if !goals.is_empty() => {
            let byte_len = goals.len() * mem::size_of::<MettaValue>();
            dead_data.push((goals.as_ptr() as *mut u8, byte_len));
        }
        MettaValueInner::Error(msg, _) if !msg.is_empty() => {
            dead_data.push((msg.as_ptr() as *mut u8, msg.len()));
        }
        MettaValueInner::Spanned(_, span) => {
            let span_size = mem::size_of::<crate::ir::Span>();
            dead_data.push((*span as *const crate::ir::Span as *mut u8, span_size));
        }
        _ => {}
    }
}

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
        global_factory()
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
        let flags = if has_vars { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn bool(&self, b: bool) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Bool(b)))
    }

    #[inline]
    fn long(&self, n: i64) -> MettaValue {
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
        let slice = self.alloc.alloc_slice_from_iter(items);
        let inner = self.alloc.alloc_value(MettaValueInner::SExpr(slice));
        let flags = if has_vars { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn sexpr_from_slice(&self, items: &[MettaValue]) -> MettaValue {
        if items.is_empty() {
            return self.unit();
        }
        let has_vars = items.iter().any(|i| i.has_variables_fast());
        let slice = self.alloc.alloc_slice_copy(items);
        let inner = self.alloc.alloc_value(MettaValueInner::SExpr(slice));
        let flags = if has_vars { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn error(&self, msg: &str, details: MettaValue) -> MettaValue {
        let msg = self.alloc.alloc_str(msg);
        let inner = self.alloc.alloc_value(MettaValueInner::Error(msg, details));
        let flags = if details.has_variables_fast() { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn type_value(&self, value: MettaValue) -> MettaValue {
        let inner = self.alloc.alloc_value(MettaValueInner::Type(value));
        let flags = if value.has_variables_fast() { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn conjunction(&self, goals: Vec<MettaValue>) -> MettaValue {
        let has_vars = goals.iter().any(|g| g.has_variables_fast());
        let slice = self.alloc.alloc_slice_from_iter(goals);
        let inner = self.alloc.alloc_value(MettaValueInner::Conjunction(slice));
        let flags = if has_vars { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
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
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Unit))
    }

    #[inline]
    fn memo(&self, handle: super::MemoHandle) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Memo(handle)))
    }

    #[inline]
    fn quote(&self, value: MettaValue) -> MettaValue {
        let inner = self.alloc.alloc_value(MettaValueInner::Quoted(value));
        let flags = if value.has_variables_fast() { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn spanned(&self, value: MettaValue, span: crate::ir::Span) -> MettaValue {
        let span = self.alloc.alloc_span(span);
        let inner = self.alloc.alloc_value(MettaValueInner::Spanned(value, span));
        // Propagate variable flag through Spanned wrapper
        let flags = if value.has_variables_fast() { super::metta_value::FLAG_HAS_VARIABLES as u8 } else { 0 };
        MettaValue::from_inner_tagged(inner, flags)
    }

    #[inline]
    fn empty(&self) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Empty))
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
fn deserialize_slab_value(
    factory: &GcFactory,
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
            let (msg_len, varint_size) = read_varint(rest)?;
            let msg_start = varint_size;
            let msg_end = msg_start + msg_len;
            if rest.len() < msg_end {
                return Err("unexpected end of error message".to_string());
            }
            let msg = std::str::from_utf8(&rest[msg_start..msg_end])
                .map_err(|e| format!("invalid UTF-8 in error message: {}", e))?;
            let (details, details_consumed) =
                deserialize_slab_value(factory, &bytes[1 + msg_end..])?;
            Ok((factory.error(msg, details), 1 + msg_end + details_consumed))
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

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;
    use super::super::metta_value_trait::MettaValueFactory;

    #[test]
    fn test_slab_allocator_creation() {
        let alloc = SlabAllocator::new();
        assert_eq!(alloc.committed_bytes(), 0);
    }

    #[test]
    fn test_value_slot_size() {
        let alloc = SlabAllocator::new();
        let slot_size = alloc.value_slot_size();
        assert!(slot_size >= mem::size_of::<MettaValueInner>());
        assert_eq!(slot_size % SLOT_ALIGN, 0);
    }

    #[test]
    fn test_alloc_value_atom() {
        let alloc = SlabAllocator::new();
        let s = alloc.alloc_str("hello");
        let inner = alloc.alloc_value(MettaValueInner::Atom(s));
        match inner {
            MettaValueInner::Atom(a) => assert_eq!(*a, "hello"),
            _ => panic!("expected Atom"),
        }
    }

    #[test]
    fn test_alloc_value_long() {
        let alloc = SlabAllocator::new();
        let inner = alloc.alloc_value(MettaValueInner::Long(42));
        match inner {
            MettaValueInner::Long(n) => assert_eq!(*n, 42),
            _ => panic!("expected Long"),
        }
    }

    #[test]
    fn test_alloc_str_empty() {
        let alloc = SlabAllocator::new();
        let s = alloc.alloc_str("");
        assert_eq!(s, "");
    }

    #[test]
    fn test_alloc_str_nonempty() {
        let alloc = SlabAllocator::new();
        let s = alloc.alloc_str("hello world");
        assert_eq!(s, "hello world");
    }

    #[test]
    fn test_alloc_str_unicode() {
        let alloc = SlabAllocator::new();
        let s = alloc.alloc_str("こんにちは世界");
        assert_eq!(s, "こんにちは世界");
    }

    #[test]
    fn test_alloc_many_values() {
        let alloc = SlabAllocator::new();
        let mut refs = Vec::new();
        for i in 0..10_000 {
            let inner = alloc.alloc_value(MettaValueInner::Long(i));
            refs.push(inner);
        }
        for (i, inner) in refs.iter().enumerate() {
            match inner {
                MettaValueInner::Long(n) => assert_eq!(*n, i as i64),
                _ => panic!("expected Long at index {}", i),
            }
        }
    }

    #[test]
    fn test_alloc_many_strings() {
        let alloc = SlabAllocator::new();
        let mut refs = Vec::new();
        for i in 0..1000 {
            let s = format!("string-{}", i);
            let r = alloc.alloc_str(&s);
            refs.push((s, r));
        }
        for (expected, actual) in &refs {
            assert_eq!(*actual, expected.as_str());
        }
    }

    #[test]
    fn test_alloc_slice_empty() {
        let alloc = SlabAllocator::new();
        let slice: &[MettaValue] = alloc.alloc_slice_from_iter(std::iter::empty());
        assert!(slice.is_empty());
    }

    #[test]
    fn test_alloc_slice() {
        let alloc = SlabAllocator::new();
        let v1 = MettaValue::from_inner(alloc.alloc_value(MettaValueInner::Long(1)));
        let v2 = MettaValue::from_inner(alloc.alloc_value(MettaValueInner::Long(2)));
        let v3 = MettaValue::from_inner(alloc.alloc_value(MettaValueInner::Long(3)));

        let slice = alloc.alloc_slice_from_iter(vec![v1, v2, v3]);
        assert_eq!(slice.len(), 3);
        assert_eq!(slice[0].as_long(), Some(1));
        assert_eq!(slice[1].as_long(), Some(2));
        assert_eq!(slice[2].as_long(), Some(3));
    }

    #[test]
    fn test_alloc_slice_copy() {
        let alloc = SlabAllocator::new();
        let v1 = MettaValue::from_inner(alloc.alloc_value(MettaValueInner::Long(10)));
        let v2 = MettaValue::from_inner(alloc.alloc_value(MettaValueInner::Long(20)));
        let original = &[v1, v2];

        let copy = alloc.alloc_slice_copy(original);
        assert_eq!(copy.len(), 2);
        assert_eq!(copy[0].as_long(), Some(10));
        assert_eq!(copy[1].as_long(), Some(20));
    }

    #[test]
    fn test_committed_bytes_grows() {
        let alloc = SlabAllocator::new();
        let before = alloc.committed_bytes();
        for i in 0..1000 {
            alloc.alloc_value(MettaValueInner::Long(i));
        }
        let after = alloc.committed_bytes();
        assert!(after > before, "committed bytes should grow");
    }

    #[test]
    fn test_free_list_reuse() {
        let alloc = SlabAllocator::new();
        let inner = alloc.alloc_value(MettaValueInner::Long(42));
        let ptr = inner as *const MettaValueInner as *mut u8;
        unsafe { alloc.free_value(ptr); }

        let inner2 = alloc.alloc_value(MettaValueInner::Long(99));
        let ptr2 = inner2 as *const MettaValueInner as *mut u8;
        assert_eq!(ptr, ptr2, "freed slot should be reused");
    }

    #[test]
    fn test_gc_threshold() {
        let alloc = SlabAllocator::new();
        assert_eq!(alloc.gc_threshold.load(Ordering::Relaxed), MIN_GC_THRESHOLD);
        alloc.set_gc_threshold(1);
        assert_eq!(alloc.gc_threshold.load(Ordering::Relaxed), 1);
        alloc.alloc_value(MettaValueInner::Long(1));
        // committed_bytes should exceed threshold of 1
        assert!(alloc.committed_bytes() >= 1);
    }

    #[test]
    fn test_large_string_alloc() {
        let alloc = SlabAllocator::new();
        let large = "x".repeat(8000);
        let s = alloc.alloc_str(&large);
        assert_eq!(s.len(), 8000);
        assert_eq!(s, large.as_str());
    }

    #[test]
    fn test_debug_display() {
        let alloc = SlabAllocator::new();
        let debug_str = format!("{:?}", alloc);
        assert!(debug_str.contains("SlabAllocator"));
        assert!(debug_str.contains("value_pages"));
    }

    #[test]
    fn test_contains_value() {
        let alloc = SlabAllocator::new();
        let inner = alloc.alloc_value(MettaValueInner::Long(42));
        let ptr = inner as *const MettaValueInner as *const u8;
        assert!(alloc.contains_value(ptr));

        let stack_var: u8 = 0;
        assert!(!alloc.contains_value(&stack_var as *const u8));
    }

    #[test]
    fn test_multiple_pages() {
        let alloc = SlabAllocator::new();
        let slot_size = alloc.value_slot_size();
        let slots_per_page = PAGE_SIZE / slot_size;

        for i in 0..(slots_per_page + 100) {
            alloc.alloc_value(MettaValueInner::Long(i as i64));
        }

        let pages = alloc.values.pages.read();
        assert!(pages.len() >= 2,
            "expected at least 2 pages, got {}", pages.len());
    }

    // ====================================================================
    // GcFactory Tests
    // ====================================================================

    fn test_factory(alloc: &SlabAllocator) -> GcFactory {
        let static_ref: &'static SlabAllocator = unsafe {
            &*(alloc as *const SlabAllocator)
        };
        GcFactory::new(static_ref)
    }

    #[test]
    fn test_gc_factory_atom() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.atom("hello");
        assert!(v.is_atom());
        assert_eq!(v.as_atom(), Some("hello"));
    }

    #[test]
    fn test_gc_factory_bool() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let t = factory.bool(true);
        let f = factory.bool(false);
        assert!(t.is_bool());
        assert!(f.is_bool());
        assert_eq!(t.as_bool(), Some(true));
        assert_eq!(f.as_bool(), Some(false));
    }

    #[test]
    fn test_gc_factory_long() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.long(42);
        assert!(v.is_long());
        assert_eq!(v.as_long(), Some(42));
    }

    #[test]
    fn test_gc_factory_float() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.float(3.14);
        assert!(v.is_float());
        assert_eq!(v.as_float(), Some(3.14));
    }

    #[test]
    fn test_gc_factory_string() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.string("world");
        assert!(v.is_string());
        assert_eq!(v.as_string(), Some("world"));
    }

    #[test]
    fn test_gc_factory_nil() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.unit();
        assert!(v.is_unit());
    }

    #[test]
    fn test_gc_factory_unit() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.unit();
        assert!(v.is_unit());
    }

    #[test]
    fn test_gc_factory_empty() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.empty();
        assert!(v.is_empty());
    }

    #[test]
    fn test_gc_factory_sexpr() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let items = vec![factory.atom("+"), factory.long(1), factory.long(2)];
        let v = factory.sexpr(items);
        assert!(v.is_sexpr());
        let elems = v.as_sexpr().expect("should be sexpr");
        assert_eq!(elems.len(), 3);
        assert_eq!(elems[0].as_atom(), Some("+"));
        assert_eq!(elems[1].as_long(), Some(1));
        assert_eq!(elems[2].as_long(), Some(2));
    }

    #[test]
    fn test_gc_factory_sexpr_from_slice() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let a = factory.atom("a");
        let b = factory.atom("b");
        let items = &[a, b];
        let v = factory.sexpr_from_slice(items);
        assert!(v.is_sexpr());
        let elems = v.as_sexpr().expect("should be sexpr");
        assert_eq!(elems.len(), 2);
        assert_eq!(elems[0].as_atom(), Some("a"));
        assert_eq!(elems[1].as_atom(), Some("b"));
    }

    #[test]
    fn test_gc_factory_error() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let details = factory.atom("bad-input");
        let v = factory.error("oops", details);
        assert!(v.is_error());
        let (msg, det) = v.as_error().expect("should be error");
        assert_eq!(msg, "oops");
        assert_eq!(det.as_atom(), Some("bad-input"));
    }

    #[test]
    fn test_gc_factory_type_value() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let inner = factory.atom("Int");
        let v = factory.type_value(inner);
        assert!(v.is_type());
    }

    #[test]
    fn test_gc_factory_conjunction() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let goals = vec![factory.atom("a"), factory.atom("b"), factory.atom("c")];
        let v = factory.conjunction(goals);
        let conj = v.as_conjunction().expect("should be conjunction");
        assert_eq!(conj.len(), 3);
        assert_eq!(conj[0].as_atom(), Some("a"));
        assert_eq!(conj[1].as_atom(), Some("b"));
        assert_eq!(conj[2].as_atom(), Some("c"));
    }

    #[test]
    fn test_gc_factory_state() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v = factory.state(42);
        assert!(v.is_state());
    }

    #[test]
    fn test_gc_factory_nested_sexpr() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let inner = factory.sexpr(vec![factory.atom("inner"), factory.long(1)]);
        let outer = factory.sexpr(vec![factory.atom("outer"), inner]);
        assert!(outer.is_sexpr());
        let items = outer.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 2);
        assert!(items[1].is_sexpr());
        let inner_items = items[1].as_sexpr().expect("should be sexpr");
        assert_eq!(inner_items.len(), 2);
        assert_eq!(inner_items[0].as_atom(), Some("inner"));
        assert_eq!(inner_items[1].as_long(), Some(1));
    }

    #[test]
    fn test_gc_factory_many_allocations() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let mut values = Vec::new();
        for i in 0..5000 {
            values.push(factory.long(i));
        }
        for (i, v) in values.iter().enumerate() {
            assert_eq!(v.as_long(), Some(i as i64), "value at index {} corrupted", i);
        }
    }

    #[test]
    fn test_gc_factory_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<GcFactory>();
        assert_sync::<GcFactory>();
    }

    #[test]
    fn test_gc_factory_debug() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let debug = format!("{:?}", factory);
        assert!(debug.contains("GcFactory"));
    }

    // ====================================================================
    // GC Mark-Sweep Tests
    // ====================================================================

    #[test]
    fn test_mark_and_check_value() {
        let alloc = SlabAllocator::new();
        let inner = alloc.alloc_value(MettaValueInner::Long(42));
        let ptr = inner as *const MettaValueInner as *const u8;
        assert!(!alloc.is_value_marked(ptr));
        assert!(alloc.mark_value(ptr));
        assert!(!alloc.mark_value(ptr));
        assert!(alloc.is_value_marked(ptr));
        alloc.clear_marks();
        assert!(!alloc.is_value_marked(ptr));
    }

    #[test]
    fn test_mark_multiple_values() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v1 = factory.long(1);
        let v2 = factory.long(2);
        let v3 = factory.long(3);
        alloc.mark_value(v1.inner_ptr() as *const u8);
        alloc.mark_value(v3.inner_ptr() as *const u8);
        assert!(alloc.is_value_marked(v1.inner_ptr() as *const u8));
        assert!(!alloc.is_value_marked(v2.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(v3.inner_ptr() as *const u8));
    }

    #[test]
    fn test_watermark_snapshot() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let _v1 = factory.long(1);
        let _v2 = factory.long(2);
        let wm = alloc.watermark();
        assert!(wm.value_page_count >= 1);
        assert!(wm.last_page_bump_count >= 2);
        let _v3 = factory.long(3);
        let wm2 = alloc.watermark();
        assert!(wm2.last_page_bump_count > wm.last_page_bump_count
            || wm2.value_page_count > wm.value_page_count);
    }

    #[test]
    fn test_mark_from_roots_simple() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v1 = factory.long(1);
        let v2 = factory.long(2);
        let _v3 = factory.long(3);
        mark_from_roots(vec![v1, v2].into_iter(), &alloc);
        assert!(alloc.is_value_marked(v1.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(v2.inner_ptr() as *const u8));
        assert!(!alloc.is_value_marked(_v3.inner_ptr() as *const u8));
    }

    #[test]
    fn test_mark_from_roots_nested() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let atom_plus = factory.atom("+");
        let num1 = factory.long(1);
        let num2 = factory.long(2);
        let expr = factory.sexpr(vec![atom_plus, num1, num2]);
        mark_from_roots(std::iter::once(expr), &alloc);
        assert!(alloc.is_value_marked(expr.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(atom_plus.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(num1.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(num2.inner_ptr() as *const u8));
    }

    #[test]
    fn test_mark_from_roots_error_traces_details() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let details = factory.atom("bad-input");
        let err = factory.error("oops", details);
        mark_from_roots(std::iter::once(err), &alloc);
        assert!(alloc.is_value_marked(err.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(details.inner_ptr() as *const u8));
    }

    #[test]
    fn test_mark_from_roots_type_traces_inner() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let inner = factory.atom("Int");
        let tv = factory.type_value(inner);
        mark_from_roots(std::iter::once(tv), &alloc);
        assert!(alloc.is_value_marked(tv.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(inner.inner_ptr() as *const u8));
    }

    #[test]
    fn test_sweep_collects_dead() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let alive = factory.long(1);
        let _dead1 = factory.long(2);
        let _dead2 = factory.long(3);
        let wm = alloc.watermark();
        mark_from_roots(std::iter::once(alive), &alloc);
        let dead_set = sweep(&alloc, &wm);
        assert!(dead_set.dead_values.len() >= 2,
            "expected at least 2 dead values, got {}", dead_set.dead_values.len());
        assert_eq!(dead_set.live_values, 1);
        alloc.clear_marks();
    }

    #[test]
    fn test_sweep_respects_watermark() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let v1 = factory.long(1);
        let wm = alloc.watermark();
        let _v2 = factory.long(2);
        let dead_set = sweep(&alloc, &wm);
        // v1 should be dead (unmarked), v2 is after watermark
        assert_eq!(dead_set.dead_values.len(), 1);
        let dead_ptrs: std::collections::HashSet<*const u8> = dead_set.dead_values.iter()
            .map(|&p| p as *const u8).collect();
        assert!(dead_ptrs.contains(&(v1.inner_ptr() as *const u8)));
    }

    #[test]
    fn test_sweep_collects_dead_data() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let alive = factory.long(42);
        let _dead_atom = factory.atom("dead-string");
        let wm = alloc.watermark();
        mark_from_roots(std::iter::once(alive), &alloc);
        let dead_set = sweep(&alloc, &wm);
        assert!(!dead_set.dead_data.is_empty(),
            "expected dead data for atom string");
        alloc.clear_marks();
    }

    #[test]
    fn test_process_dead_set_returns_to_free_list() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let alive = factory.long(1);
        let dead = factory.long(2);
        let dead_ptr = dead.inner_ptr() as *mut u8;
        let wm = alloc.watermark();
        mark_from_roots(std::iter::once(alive), &alloc);
        let dead_set = sweep(&alloc, &wm);
        alloc.process_dead_set(&dead_set);
        let new_inner = alloc.alloc_value(MettaValueInner::Long(99));
        let new_ptr = new_inner as *const MettaValueInner as *mut u8;
        assert_eq!(new_ptr, dead_ptr, "freed slot should be reused");
        alloc.clear_marks();
    }


    #[test]
    fn test_full_gc_cycle() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);
        let root1 = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        let root2 = factory.atom("keep-me");
        let _garbage1 = factory.long(999);
        let _garbage2 = factory.atom("throw-away");
        let _garbage3 = factory.sexpr(vec![factory.atom("dead"), factory.atom("expr")]);
        let wm = alloc.watermark();
        mark_from_roots(vec![root1, root2].into_iter(), &alloc);
        let dead_set = sweep(&alloc, &wm);
        assert!(dead_set.dead_values.len() >= 3,
            "expected at least 3 dead values, got {}", dead_set.dead_values.len());
        alloc.process_dead_set(&dead_set);
        alloc.clear_marks();
        // Verify live values are still accessible after GC
        assert_eq!(root1.as_sexpr().expect("root1 is sexpr").len(), 3);
        assert_eq!(root2.as_atom(), Some("keep-me"));
    }

    #[test]
    fn test_dead_set_send() {
        fn assert_send<T: Send>() {}
        assert_send::<DeadSet>();
    }

    // ====================================================================
    // Thread-Safety Tests
    // ====================================================================

    #[test]
    fn test_concurrent_allocation() {
        let alloc = Box::leak(Box::new(SlabAllocator::new()));
        let factory = GcFactory::new(alloc);

        let handles: Vec<_> = (0..4).map(|t| {
            let f = factory;
            thread::spawn(move || {
                let mut values = Vec::new();
                for i in 0..1000 {
                    values.push(f.long(t * 1000 + i));
                }
                // Verify all values
                for (i, v) in values.iter().enumerate() {
                    assert_eq!(v.as_long(), Some(t * 1000 + i as i64));
                }
            })
        }).collect();

        for h in handles {
            h.join().expect("thread panicked");
        }
    }

    #[test]
    fn test_global_allocator_init() {
        init_global_allocator();
        let alloc = global_allocator();
        let factory = global_factory();
        let v = factory.long(42);
        assert_eq!(v.as_long(), Some(42));
        assert!(alloc.contains_value(v.inner_ptr() as *const u8));
    }

    #[test]
    fn test_mmap_page_basic() {
        let page = MmapPage::new(PAGE_SIZE);
        assert!(!page.ptr.is_null());
        // Write and read back
        unsafe {
            *page.ptr = 42;
            assert_eq!(*page.ptr, 42);
        }
        // Drop will call munmap
    }

    // ========================================================================
    // Session-Based GC — Context ID Infrastructure Tests
    // ========================================================================

    #[test]
    fn test_context_id_persistent_by_default() {
        // Without a SessionGuard, allocations should get context_id=0 (persistent)
        assert_eq!(current_context_id(), 0);
        let factory = global_factory();
        let v = factory.long(42);
        let alloc = global_allocator();
        let pages = alloc.values.pages.read();
        let slot_size = alloc.values.slot_size;
        for page in pages.iter() {
            if let Some(idx) = page.slot_index(v.inner_ptr() as *const u8, slot_size) {
                assert_eq!(page.context_id(idx), 0, "persistent alloc should have context_id=0");
                return;
            }
        }
        panic!("value not found in any page");
    }

    #[test]
    fn test_session_guard_sets_context_id() {
        let guard = SessionGuard::enter();
        let ctx_id = guard.context_id();
        assert_ne!(ctx_id, 0, "session context ID should be non-zero");
        assert_eq!(current_context_id(), ctx_id, "thread-local should match guard");

        // Allocate a value inside the session
        let factory = global_factory();
        let v = factory.long(123);
        let alloc = global_allocator();
        let pages = alloc.values.pages.read();
        let slot_size = alloc.values.slot_size;
        for page in pages.iter() {
            if let Some(idx) = page.slot_index(v.inner_ptr() as *const u8, slot_size) {
                assert_eq!(page.context_id(idx), ctx_id,
                    "value allocated inside session should have session's context_id");
                drop(guard);
                assert_eq!(current_context_id(), 0, "thread-local should be cleared after drop");
                return;
            }
        }
        panic!("value not found in any page");
    }

    #[test]
    fn test_session_guard_clears_on_drop() {
        {
            let _guard = SessionGuard::enter();
            assert_ne!(current_context_id(), 0);
        }
        assert_eq!(current_context_id(), 0, "context ID should be 0 after guard drops");
    }

    #[test]
    fn test_session_ids_monotonically_increasing() {
        let g1 = SessionGuard::enter();
        let id1 = g1.context_id();
        drop(g1);

        let g2 = SessionGuard::enter();
        let id2 = g2.context_id();
        drop(g2);

        let g3 = SessionGuard::enter();
        let id3 = g3.context_id();
        drop(g3);

        assert!(id2 > id1, "session IDs should be monotonically increasing");
        assert!(id3 > id2, "session IDs should be monotonically increasing");
    }

    #[test]
    fn test_session_id_never_zero() {
        // Context ID 0 is reserved as the "persistent" sentinel.
        // SessionGuard::enter() must skip 0 even on wrap-around.
        // We can't easily test the full u32 wrap, but we can verify the
        // invariant: every SessionGuard has context_id != 0.
        for _ in 0..100 {
            let guard = SessionGuard::enter();
            assert_ne!(guard.context_id(), 0, "session context_id must never be 0 (persistent sentinel)");
            drop(guard);
        }
    }

    #[test]
    fn test_multiple_values_in_session() {
        let guard = SessionGuard::enter();
        let ctx_id = guard.context_id();
        let factory = global_factory();

        // Allocate several values of different types
        let v1 = factory.long(1);
        let v2 = factory.atom("test");
        let v3 = factory.bool(true);
        let v4 = factory.sexpr(vec![v1, v2]);

        let alloc = global_allocator();
        let pages = alloc.values.pages.read();
        let slot_size = alloc.values.slot_size;

        for val in &[v1, v2, v3, v4] {
            let mut found = false;
            for page in pages.iter() {
                if let Some(idx) = page.slot_index(val.inner_ptr() as *const u8, slot_size) {
                    assert_eq!(page.context_id(idx), ctx_id,
                        "all values in session should share context_id");
                    found = true;
                    break;
                }
            }
            assert!(found, "value not found in any page");
        }
        drop(guard);
    }

    #[test]
    fn test_concurrent_sessions_different_ids() {
        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4).map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let guard = SessionGuard::enter();
                let id = guard.context_id();
                barrier.wait(); // All threads have their session IDs
                assert_ne!(id, 0);
                drop(guard);
                id
            })
        }).collect();

        let ids: Vec<u32> = handles.into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect();

        // All IDs should be unique
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "concurrent session IDs should be unique");
    }

    // ========================================================================
    // Session-Based GC — release_session() Tests
    // ========================================================================

    #[test]
    fn test_release_session_frees_dead_values() {
        let alloc = global_allocator();
        let factory = global_factory();

        // Allocate values in a session
        let guard = SessionGuard::enter();
        let ctx_id = guard.context_id();
        let _v1 = factory.long(10001);
        let _v2 = factory.long(10002);
        let _v3 = factory.atom("session-garbage");

        // Manually release synchronously (testing the core release logic)
        THREAD_CONTEXT_ID.with(|c| c.set(0));
        alloc.release_session(ctx_id);

        // After release, allocator should still be functional
        let v = factory.long(42);
        assert_eq!(v.as_long(), Some(42));

        // Prevent double release from guard drop
        mem::forget(guard);
    }

    #[test]
    fn test_release_session_preserves_persistent_values() {
        let factory = global_factory();
        let alloc = global_allocator();

        // Allocate persistent values (no session guard)
        let persistent = factory.long(99999);
        let persistent_ptr = persistent.inner_ptr() as *const u8;

        // Start a session and allocate garbage
        let guard = SessionGuard::enter();
        let ctx_id = guard.context_id();
        let _garbage = factory.long(88888);
        THREAD_CONTEXT_ID.with(|c| c.set(0));

        // Release session
        alloc.release_session(ctx_id);

        // Persistent value should still be accessible
        assert_eq!(persistent.as_long(), Some(99999));
        assert!(alloc.contains_value(persistent_ptr),
            "persistent value should still exist after session release");

        mem::forget(guard);
    }

    #[test]
    fn test_release_session_context_zero_is_noop() {
        let alloc = global_allocator();
        // Releasing context 0 should return immediately without crashing
        alloc.release_session(0);
        // Allocator should still work after no-op release
        let factory = global_factory();
        let v = factory.long(42);
        assert_eq!(v.as_long(), Some(42));
    }

    #[test]
    fn test_session_guard_drop_releases() {
        let factory = global_factory();

        // Create a session, allocate values, then drop the guard
        {
            let _guard = SessionGuard::enter();
            for i in 0..100 {
                let _v = factory.long(i + 50000);
            }
            // guard drops here, triggering release_session()
        }

        // After drop, no crash and allocator is still functional
        let v = factory.long(42);
        assert_eq!(v.as_long(), Some(42));
    }

    #[test]
    fn test_session_guard_drop_with_gc_disabled() {
        // Temporarily disable GC
        let was_disabled = is_gc_disabled();
        if !was_disabled {
            disable_gc();
        }

        let factory = global_factory();
        {
            let _guard = SessionGuard::enter();
            let _v = factory.long(77777);
            // guard drops but release_session is skipped when GC is disabled
        }

        // Re-enable GC for other tests
        if !was_disabled {
            GC_DISABLED.store(false, Ordering::Release);
        }

        // Should not crash
        let v = factory.long(88888);
        assert_eq!(v.as_long(), Some(88888));
    }

    // ================================================================
    // Safepoint Root Registry Tests
    // ================================================================

    #[test]
    fn test_register_temporary_roots_basic() {
        let factory = global_factory();
        let v1 = factory.long(111);
        let v2 = factory.atom("safepoint_test");

        let handle = register_temporary_roots(vec![v1, v2]);

        // Roots should appear in collect_all_roots()
        let roots = collect_all_roots();
        let has_v1 = roots.iter().any(|r| r.as_long() == Some(111));
        let has_v2 = roots.iter().any(|r| r.as_atom() == Some("safepoint_test"));
        assert!(has_v1, "safepoint root v1 should be in collect_all_roots()");
        assert!(has_v2, "safepoint root v2 should be in collect_all_roots()");

        // Drop handle — roots should be cleared
        drop(handle);

        // After drop, roots should no longer include our values
        // (other roots from environments may still be present)
        let roots_after = collect_all_roots();
        let still_has_v2 = roots_after.iter().any(|r| r.as_atom() == Some("safepoint_test"));
        assert!(!still_has_v2, "safepoint roots should be cleared after handle drop");
    }

    #[test]
    fn test_register_temporary_roots_multiple_handles() {
        let factory = global_factory();

        let handle1 = register_temporary_roots(vec![factory.long(1001)]);
        let handle2 = register_temporary_roots(vec![factory.long(1002)]);

        // Both should appear
        let roots = collect_all_roots();
        assert!(roots.iter().any(|r| r.as_long() == Some(1001)));
        assert!(roots.iter().any(|r| r.as_long() == Some(1002)));

        // Drop first handle
        drop(handle1);
        let roots = collect_all_roots();
        assert!(!roots.iter().any(|r| r.as_long() == Some(1001)));
        assert!(roots.iter().any(|r| r.as_long() == Some(1002)));

        // Drop second handle
        drop(handle2);
        let roots = collect_all_roots();
        assert!(!roots.iter().any(|r| r.as_long() == Some(1002)));
    }

    #[test]
    fn test_register_temporary_roots_slot_reuse() {
        let factory = global_factory();

        // Register and drop to create an empty slot
        let handle = register_temporary_roots(vec![factory.long(2001)]);
        drop(handle);

        // Next registration should reuse the empty slot
        let handle2 = register_temporary_roots(vec![factory.long(2002)]);
        let roots = collect_all_roots();
        assert!(roots.iter().any(|r| r.as_long() == Some(2002)));
        assert!(!roots.iter().any(|r| r.as_long() == Some(2001)));
        drop(handle2);
    }

    // ================================================================
    // EvalGuard Drop/Reacquire Tests
    // ================================================================

    #[test]
    fn test_eval_guard_depth_tracking() {
        let guard = EvalGuard::enter();
        assert!(active_evaluator_count() >= 1);

        // Depth should be at least 1
        EVAL_GUARD_DEPTH.with(|d| assert!(d.get() >= 1));

        drop(guard);
    }

    #[test]
    fn test_safepoint_drop_reacquire_cycle() {
        let _guard = EvalGuard::enter();
        let before = active_evaluator_count();
        assert!(before >= 1, "should have at least our own guard");

        // Drop for safepoint
        drop_eval_guard_for_safepoint();
        let during = active_evaluator_count();
        // Use relative check: other parallel tests may concurrently change
        // ACTIVE_EVALUATORS, so we can only assert our decrement was observed.
        assert!(during < before, "drop_eval_guard should decrement ACTIVE_EVALUATORS");

        // Re-acquire
        reacquire_eval_guard_after_safepoint();
        let after = active_evaluator_count();
        assert!(after > during, "reacquire should increment ACTIVE_EVALUATORS");

        // _guard drops here. Since drop_eval_guard_for_safepoint() +
        // reacquire_eval_guard_after_safepoint() is a balanced pair (restores both
        // EVAL_GUARD_DEPTH and ACTIVE_EVALUATORS), the guard's Drop will correctly
        // decrement both depth and ACTIVE_EVALUATORS exactly once.
    }

    #[test]
    fn test_safepoint_with_temporary_roots() {
        let factory = global_factory();
        let _guard = EvalGuard::enter();

        // Simulate safepoint: register roots, drop guard, re-acquire
        let roots = vec![factory.long(9999), factory.atom("safepoint_root")];
        let root_handle = register_temporary_roots(roots);

        drop_eval_guard_for_safepoint();

        // Roots should be visible in collect_all_roots() while guard is dropped
        let all_roots = collect_all_roots();
        assert!(
            all_roots.iter().any(|r| r.as_long() == Some(9999)),
            "safepoint roots should be visible during safepoint"
        );

        reacquire_eval_guard_after_safepoint();
        drop(root_handle);
        drop(_guard);
    }

    #[test]
    fn test_committed_bytes_snapshot_returns_value() {
        // Allocate something to ensure committed_bytes > 0
        let factory = global_factory();
        let _v = factory.long(12345);
        // Just verify it returns without panic
        let bytes = committed_bytes_snapshot();
        // Can be 0 if counter hasn't updated yet (it updates every 1024 allocs),
        // but should at least not panic
        let _ = bytes;
    }
}
