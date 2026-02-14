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
use std::sync::{Arc, OnceLock, Weak};
use parking_lot::{Condvar, Mutex, RwLock};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use portable_atomic::AtomicU128;

use super::metta_value::{MettaValue, MettaValueInner};

// ============================================================================
// Constants
// ============================================================================

/// Page size for value and data slabs (64 KB).
const PAGE_SIZE: usize = 64 * 1024;

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
                std::ptr::null_mut(),
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
struct ValuePage {
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
}

impl ValuePage {
    /// Create a new value page for the given slot size.
    fn new(slot_size: usize) -> Self {
        let capacity = PAGE_SIZE / slot_size;
        let mark_words = (capacity + 63) / 64;
        let data = MmapPage::new(PAGE_SIZE);
        let marks: Vec<AtomicU64> = (0..mark_words).map(|_| AtomicU64::new(0)).collect();
        let epochs: Vec<AtomicU64> = (0..capacity).map(|_| AtomicU64::new(0)).collect();
        Self {
            data,
            bump_count: AtomicUsize::new(0),
            capacity,
            live_count: AtomicIsize::new(0),
            marks,
            epochs,
        }
    }

    /// Get pointer to slot at the given index.
    #[inline]
    fn slot_ptr(&self, idx: usize, slot_size: usize) -> *mut u8 {
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
    fn slot_epoch(&self, idx: usize) -> u64 {
        self.epochs[idx].load(Ordering::Acquire)
    }

    /// Set slot epoch (atomic).
    #[inline]
    fn set_slot_epoch(&self, idx: usize, epoch: u64) {
        self.epochs[idx].store(epoch, Ordering::Release);
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
#[repr(C)]
struct FreeNode {
    /// Packed [64-bit counter | 64-bit pointer] to the next free node.
    next: u128,
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

    /// Push a freed slot onto the stack (lock-free).
    fn push(&self, ptr: *mut u8) {
        loop {
            let old_head = self.head.load(Ordering::Acquire);
            // Write the current head as this node's next pointer
            let node = ptr as *mut FreeNode;
            unsafe { (*node).next = old_head; }
            // Pack with incremented counter for ABA prevention
            let old_counter = treiber_unpack_counter(old_head);
            let new_head = treiber_pack(ptr, old_counter.wrapping_add(1));
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
            let ptr = treiber_unpack_ptr(old_head);
            let old_counter = treiber_unpack_counter(old_head);
            // Read the next pointer from the node
            let next = unsafe { (*(ptr as *const FreeNode)).next };
            // Pack with incremented counter
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
                Ok(_) => return Some(ptr),
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

    /// Check if the stack is empty (approximate, for diagnostics only).
    fn is_empty(&self) -> bool {
        self.head.load(Ordering::Relaxed) == TREIBER_NULL
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

/// Size of the Treiber stack FreeNode header (u128 = 16 bytes).
/// This region is left unpoisoned when a slot is freed because the
/// Treiber stack stores its `next` pointer there.
const FREE_NODE_SIZE: usize = std::mem::size_of::<FreeNode>();

/// Poison a slab slot after freeing it.
///
/// Marks bytes `FREE_NODE_SIZE..slot_size` as inaccessible to ASAN. The first
/// `FREE_NODE_SIZE` bytes are left unpoisoned because the Treiber stack stores
/// `FreeNode.next` (a u128 = 16 bytes) at the beginning of the freed slot.
#[inline(always)]
#[allow(unused_variables)]
unsafe fn asan_poison_slab_slot(ptr: *mut u8, slot_size: usize) {
    #[cfg(sanitize = "address")]
    {
        extern "C" {
            fn __asan_poison_memory_region(addr: *const std::ffi::c_void, size: usize);
        }
        if slot_size > FREE_NODE_SIZE {
            __asan_poison_memory_region(
                ptr.add(FREE_NODE_SIZE) as *const std::ffi::c_void,
                slot_size - FREE_NODE_SIZE,
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
            current_page: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    /// Allocate a slot of this size class (lock-free hot path).
    fn alloc(&self) -> *mut u8 {
        // Fast path: pop from Treiber stack free list
        if let Some(ptr) = self.free_list.pop() {
            // ASAN: unpoison the slot before reuse
            unsafe { asan_unpoison_slab_slot(ptr, self.slot_size); }
            // Increment page live_count for the page containing this slot
            self.increment_page_live_count(ptr);
            return ptr;
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
    fn increment_page_live_count(&self, ptr: *mut u8) {
        let pages = self.pages.read();
        for page in pages.iter() {
            if page.contains(ptr as *const u8, self.slot_size) {
                page.live_count.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    /// Return a slot to the free list (lock-free).
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
            // Read next BEFORE any munmap — slot memory is still mapped here
            let next = unsafe { (*(ptr as *const FreeNode)).next };

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

        // Phase 6: Update current_page if all pages were released
        if pages.is_empty() {
            self.current_page.store(std::ptr::null_mut(), Ordering::Release);
        }
        // Otherwise current_page is still valid — we excluded it from release,
        // and Box<DataPage> heap address is stable across swap_remove.
    }
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
        let raw_size = std::mem::size_of::<MettaValueInner>();
        let slot_size = (raw_size + SLOT_ALIGN - 1) & !(SLOT_ALIGN - 1);
        Self {
            slot_size,
            pages: RwLock::new(Vec::new()),
            free_list: TreiberStack::new(),
            epoch: AtomicU64::new(0),
            current_page: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    /// Allocate a value slot (lock-free hot path).
    ///
    /// Free-list allocations increment the epoch and tag the slot.
    /// Bump allocations don't need epoch tagging.
    fn alloc(&self) -> *mut u8 {
        // Fast path: pop from Treiber stack free list
        if let Some(ptr) = self.free_list.pop() {
            // EPOCH: increment and tag the re-allocated slot
            let new_epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            // Find the page and set the slot's epoch + increment page live_count
            let pages = self.pages.read();
            for page in pages.iter() {
                if let Some(idx) = page.slot_index(ptr as *const u8, self.slot_size) {
                    page.set_slot_epoch(idx, new_epoch);
                    page.live_count.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
            // ASAN: mark slot as accessible (was poisoned on free)
            unsafe { asan_unpoison_slab_slot(ptr, self.slot_size); }
            return ptr;
        }

        // Try bump-allocating from the current page
        let page_ptr = self.current_page.load(Ordering::Acquire);
        if !page_ptr.is_null() {
            let page = unsafe { &*page_ptr };
            if let Some((ptr, _idx)) = page.bump_alloc(self.slot_size) {
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
        // Double-check: another thread may have added a page while we waited
        if let Some(last) = pages.last() {
            if let Some((ptr, _idx)) = last.bump_alloc(self.slot_size) {
                // NOTE: page.live_count already incremented inside bump_alloc()
                return ptr;
            }
        }
        let page = Box::new(ValuePage::new(self.slot_size));
        let (ptr, _idx) = page.bump_alloc(self.slot_size)
            .expect("fresh page should have room");
        // NOTE: page.live_count already incremented inside bump_alloc()
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
            // Read next BEFORE any munmap — slot memory is still mapped here
            let next = unsafe { (*(ptr as *const FreeNode)).next };

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

        // Phase 6: Update current_page if all pages were released
        if pages.is_empty() {
            self.current_page.store(std::ptr::null_mut(), Ordering::Release);
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
            // Zero the slot to eliminate stale padding bytes
            std::ptr::write_bytes(ptr, 0, self.values.slot_size);
            // Write the value
            std::ptr::write(ptr as *mut MettaValueInner, val);
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
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, bytes.len()))
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
        let byte_len = len * std::mem::size_of::<MettaValue>();
        let ptr = self.alloc_data(byte_len);
        unsafe {
            let slot = ptr as *mut MettaValue;
            for (i, item) in items.into_iter().enumerate() {
                std::ptr::write(slot.add(i), item);
            }
            std::slice::from_raw_parts(slot as *const MettaValue, len)
        }
    }

    /// Allocate a slice of MettaValues from an existing slice (copy).
    pub fn alloc_slice_copy(&self, items: &[MettaValue]) -> &'static [MettaValue] {
        if items.is_empty() {
            return &[];
        }
        let byte_len = items.len() * std::mem::size_of::<MettaValue>();
        let ptr = self.alloc_data(byte_len);
        unsafe {
            let slot = ptr as *mut MettaValue;
            std::ptr::copy_nonoverlapping(items.as_ptr(), slot, items.len());
            std::slice::from_raw_parts(slot as *const MettaValue, items.len())
        }
    }

    /// Allocate variable-length data. Selects the appropriate size class
    /// or falls back to system allocator for large data.
    fn alloc_data(&self, size: usize) -> *mut u8 {
        if size == 0 {
            return SLOT_ALIGN as *mut u8;
        }
        // Find the smallest size class that fits
        for (i, &class_size) in DATA_SIZE_CLASSES.iter().enumerate() {
            if size <= class_size {
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
    fn free_data_slot(&self, ptr: *mut u8, size: usize) {
        if size == 0 {
            return;
        }
        for (i, &class_size) in DATA_SIZE_CLASSES.iter().enumerate() {
            if size <= class_size {
                self.data_classes[i].free(ptr);
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

/// Global GC thread singleton. Lazily spawned on first use.
/// Wrapped in `Mutex` because `GcThread` contains `mpsc::Receiver` which is `!Sync`.
static GLOBAL_GC_THREAD: OnceLock<Mutex<super::gc_thread::GcThread>> = OnceLock::new();

/// Flag set by the cron manager or allocation pressure to request a GC cycle.
/// Checked by `maybe_gc()` in the trampoline loop (every 256 iterations).
/// `pub(crate)` for test observability (clearing between tests).
pub(crate) static GC_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Get the global GC thread (locked), spawning it if needed.
pub fn global_gc_thread() -> &'static Mutex<super::gc_thread::GcThread> {
    GLOBAL_GC_THREAD.get_or_init(|| Mutex::new(super::gc_thread::GcThread::spawn()))
}

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
static ACTIVE_EVALUATORS: AtomicU32 = AtomicU32::new(0);

/// Set by `maybe_quiescent_gc()` during snapshot building (sub-millisecond).
/// `EvalGuard::enter()` parks on condvar until this is false.
/// NOT stop-the-world: only guards the brief snapshot capture, not mark-sweep.
static GC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Mutex + Condvar pair for parking evaluator threads while GC snapshot is in
/// progress. The mutex protects against lost wakeups: `GcInProgressGuard::drop()`
/// holds the mutex when clearing `GC_IN_PROGRESS`, ensuring threads that checked
/// the flag and are about to `wait()` cannot miss the notification.
static GC_PROGRESS_MUTEX: Mutex<()> = Mutex::new(());
static GC_PROGRESS_CONDVAR: Condvar = Condvar::new();

/// Set when a GC snapshot is sent to the GC thread, cleared when the response
/// is processed. Prevents queueing multiple snapshots in the mpsc channel.
///
/// Aligns with TLA+ model's `~hasGcRequest /\ gcPhase = "idle"` preconditions
/// on `TryQuiescentGc_AcquireFlag`. Without this, `maybe_quiescent_gc()` could
/// send multiple snapshots while the GC thread is still processing a previous
/// one, wasting memory and CPU on redundant GC cycles.
static GC_CYCLE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

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
    #[inline]
    pub fn enter() -> Self {
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
                GC_PROGRESS_CONDVAR.wait(&mut lock);
            }
            drop(lock);
        }
        EvalGuard
    }
}

impl Drop for EvalGuard {
    #[inline]
    fn drop(&mut self) {
        ACTIVE_EVALUATORS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Get the current active evaluator count (for testing and diagnostics).
pub fn active_evaluator_count() -> u32 {
    ACTIVE_EVALUATORS.load(Ordering::Acquire)
}

/// RAII guard that sets `GC_IN_PROGRESS = true` on creation and clears it on drop.
/// Ensures the flag is always cleared, even if the GC snapshot path panics.
struct GcInProgressGuard;

impl GcInProgressGuard {
    fn enter() -> Self {
        GC_IN_PROGRESS.store(true, Ordering::Release);
        GcInProgressGuard
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
        1 => std::thread::yield_now(),
        2 => std::thread::sleep(std::time::Duration::from_micros(10)),
        _ => std::thread::sleep(std::time::Duration::from_micros(100)),
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
            GC_CYCLE_CONDVAR.wait_for(&mut lock, std::time::Duration::from_millis(100));
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
    // RAII guard ensures the flag is always cleared, even on panic.
    let _gc_guard = GcInProgressGuard::enter();

    // Double-check no eval snuck in between our check and the flag set
    if ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0 {
        drop(_gc_guard);
        // Re-set GC_REQUESTED so we try again at the next quiescent point
        GC_REQUESTED.store(true, Ordering::Release);
        return false;
    }

    // Safe: no evaluators active, build snapshot and trigger GC.
    let gc = global_gc_thread().lock();
    let result = trigger_gc_cycle_locked(&gc);

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
/// Returns `true` if a GC response was processed.
pub fn maybe_process_gc_response() -> bool {
    // Signal that the GC lifecycle is reachable (for cron backpressure gating)
    bump_gc_reachable();

    // Lazily spawn the GC cron manager (idempotent via OnceLock)
    let _ = global_gc_cron();

    let gc = global_gc_thread().lock();
    let alloc = global_allocator();

    match gc.try_recv_response() {
        super::gc_thread::TryRecvGcResponse::Response(response) => {
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
        super::gc_thread::TryRecvGcResponse::Disconnected => {
            // GC thread crashed or shut down — clear in-flight flag to prevent
            // indefinite blocking in apply_backpressure_tier2().
            if gc_cycle_in_flight() {
                GC_CYCLE_IN_FLIGHT.store(false, Ordering::Release);
                // Wake blocked threads since in-flight was cleared
                GC_CYCLE_CONDVAR.notify_all();
                set_backpressure_level(0);
            }
        }
        super::gc_thread::TryRecvGcResponse::Empty => {}
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
pub fn collect_all_roots() -> Vec<MettaValue> {
    let mut registry = root_registry().write();
    let mut roots = Vec::with_capacity(registry.len() * 64); // heuristic pre-alloc
    registry.retain(|weak| {
        if let Some(strong) = weak.upgrade() {
            strong.collect_roots(&mut roots);
            true
        } else {
            false // Provider was dropped — remove from registry
        }
    });
    roots
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
    // Skip registration when GC is disabled or the GC thread hasn't been spawned.
    // This avoids ROOT_REGISTRY write lock contention when many environments are
    // created in parallel (e.g., test suites with thousands of env creations).
    if is_gc_disabled() || GLOBAL_GC_THREAD.get().is_none() {
        return;
    }

    use std::any::Any;
    // Clone the Arc and try to downcast to the concrete MettaValue type
    let any: Arc<dyn Any + Send + Sync> = shared.clone();
    if let Ok(arena_shared) = any.downcast::<crate::backend::environment::GenericEnvironmentShared<MettaValue>>() {
        // GenericEnvironmentShared<MettaValue> implements RootProvider
        let provider: Arc<dyn RootProvider> = arena_shared;
        register_root_provider(&provider);
    }
}

/// Trigger a GC cycle using the global allocator and root registry.
///
/// This is the primary GC entry point. It:
/// 1. Processes any pending GC response from a previous cycle
/// 2. Collects roots from all registered providers
/// 3. Builds a snapshot from the global allocator
/// 4. Sends the snapshot to the GC thread
///
/// Returns `true` if a GC cycle was initiated.
pub fn trigger_gc_cycle(gc_thread: &super::gc_thread::GcThread) -> bool {
    let alloc = global_allocator();

    // First, process any pending GC response from previous cycle
    match gc_thread.try_recv_response() {
        super::gc_thread::TryRecvGcResponse::Response(response) => {
            alloc.process_gc_response(&response);
            // Page release is handled inside process_gc_response() (Phase 5).
            GC_CYCLE_IN_FLIGHT.store(false, Ordering::Release);
        }
        super::gc_thread::TryRecvGcResponse::Disconnected => {
            // GC thread crashed or shut down — can't trigger GC.
            GC_CYCLE_IN_FLIGHT.store(false, Ordering::Release);
            set_backpressure_level(0);
            return false;
        }
        super::gc_thread::TryRecvGcResponse::Empty => {}
    }

    // Don't queue another cycle if one is already in flight
    if GC_CYCLE_IN_FLIGHT.load(Ordering::Acquire) {
        return false;
    }

    // Collect roots from all registered providers
    let roots = collect_all_roots();

    // Build snapshot and send to GC thread
    let snapshot = alloc.build_snapshot(roots);
    GC_CYCLE_IN_FLIGHT.store(true, Ordering::Release);
    gc_thread.request_gc(snapshot);
    true
}

/// Same as `trigger_gc_cycle` but takes an already-locked GcThread mutex guard.
/// Used by `maybe_quiescent_gc()` to avoid double-locking.
///
/// Sets `GC_CYCLE_IN_FLIGHT` before sending the snapshot to prevent queueing
/// multiple snapshots. Aligns with TLA+ `hasGcRequest' = TRUE` in
/// `TryQuiescentGc_SnapshotOK`.
fn trigger_gc_cycle_locked(gc_thread: &super::gc_thread::GcThread) -> bool {
    let alloc = global_allocator();
    let roots = collect_all_roots();
    let snapshot = alloc.build_snapshot(roots);
    GC_CYCLE_IN_FLIGHT.store(true, Ordering::Release);
    gc_thread.request_gc(snapshot);
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
}

/// Immutable snapshot sent to the GC thread for mark-sweep collection.
pub struct GcSnapshot {
    pub page_snapshots: Vec<PageSnapshot>,
    pub slot_size: usize,
    pub free_set: std::collections::HashSet<*const u8>,
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
    pub dead_data: Vec<(*mut u8, usize)>,
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
            PageSnapshot {
                data_ptr: page.data.as_ptr(),
                bump_count: page.bump_count.load(Ordering::Acquire),
                capacity: page.capacity,
            }
        }).collect();

        // Build free set by scanning Treiber stack
        // Since we can't iterate a lock-free stack safely, we build the free set
        // from the value allocator's pages by checking which slots have been freed.
        // For correctness, we use a simpler approach: mark all slots that are
        // bump-allocated but have zero live_count contribution.
        // Actually, the simplest correct approach is an empty free set — the sweep
        // will just treat free-list slots as "unmarked allocated" = dead, which is
        // fine since they're already dead. The only issue is double-freeing, which
        // we prevent via epoch filtering.
        let free_set = std::collections::HashSet::new();

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
        let mut filtered_count = 0usize;
        let mut non_filtered_dead: Vec<ResolvedDead> = Vec::with_capacity(response.dead_values.len());

        // Declare outside the block so it outlives the read lock.
        // It's an owned Vec<(*mut u8, usize)> — no borrows on pages.
        let dead_data_to_free: Vec<(*mut u8, usize)>;

        // Single read lock across Phases 1-3. Safe because:
        // - No pages added (alloc_new_page takes write lock)
        // - No pages removed (release_empty_pages is Phase 5, after this block)
        // - collect_dead_data (Phase 2) only reads slot content in mmap pages
        // - free_list.push (Phase 3) is lock-free Treiber stack, doesn't touch pages
        {
            let pages = self.values.pages.read();
            let page_index = PageIndex::new(&pages);

            // === Phase 1: Epoch filtering — O(D log P) ===
            // Separate dead values into filtered (re-allocated after snapshot) and
            // non-filtered (genuinely dead, safe to reclaim).
            for &ptr in &response.dead_values {
                if let Some((page_idx, slot_idx)) = page_index.find(
                    &pages, ptr as *const u8, self.values.slot_size,
                ) {
                    if pages[page_idx].slot_epoch(slot_idx) > response.snapshot_epoch {
                        filtered_count += 1;
                    } else {
                        non_filtered_dead.push(ResolvedDead { ptr, page_idx, slot_idx });
                    }
                }
                // else: ptr not in any page (released in prior cycle) — skip
            }

            // === Phase 2: Collect dead data BEFORE freeing value slots ===
            // The slot content is still valid here — we haven't pushed to the free list yet.
            // This fixes a bug in the previous code where the filtered-values branch read
            // from slots that had already been pushed to the free list (use-after-free).
            dead_data_to_free = if filtered_count == 0 {
                // No filtering needed — use the pre-computed dead_data from the GC response
                response.dead_data.clone()
            } else {
                // Filtering needed — re-derive dead data from non-filtered values only
                let mut data = Vec::new();
                for entry in &non_filtered_dead {
                    let inner_val = unsafe { &*(entry.ptr as *const MettaValueInner) };
                    let mut data_entries = Vec::new();
                    collect_dead_data(inner_val, &mut data_entries);
                    data.extend(data_entries);
                }
                data
            };

            // === Phase 3: Free value slots — O(D'), zero page lookups ===
            for entry in &non_filtered_dead {
                let page = &pages[entry.page_idx];
                page.live_count.fetch_sub(1, Ordering::Relaxed);
                // Sentinel epoch: prevents double-free by future GC cycles.
                page.set_slot_epoch(entry.slot_idx, u64::MAX);
                // ASAN: poison the freed slot BEFORE push (skip FreeNode header).
                unsafe { asan_poison_slab_slot(entry.ptr, self.values.slot_size); }
                self.values.free_list.push(entry.ptr);
            }
        } // read lock released

        // === Phase 4: Free dead data slots — O(D_data log P_data) ===
        // Batch by size class, build sorted page index once per class.
        self.free_data_slots_batch(dead_data_to_free);

        // === Phase 5: Release empty pages ===
        // Safe: free-list entries from released pages are filtered out via
        // atomic drain + rebuild in release_empty_pages().
        self.values.release_empty_pages();
        for dc in &self.data_classes {
            dc.release_empty_pages();
        }

        // Update committed bytes
        self.committed_bytes_atomic.store(self.committed_bytes(), Ordering::Relaxed);
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
    pub fn release_empty_pages(&self) {
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
            MettaValueInner::Type(inner) => {
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
fn snapshot_mark_value(snapshot: &mut GcSnapshot, ptr: *const u8, slot_size: usize) -> bool {
    for (page_idx, ps) in snapshot.page_snapshots.iter().enumerate() {
        let offset = (ptr as usize).wrapping_sub(ps.data_ptr as usize);
        if offset < ps.capacity * slot_size {
            let idx = offset / slot_size;
            if idx < ps.bump_count {
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
        dead_data: Vec::new(),
        snapshot_epoch: snapshot.snapshot_epoch,
        live_bytes: 0,
        live_values: 0,
    };

    for (page_idx, ps) in snapshot.page_snapshots.iter().enumerate() {
        for slot_idx in 0..ps.bump_count {
            let ptr = unsafe { ps.data_ptr.add(slot_idx * slot_size) as *mut u8 };

            if snapshot.free_set.contains(&(ptr as *const u8)) {
                continue;
            }

            if snapshot_is_marked(snapshot, page_idx, slot_idx) {
                response.live_values += 1;
                response.live_bytes += slot_size;
                let inner_val = unsafe { &*(ptr as *const MettaValueInner) };
                response.live_bytes += data_size_of(inner_val);
            } else {
                response.dead_values.push(ptr);
                let inner_val = unsafe { &*(ptr as *const MettaValueInner) };
                collect_dead_data(inner_val, &mut response.dead_data);
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
            MettaValueInner::Type(inner) => {
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
        MettaValueInner::SExpr(children) => children.len() * std::mem::size_of::<MettaValue>(),
        MettaValueInner::Conjunction(goals) => goals.len() * std::mem::size_of::<MettaValue>(),
        MettaValueInner::Error(msg, _) => msg.len(),
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
            let byte_len = children.len() * std::mem::size_of::<MettaValue>();
            dead_data.push((children.as_ptr() as *mut u8, byte_len));
        }
        MettaValueInner::Conjunction(goals) if !goals.is_empty() => {
            let byte_len = goals.len() * std::mem::size_of::<MettaValue>();
            dead_data.push((goals.as_ptr() as *mut u8, byte_len));
        }
        MettaValueInner::Error(msg, _) if !msg.is_empty() => {
            dead_data.push((msg.as_ptr() as *mut u8, msg.len()));
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
        let s = self.alloc.alloc_str(s);
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Atom(s)))
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
        let slice = self.alloc.alloc_slice_from_iter(items);
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::SExpr(slice)))
    }

    #[inline]
    fn sexpr_from_slice(&self, items: &[MettaValue]) -> MettaValue {
        if items.is_empty() {
            return self.unit();
        }
        let slice = self.alloc.alloc_slice_copy(items);
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::SExpr(slice)))
    }

    #[inline]
    fn error(&self, msg: &str, details: MettaValue) -> MettaValue {
        let msg = self.alloc.alloc_str(msg);
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Error(msg, details)))
    }

    #[inline]
    fn type_value(&self, inner: MettaValue) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Type(inner)))
    }

    #[inline]
    fn conjunction(&self, goals: Vec<MettaValue>) -> MettaValue {
        let slice = self.alloc.alloc_slice_from_iter(goals);
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Conjunction(slice)))
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
    fn empty(&self) -> MettaValue {
        MettaValue::from_inner(self.alloc.alloc_value(MettaValueInner::Empty))
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
    use super::metta_value::serialize_tags::*;
    use super::metta_value::read_varint;
    use super::metta_value_trait::MettaValueFactory;

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
    use super::*;

    #[test]
    fn test_slab_allocator_creation() {
        let alloc = SlabAllocator::new();
        assert_eq!(alloc.committed_bytes(), 0);
    }

    #[test]
    fn test_value_slot_size() {
        let alloc = SlabAllocator::new();
        let slot_size = alloc.value_slot_size();
        assert!(slot_size >= std::mem::size_of::<MettaValueInner>());
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

    use super::super::metta_value_trait::MettaValueFactory;

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
            std::thread::spawn(move || {
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
}
