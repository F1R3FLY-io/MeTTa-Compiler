//! Custom Slab Allocator for GC-Managed MeTTa Values
//!
//! This module provides a session-scoped slab allocator that replaces the dual
//! bumpalo arena model. Instead of deep-copying values between eval and storage
//! arenas after each expression, all values live in a single allocator and a
//! mark-sweep GC reclaims dead values between expressions.
//!
//! ## Design
//!
//! - **Fixed-size value slots**: All `ArenaValueInner` instances are the same size
//!   (Rust enum = largest variant). One slab with uniform slots.
//! - **Power-of-2 data classes**: Variable-length data (strings, slices) allocated
//!   in size-class buckets (8, 16, 32, ... 4096 bytes).
//! - **Free-list + bump**: Freed slots go to a free list; fresh allocations bump.
//! - **Cell-based interior mutability**: `alloc(&self, ...)` (shared ref) for
//!   ergonomic factory integration — same pattern as bumpalo.
//!
//! ## Memory Safety
//!
//! All allocations are tied to the `SlabAllocator`'s lifetime. The `'static`
//! lifetime on `ArenaValue<'static>` is a lie for ergonomics (same as the
//! current bumpalo approach). Values must not be accessed after the allocator
//! (ArenaState) is dropped.
//!
//! ## Thread Safety
//!
//! The allocator is **not thread-safe** — it is session-owned and accessed from
//! a single thread at a time. The `unsafe impl Send/Sync` on the factory is the
//! same safety invariant as the current `StorageFactory`.

use std::alloc::Layout;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::arena_value::{ArenaValue, ArenaValueInner};

// ============================================================================
// Constants
// ============================================================================

/// Page size for value and data slabs (64 KB).
const PAGE_SIZE: usize = 64 * 1024;

/// Power-of-2 size classes for variable-length data.
const DATA_SIZE_CLASSES: [usize; 10] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

/// Alignment for all slots (16 bytes for SIMD-friendly access).
const SLOT_ALIGN: usize = 16;

// ============================================================================
// ValuePage — Fixed-Size Slots for ArenaValueInner
// ============================================================================

/// A page of fixed-size slots for `ArenaValueInner` values.
///
/// Each page is a contiguous 64 KB block divided into uniform slots.
/// Allocation state is tracked by a bump offset and a mark bitmap
/// for GC support.
struct ValuePage {
    /// Raw page memory. Slots are laid out contiguously.
    data: Box<[u8]>,
    /// Number of slots that have been bump-allocated in this page.
    bump_count: usize,
    /// Maximum slots this page can hold.
    capacity: usize,
    /// Number of live slots (bumped minus freed). When 0, page can be released.
    live_count: usize,
    /// GC mark bitmap: bit i = 1 means slot i is marked (reachable).
    /// Sized to capacity bits, packed into u64 words.
    marks: Vec<u64>,
}

impl ValuePage {
    /// Create a new value page for the given slot size.
    fn new(slot_size: usize) -> Self {
        let capacity = PAGE_SIZE / slot_size;
        let mark_words = (capacity + 63) / 64; // ceil(capacity / 64)
        // Allocate zeroed memory for the page
        let data = vec![0u8; PAGE_SIZE].into_boxed_slice();
        Self {
            data,
            bump_count: 0,
            capacity,
            live_count: 0,
            marks: vec![0u64; mark_words],
        }
    }

    /// Get pointer to slot at the given index.
    #[inline]
    fn slot_ptr(&self, idx: usize, slot_size: usize) -> *mut u8 {
        debug_assert!(idx < self.capacity);
        unsafe { self.data.as_ptr().add(idx * slot_size) as *mut u8 }
    }

    /// Try to bump-allocate the next slot. Returns None if page is full.
    #[inline]
    fn bump_alloc(&mut self, slot_size: usize) -> Option<*mut u8> {
        if self.bump_count < self.capacity {
            let ptr = self.slot_ptr(self.bump_count, slot_size);
            self.bump_count += 1;
            self.live_count += 1;
            Some(ptr)
        } else {
            None
        }
    }

    /// Check if this page contains the given pointer.
    #[inline]
    fn contains(&self, ptr: *const u8) -> bool {
        let start = self.data.as_ptr();
        let end = unsafe { start.add(self.data.len()) };
        let ptr = ptr as *const u8;
        ptr >= start && ptr < end
    }

    /// Compute the slot index for a pointer within this page.
    /// Returns None if the pointer is not in this page.
    #[inline]
    fn slot_index(&self, ptr: *const u8, slot_size: usize) -> Option<usize> {
        let start = self.data.as_ptr();
        let offset = (ptr as usize).wrapping_sub(start as usize);
        if offset < self.data.len() {
            let idx = offset / slot_size;
            if idx < self.bump_count {
                Some(idx)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Set the mark bit for a slot.
    #[inline]
    fn set_mark(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        self.marks[word] |= 1u64 << bit;
    }

    /// Check if a slot is marked.
    #[inline]
    fn is_marked(&self, idx: usize) -> bool {
        let word = idx / 64;
        let bit = idx % 64;
        (self.marks[word] & (1u64 << bit)) != 0
    }

    /// Clear all mark bits.
    #[inline]
    fn clear_marks(&mut self) {
        for word in &mut self.marks {
            *word = 0;
        }
    }
}

// ============================================================================
// DataPage — Variable-Length Data Slots
// ============================================================================

/// A page of same-sized data slots for one size class.
struct DataPage {
    data: Box<[u8]>,
    bump_count: usize,
    capacity: usize,
    /// Number of live slots (bumped minus freed). When 0, page can be released.
    live_count: usize,
}

impl DataPage {
    fn new(slot_size: usize) -> Self {
        let capacity = PAGE_SIZE / slot_size;
        let data = vec![0u8; PAGE_SIZE].into_boxed_slice();
        Self {
            data,
            bump_count: 0,
            capacity,
            live_count: 0,
        }
    }

    #[inline]
    fn slot_ptr(&self, idx: usize, slot_size: usize) -> *mut u8 {
        debug_assert!(idx < self.capacity);
        unsafe { self.data.as_ptr().add(idx * slot_size) as *mut u8 }
    }

    #[inline]
    fn bump_alloc(&mut self, slot_size: usize) -> Option<*mut u8> {
        if self.bump_count < self.capacity {
            let ptr = self.slot_ptr(self.bump_count, slot_size);
            self.bump_count += 1;
            self.live_count += 1;
            Some(ptr)
        } else {
            None
        }
    }

    /// Check if this page contains the given pointer.
    #[inline]
    fn contains(&self, ptr: *const u8) -> bool {
        let start = self.data.as_ptr();
        let end = unsafe { start.add(self.data.len()) };
        ptr >= start && ptr < end
    }
}

// ============================================================================
// DataClassAllocator — Per-Size-Class Allocator
// ============================================================================

/// Allocator for one power-of-2 data size class.
struct DataClassAllocator {
    slot_size: usize,
    pages: Vec<DataPage>,
    free_list: Vec<*mut u8>,
    total_allocated: usize,
    /// Number of slots currently live (allocated - freed). Used for GC pressure.
    live_count: usize,
}

impl DataClassAllocator {
    fn new(slot_size: usize) -> Self {
        Self {
            slot_size,
            pages: Vec::new(),
            free_list: Vec::new(),
            total_allocated: 0,
            live_count: 0,
        }
    }

    /// Allocate a slot of this size class.
    fn alloc(&mut self) -> *mut u8 {
        self.total_allocated += 1;
        self.live_count += 1;

        // Fast path: pop from free list
        if let Some(ptr) = self.free_list.pop() {
            // Increment page live_count — slot is alive again.
            // Without this, release_empty_pages() could drop a page
            // that still contains live values (Bug 4: page-level UAF).
            for page in &mut self.pages {
                if page.contains(ptr as *const u8) {
                    page.live_count += 1;
                    break;
                }
            }
            return ptr;
        }

        // Try bump-allocating from the last page
        if let Some(page) = self.pages.last_mut() {
            if let Some(ptr) = page.bump_alloc(self.slot_size) {
                return ptr;
            }
        }

        // Need a new page
        let mut page = DataPage::new(self.slot_size);
        let ptr = page.bump_alloc(self.slot_size)
            .expect("fresh page should have room");
        self.pages.push(page);
        ptr
    }

    /// Return a slot to the free list and decrement the page's live count.
    fn free(&mut self, ptr: *mut u8) {
        self.free_list.push(ptr);
        self.live_count = self.live_count.saturating_sub(1);

        // Decrement the page's live_count
        for page in &mut self.pages {
            if page.contains(ptr as *const u8) {
                page.live_count = page.live_count.saturating_sub(1);
                break;
            }
        }
    }

    /// Total bytes committed by this size class (all pages, regardless of occupancy).
    fn committed_bytes(&self) -> usize {
        self.pages.len() * PAGE_SIZE
    }

    /// Live bytes in this size class (allocated - freed).
    fn live_bytes(&self) -> usize {
        self.live_count * self.slot_size
    }

    /// Release empty pages (live_count == 0) and remove stale free list entries.
    ///
    /// Called after processing a GC dead set. Drops pages where all slots have
    /// been freed, reducing RSS.
    fn release_empty_pages(&mut self) {
        let empty_indices: Vec<usize> = self.pages.iter().enumerate()
            .filter(|(_, page)| page.live_count == 0 && page.bump_count > 0)
            .map(|(i, _)| i)
            .collect();

        if empty_indices.is_empty() {
            return;
        }

        // Remove free list entries that point into empty pages
        let empty_pages: Vec<&DataPage> = empty_indices.iter()
            .map(|&i| &self.pages[i])
            .collect();

        self.free_list.retain(|&ptr| {
            !empty_pages.iter().any(|page| page.contains(ptr as *const u8))
        });

        // Remove empty pages in reverse order to preserve indices
        for &idx in empty_indices.iter().rev() {
            self.pages.swap_remove(idx);
        }
    }
}

// ============================================================================
// ValueAllocator — Fixed-Size Value Slot Manager
// ============================================================================

/// Allocator for fixed-size `ArenaValueInner` slots.
struct ValueAllocator {
    /// Size of each slot (aligned to SLOT_ALIGN).
    slot_size: usize,
    /// All allocated pages.
    pages: Vec<ValuePage>,
    /// Free list of reclaimed slot pointers.
    free_list: Vec<*mut u8>,
    /// Total number of slots allocated (lifetime counter).
    total_allocated: usize,
    /// Number of slots currently live (allocated - freed). Used for GC pressure.
    live_count: usize,
    /// Monotonic epoch counter. Incremented on each free-list re-allocation.
    /// Used for TOCTOU prevention: GC snapshots record the epoch, and dead
    /// set processing filters out slots re-allocated after the snapshot.
    epoch: u64,
    /// Maps slot pointers to the epoch of their last free-list allocation.
    /// Only populated for slots re-allocated from the free list (not bump allocs).
    /// Used to filter stale GC dead sets: if slot_epochs[ptr] > snapshot_epoch,
    /// the slot was re-allocated after the snapshot and must not be freed.
    slot_epochs: HashMap<*mut u8, u64>,
}

impl ValueAllocator {
    fn new() -> Self {
        let raw_size = std::mem::size_of::<ArenaValueInner<'static>>();
        // Round up to SLOT_ALIGN
        let slot_size = (raw_size + SLOT_ALIGN - 1) & !(SLOT_ALIGN - 1);
        Self {
            slot_size,
            pages: Vec::new(),
            free_list: Vec::new(),
            total_allocated: 0,
            live_count: 0,
            epoch: 0,
            slot_epochs: HashMap::new(),
        }
    }

    /// Allocate a value slot.
    /// Free-list allocations increment the epoch and tag the slot for
    /// TOCTOU prevention. Bump allocations don't need epoch tagging
    /// (they are always newer than any snapshot).
    fn alloc(&mut self) -> *mut u8 {
        self.total_allocated += 1;
        self.live_count += 1;

        // Fast path: pop from free list
        if let Some(ptr) = self.free_list.pop() {
            // EPOCH: increment and tag the re-allocated slot
            self.epoch += 1;
            self.slot_epochs.insert(ptr, self.epoch);
            // Increment page live_count — slot is alive again.
            // Without this, release_empty_pages() could drop a page
            // that still contains live values (Bug 4: page-level UAF).
            for page in &mut self.pages {
                if page.contains(ptr as *const u8) {
                    page.live_count += 1;
                    break;
                }
            }
            return ptr;
        }

        // Try bump-allocating from the last page
        if let Some(page) = self.pages.last_mut() {
            if let Some(ptr) = page.bump_alloc(self.slot_size) {
                return ptr;
            }
        }

        // Need a new page
        let mut page = ValuePage::new(self.slot_size);
        let ptr = page.bump_alloc(self.slot_size)
            .expect("fresh page should have room");
        self.pages.push(page);
        ptr
    }

    /// Return a value slot to the free list and decrement the page's live count.
    fn free(&mut self, ptr: *mut u8) {
        self.free_list.push(ptr);
        self.live_count = self.live_count.saturating_sub(1);

        // Decrement the page's live_count
        for page in &mut self.pages {
            if page.contains(ptr as *const u8) {
                page.live_count = page.live_count.saturating_sub(1);
                break;
            }
        }
    }

    /// Check if a pointer belongs to this allocator's pages.
    fn contains(&self, ptr: *const u8) -> bool {
        self.pages.iter().any(|page| page.contains(ptr))
    }

    /// Total bytes committed by value pages.
    fn committed_bytes(&self) -> usize {
        self.pages.len() * PAGE_SIZE
    }

    /// Live bytes in value slots (allocated - freed).
    fn live_bytes(&self) -> usize {
        self.live_count * self.slot_size
    }

    /// Release empty pages (live_count == 0) and remove stale free list entries.
    ///
    /// Called after processing a GC dead set. Drops pages where all slots have
    /// been freed, reducing RSS.
    fn release_empty_pages(&mut self) {
        // Find empty pages
        let empty_indices: Vec<usize> = self.pages.iter().enumerate()
            .filter(|(_, page)| page.live_count == 0 && page.bump_count > 0)
            .map(|(i, _)| i)
            .collect();

        if empty_indices.is_empty() {
            return;
        }

        // Remove free list entries that point into empty pages
        let empty_pages: Vec<&ValuePage> = empty_indices.iter()
            .map(|&i| &self.pages[i])
            .collect();

        self.free_list.retain(|&ptr| {
            !empty_pages.iter().any(|page| page.contains(ptr as *const u8))
        });

        // Remove empty pages in reverse order to preserve indices
        for &idx in empty_indices.iter().rev() {
            self.pages.swap_remove(idx);
        }
    }
}

// ============================================================================
// SlabAllocator — Top-Level Allocator
// ============================================================================

/// Session-scoped slab allocator for GC-managed MeTTa values.
///
/// Provides O(1) allocation for both fixed-size values and variable-length data.
/// Freed slots (from GC sweep) are recycled via free lists. All memory is
/// released when the allocator is dropped.
///
/// ## Interior Mutability
///
/// Uses `Cell`-based interior mutability (via `SlabAllocatorInner`) so that
/// `alloc_*(&self, ...)` methods take shared references. This matches bumpalo's
/// ergonomics and allows the factory to hold `&SlabAllocator`.
pub struct SlabAllocator {
    inner: std::cell::UnsafeCell<SlabAllocatorInner>,
    /// Atomic mirror of committed bytes for cross-thread reads (cron manager).
    /// Updated on page allocation. The eval thread is the sole writer;
    /// the cron thread reads with `Relaxed` ordering (approximate is fine).
    committed_bytes_atomic: Arc<AtomicUsize>,
    /// Atomic allocation counter since last GC cycle for cross-thread reads.
    /// Incremented on each `alloc_value()`. The cron thread reads the delta
    /// between polls to compute allocation rate.
    alloc_count_atomic: Arc<AtomicU64>,
}

/// The actual mutable state of the slab allocator.
struct SlabAllocatorInner {
    /// Allocator for ArenaValueInner (fixed-size slots)
    values: ValueAllocator,
    /// Allocators for variable-length data (strings, slices)
    data_classes: Vec<DataClassAllocator>,
    /// Fallback for data > 4096 bytes (heap allocated)
    large_allocs: Vec<(*mut u8, Layout)>,
    /// GC trigger threshold (bytes)
    gc_threshold: usize,
}

// SAFETY: SlabAllocator is session-owned. Only one thread accesses it at a time.
// The factory holds a reference but only the owning session thread calls alloc.
unsafe impl Send for SlabAllocator {}
unsafe impl Sync for SlabAllocator {}

/// Minimum GC threshold (4 MB).
const MIN_GC_THRESHOLD: usize = 4 * 1024 * 1024;

/// GC growth factor — next threshold = live_bytes * GROWTH_FACTOR.
const GC_GROWTH_FACTOR: f64 = 2.0;

impl SlabAllocator {
    /// Create a new slab allocator.
    pub fn new() -> Self {
        let data_classes = DATA_SIZE_CLASSES
            .iter()
            .map(|&size| DataClassAllocator::new(size))
            .collect();

        Self {
            inner: std::cell::UnsafeCell::new(SlabAllocatorInner {
                values: ValueAllocator::new(),
                data_classes,
                large_allocs: Vec::new(),
                gc_threshold: MIN_GC_THRESHOLD,
            }),
            committed_bytes_atomic: Arc::new(AtomicUsize::new(0)),
            alloc_count_atomic: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get the atomic committed bytes counter (for cron manager).
    ///
    /// Returns an `Arc` clone so the cron thread can read it without
    /// borrowing the allocator.
    #[inline]
    pub fn committed_bytes_atomic(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.committed_bytes_atomic)
    }

    /// Get the atomic allocation count counter (for cron manager).
    #[inline]
    pub fn alloc_count_atomic(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.alloc_count_atomic)
    }

    /// Get mutable reference to inner state.
    ///
    /// # Safety
    /// Caller must ensure no other references to inner exist.
    /// This is guaranteed by session-owned single-thread access.
    #[inline]
    fn inner(&self) -> &mut SlabAllocatorInner {
        unsafe { &mut *self.inner.get() }
    }

    /// Allocate an `ArenaValueInner` and return a reference.
    ///
    /// The reference is valid until this slot is swept by GC or the
    /// allocator is dropped.
    ///
    /// The slot is zero-initialized before writing to ensure that enum padding
    /// bytes are deterministic (zero) rather than containing stale data from a
    /// previous allocation. This eliminates Valgrind "uninitialised value"
    /// false positives when comparing values.
    #[inline]
    pub fn alloc_value<'a>(&self, val: ArenaValueInner<'a>) -> &'a ArenaValueInner<'a> {
        let inner = self.inner();
        let pages_before = inner.values.pages.len();
        let ptr = inner.values.alloc();

        // Update atomic counters for cron manager cross-thread reads.
        self.alloc_count_atomic.fetch_add(1, Ordering::Relaxed);
        // Only update committed bytes when a new page is allocated (avoids
        // per-alloc committed_bytes() recomputation).
        if inner.values.pages.len() != pages_before {
            self.committed_bytes_atomic.store(
                self.committed_bytes_inner(inner),
                Ordering::Relaxed,
            );
        }

        unsafe {
            // Zero the slot to eliminate stale padding bytes from reused slots.
            // Cost: ~1 cache-line write (~80 bytes), sub-nanosecond on modern x86-64.
            std::ptr::write_bytes(ptr, 0, inner.values.slot_size);
            // Write the value into the slot
            std::ptr::write(ptr as *mut ArenaValueInner<'a>, val);
            &*(ptr as *const ArenaValueInner<'a>)
        }
    }

    /// Allocate a string slice, return a reference.
    ///
    /// The string bytes are copied into a data-class slot.
    #[inline]
    pub fn alloc_str<'a>(&self, s: &str) -> &'a str {
        if s.is_empty() {
            return "";
        }

        let inner = self.inner();
        let bytes = s.as_bytes();
        let ptr = inner.alloc_data(bytes.len());

        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, bytes.len()))
        }
    }

    /// Allocate a slice of ArenaValues from an iterator.
    ///
    /// Items are collected into a contiguous data-class slot.
    pub fn alloc_slice_from_iter<'a>(
        &self,
        items: impl IntoIterator<Item = ArenaValue<'a>>,
    ) -> &'a [ArenaValue<'a>] {
        // Collect to know length (most iterators are small)
        let items: Vec<ArenaValue<'a>> = items.into_iter().collect();
        if items.is_empty() {
            return &[];
        }

        let len = items.len();
        let inner = self.inner();
        let byte_len = len * std::mem::size_of::<ArenaValue<'a>>();
        let ptr = inner.alloc_data(byte_len);

        unsafe {
            let slot = ptr as *mut ArenaValue<'a>;
            for (i, item) in items.into_iter().enumerate() {
                std::ptr::write(slot.add(i), item);
            }
            std::slice::from_raw_parts(slot as *const ArenaValue<'a>, len)
        }
    }

    /// Allocate a slice of ArenaValues from an existing slice (copy).
    pub fn alloc_slice_copy<'a>(&self, items: &[ArenaValue<'a>]) -> &'a [ArenaValue<'a>] {
        if items.is_empty() {
            return &[];
        }

        let inner = self.inner();
        let byte_len = items.len() * std::mem::size_of::<ArenaValue<'a>>();
        let ptr = inner.alloc_data(byte_len);

        unsafe {
            let slot = ptr as *mut ArenaValue<'a>;
            std::ptr::copy_nonoverlapping(
                items.as_ptr(),
                slot,
                items.len(),
            );
            std::slice::from_raw_parts(slot as *const ArenaValue<'a>, items.len())
        }
    }

    /// Live bytes across all allocators (allocated - freed).
    ///
    /// This tracks the number of bytes currently in use, decreasing when
    /// GC reclaims dead values. Used for GC threshold decisions.
    pub fn allocated_bytes(&self) -> usize {
        let inner = self.inner();
        let value_bytes = inner.values.live_bytes();
        let data_bytes: usize = inner.data_classes.iter().map(|dc| dc.live_bytes()).sum();
        let large_bytes: usize = inner.large_allocs.iter().map(|(_, l)| l.size()).sum();
        value_bytes + data_bytes + large_bytes
    }

    /// Total committed bytes (all pages, regardless of occupancy).
    ///
    /// This measures the OS-level memory footprint. Pages are never released
    /// back to the OS; only individual slots are recycled via free lists.
    pub fn committed_bytes(&self) -> usize {
        self.committed_bytes_inner(self.inner())
    }

    /// Internal: compute committed bytes from a pre-borrowed inner reference.
    /// Avoids double-borrow when called from `alloc_value()`.
    #[inline]
    fn committed_bytes_inner(&self, inner: &SlabAllocatorInner) -> usize {
        let value_bytes = inner.values.committed_bytes();
        let data_bytes: usize = inner.data_classes.iter().map(|dc| dc.committed_bytes()).sum();
        let large_bytes: usize = inner.large_allocs.iter().map(|(_, l)| l.size()).sum();
        value_bytes + data_bytes + large_bytes
    }

    /// Whether GC should be triggered (allocated bytes exceed threshold).
    pub fn needs_gc(&self) -> bool {
        self.allocated_bytes() >= self.inner().gc_threshold
    }

    /// Get the GC threshold.
    pub fn gc_threshold(&self) -> usize {
        self.inner().gc_threshold
    }

    /// Set the GC threshold.
    pub fn set_gc_threshold(&self, threshold: usize) {
        self.inner().gc_threshold = threshold;
    }

    /// Get the value slot size (useful for testing).
    pub fn value_slot_size(&self) -> usize {
        self.inner().values.slot_size
    }

    /// Get the current epoch counter.
    ///
    /// The epoch is incremented on each free-list re-allocation. Used by
    /// `ArenaState` to record the epoch at GC snapshot time for TOCTOU
    /// prevention when processing dead sets.
    pub fn epoch(&self) -> u64 {
        self.inner().values.epoch
    }

    /// Check if a slot was re-allocated after a given epoch.
    ///
    /// Returns true if the slot has an epoch tag newer than `snapshot_epoch`,
    /// meaning it was re-allocated from the free list after the GC snapshot
    /// was taken and must NOT be freed by the dead set.
    pub fn is_realloc_after_epoch(&self, ptr: *const u8, snapshot_epoch: u64) -> bool {
        let inner = self.inner();
        if let Some(&slot_epoch) = inner.values.slot_epochs.get(&(ptr as *mut u8)) {
            slot_epoch > snapshot_epoch
        } else {
            false
        }
    }

    /// Free a value slot (return to free list).
    ///
    /// # Safety
    /// The caller must ensure the pointer was allocated by this allocator's
    /// value allocator and is no longer referenced.
    pub unsafe fn free_value(&self, ptr: *mut u8) {
        self.inner().values.free(ptr);
    }

    /// Free a data slot (return to appropriate size-class free list).
    ///
    /// # Safety
    /// The caller must ensure the pointer was allocated by this allocator
    /// and is no longer referenced.
    pub unsafe fn free_data(&self, ptr: *mut u8, size: usize) {
        self.inner().free_data_slot(ptr, size);
    }

    /// Check if a value pointer belongs to this allocator.
    pub fn contains_value(&self, ptr: *const u8) -> bool {
        self.inner().values.contains(ptr)
    }
}

impl Default for SlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SlabAllocator {
    fn drop(&mut self) {
        let inner = self.inner.get_mut();
        // Free large allocations
        for (ptr, layout) in inner.large_allocs.drain(..) {
            unsafe {
                std::alloc::dealloc(ptr, layout);
            }
        }
        // ValuePages and DataPages are Box<[u8]> — dropped automatically
    }
}

impl std::fmt::Debug for SlabAllocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner();
        f.debug_struct("SlabAllocator")
            .field("value_pages", &inner.values.pages.len())
            .field("value_slot_size", &inner.values.slot_size)
            .field("value_free_list", &inner.values.free_list.len())
            .field("allocated_bytes", &self.allocated_bytes())
            .field("gc_threshold", &inner.gc_threshold)
            .finish()
    }
}

// ============================================================================
// SlabAllocatorInner — Private Implementation
// ============================================================================

impl SlabAllocatorInner {
    /// Allocate variable-length data. Selects the appropriate size class
    /// or falls back to system allocator for large data.
    fn alloc_data(&mut self, size: usize) -> *mut u8 {
        if size == 0 {
            // Return a valid non-null aligned pointer for zero-size
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
        self.large_allocs.push((ptr, layout));
        ptr
    }

    /// Return a data slot to the appropriate free list.
    fn free_data_slot(&mut self, ptr: *mut u8, size: usize) {
        if size == 0 {
            return;
        }

        // Find the size class
        for (i, &class_size) in DATA_SIZE_CLASSES.iter().enumerate() {
            if size <= class_size {
                self.data_classes[i].free(ptr);
                return;
            }
        }

        // Large allocation: find and remove from large_allocs, then dealloc
        if let Some(pos) = self.large_allocs.iter().position(|(p, _)| *p == ptr) {
            let (ptr, layout) = self.large_allocs.swap_remove(pos);
            unsafe {
                std::alloc::dealloc(ptr, layout);
            }
        }
    }
}

// ============================================================================
// GC Mark-Sweep Support
// ============================================================================

/// Statistics from a GC sweep pass.
#[derive(Debug, Clone, Default)]
pub struct SweepStats {
    /// Number of value slots freed.
    pub freed_values: usize,
    /// Total bytes freed (values + data).
    pub freed_bytes: usize,
    /// Number of live values after sweep.
    pub live_values: usize,
    /// Live bytes after sweep.
    pub live_bytes: usize,
}

/// Captures the allocation watermark at snapshot time.
///
/// Values allocated at or beyond this watermark are implicitly alive
/// (allocated after the GC snapshot was taken). The GC thread only
/// considers values before the watermark for collection.
#[derive(Debug, Clone, Copy)]
pub struct AllocationWatermark {
    /// Number of value pages at snapshot time.
    pub value_page_count: usize,
    /// Bump count in the last value page at snapshot time.
    pub last_page_bump_count: usize,
}

/// Dead set produced by the GC sweep phase.
///
/// Contains pointers to unreachable values and their associated data.
/// The evaluation thread processes this to return slots to free lists.
#[derive(Debug, Default)]
pub struct DeadSet {
    /// Dead value slot pointers.
    pub dead_values: Vec<*mut u8>,
    /// Dead data slot pointers with their sizes (for size-class routing).
    pub dead_data: Vec<(*mut u8, usize)>,
    /// Live bytes remaining (for threshold update).
    pub live_bytes: usize,
    /// Live value count.
    pub live_values: usize,
}

// SAFETY: DeadSet contains raw pointers but is only processed by the
// owning session thread (same thread that allocated the values).
unsafe impl Send for DeadSet {}

// ============================================================================
// GcSnapshot — Immutable Snapshot for GC Thread
// ============================================================================

/// Frozen snapshot of a single value page, sent to the GC thread.
///
/// Contains a raw pointer to the page's data (stable because `Box<[u8]>` is
/// heap-allocated and pages are never freed while a GC is in flight) and the
/// frozen bump count at snapshot time.
pub struct PageSnapshot {
    /// Pointer to the page's data buffer. Stable: pages are `Box<[u8]>` and
    /// are never freed/moved while a GC cycle is in flight.
    pub data_ptr: *const u8,
    /// Number of slots that were bump-allocated at snapshot time.
    pub bump_count: usize,
    /// Maximum slots this page can hold (for mark bitmap sizing).
    pub capacity: usize,
}

/// Immutable snapshot sent to the GC thread for mark-sweep collection.
///
/// The GC thread **owns** this snapshot entirely. It reads value data through
/// stable page pointers (safe: ArenaValues are immutable after creation) and
/// operates on its own mark bitmaps. The GC thread never touches the live
/// allocator state (`SlabAllocator`), eliminating data races (Bug 2 fix).
///
/// ## Safety
///
/// - `page_snapshots[i].data_ptr` points into `Box<[u8]>` which is heap-stable.
/// - Pages are never freed while a GC is in flight (only after the dead set is
///   processed by the eval thread).
/// - ArenaValues are immutable after creation, so GC reading value data is safe.
pub struct GcSnapshot {
    /// Frozen page snapshots with stable data pointers.
    pub page_snapshots: Vec<PageSnapshot>,
    /// Slot size for index computation (same as `ValueAllocator::slot_size`).
    pub slot_size: usize,
    /// Set of free-list slot pointers at snapshot time (for skip during sweep).
    /// Slots on the free list are already dead — don't double-free them.
    pub free_set: std::collections::HashSet<*const u8>,
    /// Monotonic epoch at snapshot time. Used by the eval thread to filter
    /// the dead set: slots with `slot_epochs[ptr] > snapshot_epoch` were
    /// re-allocated after the snapshot and must not be freed.
    pub snapshot_epoch: u64,
    /// Root values to trace from (env bindings, source, output).
    pub roots: Vec<ArenaValue<'static>>,
    /// GC-owned mark bitmaps (one `Vec<u64>` per page). These are NOT shared
    /// with the allocator — the GC thread exclusively owns them.
    pub marks: Vec<Vec<u64>>,
    /// Total committed bytes at snapshot time (for threshold computation).
    pub total_committed_bytes: usize,
}

// SAFETY: GcSnapshot contains raw pointers but is transferred from the eval
// thread to the GC thread via an mpsc channel. The eval thread does not access
// the snapshot after sending it. The GC thread reads value data through stable
// page pointers (immutable ArenaValues). No concurrent mutation.
unsafe impl Send for GcSnapshot {}

/// Response from the GC thread after completing a mark-sweep cycle.
///
/// Contains the dead set, the snapshot epoch for TOCTOU filtering, and
/// accurate live byte count from a full sweep (Bug 3 fix).
#[derive(Debug)]
pub struct GcResponse {
    /// Dead value slot pointers (unreachable values).
    pub dead_values: Vec<*mut u8>,
    /// Dead data slot pointers with their sizes (for size-class routing).
    pub dead_data: Vec<(*mut u8, usize)>,
    /// Snapshot epoch. The eval thread uses this for TOCTOU filtering:
    /// slots with `slot_epochs[ptr] > snapshot_epoch` were re-allocated
    /// after the snapshot and must NOT be freed.
    pub snapshot_epoch: u64,
    /// Live bytes from a FULL sweep (all committed slots in the snapshot).
    /// This is accurate — no watermark restriction (Bug 3 fix).
    pub live_bytes: usize,
    /// Live value count from the full sweep.
    pub live_values: usize,
}

// SAFETY: GcResponse contains raw pointers but is only processed by the
// owning session thread (same thread that allocated the values).
unsafe impl Send for GcResponse {}

impl SlabAllocator {
    /// Build an immutable snapshot of the current allocator state for the GC thread.
    ///
    /// The snapshot contains stable page data pointers, frozen bump counts,
    /// a copy of the free set, the current epoch, root values, and GC-owned
    /// mark bitmaps. The GC thread operates exclusively on this snapshot,
    /// never touching live allocator state (eliminates Bug 2: data race).
    ///
    /// # Arguments
    /// * `roots` - Root values for the mark phase (env bindings, source, output)
    pub fn build_snapshot(&self, roots: Vec<ArenaValue<'static>>) -> GcSnapshot {
        let inner = self.inner();
        let slot_size = inner.values.slot_size;

        let page_snapshots: Vec<PageSnapshot> = inner.values.pages.iter().map(|page| {
            PageSnapshot {
                data_ptr: page.data.as_ptr(),
                bump_count: page.bump_count,
                capacity: page.capacity,
            }
        }).collect();

        let free_set: std::collections::HashSet<*const u8> = inner.values.free_list.iter()
            .map(|&p| p as *const u8)
            .collect();

        // GC-owned mark bitmaps: one Vec<u64> per page, all zeroed.
        let marks: Vec<Vec<u64>> = page_snapshots.iter().map(|ps| {
            let mark_words = (ps.capacity + 63) / 64;
            vec![0u64; mark_words]
        }).collect();

        GcSnapshot {
            page_snapshots,
            slot_size,
            free_set,
            snapshot_epoch: inner.values.epoch,
            roots,
            marks,
            total_committed_bytes: self.committed_bytes(),
        }
    }

    /// Process a GC response with epoch-based TOCTOU filtering.
    ///
    /// Only frees slots whose epoch is <= the snapshot epoch (i.e., slots
    /// that were NOT re-allocated from the free list after the snapshot).
    /// This eliminates Bug 1 (TOCTOU use-after-free).
    ///
    /// For data slots: a dead value's associated data (strings, slices) is
    /// only freed if the parent value passes the epoch filter. We build a
    /// set of epoch-filtered value pointers, then only free data entries
    /// whose parent value was actually freed.
    ///
    /// After freeing dead slots, releases completely empty pages to reduce RSS.
    pub fn process_gc_response(&self, response: &GcResponse) {
        let inner = self.inner();

        // First pass: determine which value pointers pass the epoch filter.
        // Build a set of value ptrs that were NOT freed (epoch-filtered out).
        let mut filtered_values: std::collections::HashSet<*mut u8> =
            std::collections::HashSet::new();

        for &ptr in &response.dead_values {
            // EPOCH FILTER: skip slots re-allocated after the snapshot
            if let Some(&slot_epoch) = inner.values.slot_epochs.get(&ptr) {
                if slot_epoch > response.snapshot_epoch {
                    filtered_values.insert(ptr);
                    continue; // Re-allocated — DO NOT free
                }
            }
            inner.values.free(ptr);
        }

        // Second pass: free data for values that were actually freed.
        // Dead data entries correspond to dead values. We need to check if
        // each data entry's parent value was epoch-filtered.
        // Strategy: dead_data comes from collect_dead_data which reads the
        // value at each dead_values[i] pointer. The data pointer is derived
        // from the value's contents. If the value was re-allocated (epoch-
        // filtered), the old data pointer is stale and must not be freed.
        // We match data pointers back to their parent values by checking
        // if the data pointer falls within the allocator's data pages.
        // Simpler: we just don't free data for epoch-filtered values.
        // Since sweep_snapshot produces dead_data by iterating dead values
        // in order, we can match by index — but dead_data may have 0 or 1+
        // entries per dead value. Instead, we re-derive dead data from the
        // non-filtered dead values.
        //
        // Simplest correct approach: skip ALL dead_data freeing for epoch-
        // filtered responses if any values were filtered. For the common
        // case (no epoch-filtering), this is a no-op.
        if filtered_values.is_empty() {
            // Fast path: no epoch filtering, free all data
            for &(ptr, size) in &response.dead_data {
                inner.free_data_slot(ptr, size);
            }
        } else {
            // Slow path: re-derive dead data from non-filtered dead values only
            for &ptr in &response.dead_values {
                if filtered_values.contains(&ptr) {
                    continue; // Parent value was epoch-filtered
                }
                // Re-read the dead value to collect its data
                let inner_val = unsafe { &*(ptr as *const ArenaValueInner<'static>) };
                let mut data_entries = Vec::new();
                collect_dead_data(inner_val, &mut data_entries);
                for (data_ptr, size) in data_entries {
                    inner.free_data_slot(data_ptr, size);
                }
            }
        }

        // Release pages where all slots have been freed.
        inner.values.release_empty_pages();
        for dc in &mut inner.data_classes {
            dc.release_empty_pages();
        }

        // Update atomic committed bytes after page release (cron reads this).
        self.committed_bytes_atomic.store(
            self.committed_bytes_inner(inner),
            Ordering::Relaxed,
        );
    }
}

// ============================================================================
// Snapshot-Based Mark-Sweep (for GC thread — operates on GcSnapshot only)
// ============================================================================

/// Mark phase operating on a GcSnapshot. Traces from roots using an explicit worklist.
///
/// Uses the snapshot's GC-owned mark bitmaps. Reads value data through stable
/// page pointers (safe: ArenaValues are immutable after creation). Never touches
/// live allocator state.
pub fn mark_snapshot(snapshot: &mut GcSnapshot) {
    let slot_size = snapshot.slot_size;
    let mut worklist: Vec<*const ArenaValueInner<'static>> = Vec::with_capacity(1024);

    // Clone root pointers to avoid borrowing snapshot immutably and mutably
    let root_ptrs: Vec<*const ArenaValueInner<'static>> = snapshot.roots
        .iter()
        .map(|root| root.inner_ptr())
        .collect();

    // Enqueue all roots
    for ptr in root_ptrs {
        if snapshot_mark_value(snapshot, ptr as *const u8, slot_size) {
            worklist.push(ptr);
        }
    }

    // Trace reachable values
    while let Some(ptr) = worklist.pop() {
        // SAFETY: ptr was allocated by the allocator and is a valid ArenaValueInner.
        // ArenaValues are immutable after creation, so reading is safe.
        match unsafe { &*ptr } {
            ArenaValueInner::SExpr(children) => {
                for child in children.iter() {
                    let child_ptr = child.inner_ptr();
                    if snapshot_mark_value(snapshot, child_ptr as *const u8, slot_size) {
                        worklist.push(child_ptr);
                    }
                }
            }
            ArenaValueInner::Conjunction(goals) => {
                for goal in goals.iter() {
                    let goal_ptr = goal.inner_ptr();
                    if snapshot_mark_value(snapshot, goal_ptr as *const u8, slot_size) {
                        worklist.push(goal_ptr);
                    }
                }
            }
            ArenaValueInner::Error(_, details) => {
                let details_ptr = details.inner_ptr();
                if snapshot_mark_value(snapshot, details_ptr as *const u8, slot_size) {
                    worklist.push(details_ptr);
                }
            }
            ArenaValueInner::Type(inner) => {
                let inner_ptr = inner.inner_ptr();
                if snapshot_mark_value(snapshot, inner_ptr as *const u8, slot_size) {
                    worklist.push(inner_ptr);
                }
            }
            // Leaf nodes: no child ArenaValue references
            ArenaValueInner::Atom(_)
            | ArenaValueInner::Bool(_)
            | ArenaValueInner::Long(_)
            | ArenaValueInner::Float(_)
            | ArenaValueInner::String(_)
            | ArenaValueInner::Unit
            | ArenaValueInner::Empty
            | ArenaValueInner::Space(_)
            | ArenaValueInner::State(_)
            | ArenaValueInner::Memo(_) => {}
        }
    }
}

/// Mark a value pointer in the snapshot's GC-owned mark bitmaps.
///
/// Returns true if newly marked, false if already marked or not found.
fn snapshot_mark_value(snapshot: &mut GcSnapshot, ptr: *const u8, slot_size: usize) -> bool {
    for (page_idx, ps) in snapshot.page_snapshots.iter().enumerate() {
        let offset = (ptr as usize).wrapping_sub(ps.data_ptr as usize);
        // Check if this page contains the pointer
        if offset < ps.capacity * slot_size {
            let idx = offset / slot_size;
            if idx < ps.bump_count {
                let word = idx / 64;
                let bit = idx % 64;
                if (snapshot.marks[page_idx][word] & (1u64 << bit)) != 0 {
                    return false; // Already marked
                }
                snapshot.marks[page_idx][word] |= 1u64 << bit;
                return true; // Newly marked
            }
        }
    }
    false // Not in this allocator
}

/// Check if a slot is marked in the snapshot's GC-owned mark bitmaps.
fn snapshot_is_marked(snapshot: &GcSnapshot, page_idx: usize, slot_idx: usize) -> bool {
    let word = slot_idx / 64;
    let bit = slot_idx % 64;
    (snapshot.marks[page_idx][word] & (1u64 << bit)) != 0
}

/// Sweep phase operating on a GcSnapshot. Iterates ALL committed slots
/// (no watermark restriction — Bug 3 fix) and builds a GcResponse.
///
/// Unreachable allocated slots are dead. Their associated variable-length
/// data (strings, slices) is also added to the response.
///
/// The response includes the snapshot epoch for TOCTOU filtering by the
/// eval thread (Bug 1 fix).
pub fn sweep_snapshot(snapshot: &GcSnapshot) -> GcResponse {
    let slot_size = snapshot.slot_size;
    let mut response = GcResponse {
        dead_values: Vec::new(),
        dead_data: Vec::new(),
        snapshot_epoch: snapshot.snapshot_epoch,
        live_bytes: 0,
        live_values: 0,
    };

    // Sweep ALL committed slots in ALL pages (no watermark!)
    for (page_idx, ps) in snapshot.page_snapshots.iter().enumerate() {
        for slot_idx in 0..ps.bump_count {
            let ptr = unsafe { ps.data_ptr.add(slot_idx * slot_size) as *mut u8 };

            // Skip slots that are on the free set (already dead at snapshot time)
            if snapshot.free_set.contains(&(ptr as *const u8)) {
                continue;
            }

            if snapshot_is_marked(snapshot, page_idx, slot_idx) {
                // Live value
                response.live_values += 1;
                response.live_bytes += slot_size;
                // Also count associated data as live
                let inner_val = unsafe { &*(ptr as *const ArenaValueInner<'static>) };
                response.live_bytes += data_size_of(inner_val);
            } else {
                // Dead value — add to response
                response.dead_values.push(ptr);

                // Also free associated variable-length data
                let inner_val = unsafe { &*(ptr as *const ArenaValueInner<'static>) };
                collect_dead_data(inner_val, &mut response.dead_data);
            }
        }
    }

    response
}

// ============================================================================
// Legacy Mark-Sweep Support (kept for backward compatibility and tests)
// ============================================================================

impl SlabAllocator {
    /// Take a snapshot of the current allocation watermark.
    ///
    /// Values allocated at or beyond this point are implicitly alive
    /// during the GC cycle (they were created after the snapshot).
    pub fn watermark(&self) -> AllocationWatermark {
        let inner = self.inner();
        AllocationWatermark {
            value_page_count: inner.values.pages.len(),
            last_page_bump_count: inner.values.pages.last()
                .map(|p| p.bump_count)
                .unwrap_or(0),
        }
    }

    /// Mark a value pointer as reachable.
    ///
    /// Returns true if newly marked (not previously marked).
    /// Returns false if already marked or not in this allocator.
    pub fn mark_value(&self, ptr: *const u8) -> bool {
        let inner = self.inner();
        let slot_size = inner.values.slot_size;
        for page in &mut inner.values.pages {
            if let Some(idx) = page.slot_index(ptr, slot_size) {
                if page.is_marked(idx) {
                    return false; // Already marked
                }
                page.set_mark(idx);
                return true; // Newly marked
            }
        }
        false // Not in this allocator
    }

    /// Check if a value pointer is marked.
    pub fn is_value_marked(&self, ptr: *const u8) -> bool {
        let inner = self.inner();
        let slot_size = inner.values.slot_size;
        for page in &inner.values.pages {
            if let Some(idx) = page.slot_index(ptr, slot_size) {
                return page.is_marked(idx);
            }
        }
        false
    }

    /// Clear all mark bits across all value pages.
    pub fn clear_marks(&self) {
        let inner = self.inner();
        for page in &mut inner.values.pages {
            page.clear_marks();
        }
    }

    /// Number of live (allocated but not on free list) value slots.
    pub fn live_value_count(&self) -> usize {
        let inner = self.inner();
        let total_bumped: usize = inner.values.pages.iter()
            .map(|p| p.bump_count)
            .sum();
        total_bumped.saturating_sub(inner.values.free_list.len())
    }

    /// Process a dead set: return dead slots to free lists and release empty pages.
    ///
    /// This is called by the evaluation thread between expressions after
    /// receiving a dead set from the GC thread. All pointers in the dead
    /// set become available for reuse. Pages that become completely empty
    /// are released back to the OS to reduce RSS.
    pub fn process_dead_set(&self, dead_set: &DeadSet) {
        let inner = self.inner();
        for &ptr in &dead_set.dead_values {
            inner.values.free(ptr);
        }
        for &(ptr, size) in &dead_set.dead_data {
            inner.free_data_slot(ptr, size);
        }

        // Release pages where all slots have been freed.
        // This reduces RSS by returning memory to the OS.
        inner.values.release_empty_pages();
        for dc in &mut inner.data_classes {
            dc.release_empty_pages();
        }
    }
}

/// Mark phase: trace from roots using an explicit worklist.
///
/// Uses the allocator's page-level mark bitmaps for O(1) per-value
/// mark and check operations. Only traces values before the watermark
/// (values after the watermark are implicitly alive).
///
/// # Arguments
/// * `roots` - Iterator of root ArenaValues (env bindings, source, output)
/// * `alloc` - The slab allocator (for mark bitmap access)
pub fn mark_from_roots(
    roots: impl Iterator<Item = ArenaValue<'static>>,
    alloc: &SlabAllocator,
) {
    let mut worklist: Vec<*const ArenaValueInner<'static>> = Vec::with_capacity(1024);

    // Enqueue all roots
    for root in roots {
        let ptr = root.inner_ptr();
        if alloc.mark_value(ptr as *const u8) {
            worklist.push(ptr);
        }
    }

    // Trace reachable values
    while let Some(ptr) = worklist.pop() {
        // Follow child references
        // SAFETY: ptr was allocated by this allocator and is a valid ArenaValueInner
        match unsafe { &*ptr } {
            ArenaValueInner::SExpr(children) => {
                for child in children.iter() {
                    let child_ptr = child.inner_ptr();
                    if alloc.mark_value(child_ptr as *const u8) {
                        worklist.push(child_ptr);
                    }
                }
            }
            ArenaValueInner::Conjunction(goals) => {
                for goal in goals.iter() {
                    let goal_ptr = goal.inner_ptr();
                    if alloc.mark_value(goal_ptr as *const u8) {
                        worklist.push(goal_ptr);
                    }
                }
            }
            ArenaValueInner::Error(_, details) => {
                let details_ptr = details.inner_ptr();
                if alloc.mark_value(details_ptr as *const u8) {
                    worklist.push(details_ptr);
                }
            }
            ArenaValueInner::Type(inner) => {
                let inner_ptr = inner.inner_ptr();
                if alloc.mark_value(inner_ptr as *const u8) {
                    worklist.push(inner_ptr);
                }
            }
            // Leaf nodes: no child ArenaValue references
            ArenaValueInner::Atom(_)
            | ArenaValueInner::Bool(_)
            | ArenaValueInner::Long(_)
            | ArenaValueInner::Float(_)
            | ArenaValueInner::String(_)
            | ArenaValueInner::Unit
            | ArenaValueInner::Empty
            | ArenaValueInner::Space(_)
            | ArenaValueInner::State(_)
            | ArenaValueInner::Memo(_) => {}
        }
    }
}

/// Sweep phase: iterate all value pages and build a dead set.
///
/// Unmarked allocated slots (before watermark) are dead. Their associated
/// variable-length data (strings, slices) is also added to the dead set.
///
/// # Arguments
/// * `alloc` - The slab allocator (reads mark bitmaps, page data)
/// * `watermark` - Only sweep slots allocated before this watermark
///
/// # Returns
/// A `DeadSet` containing pointers to all unreachable values and data.
pub fn sweep(alloc: &SlabAllocator, watermark: &AllocationWatermark) -> DeadSet {
    let inner = alloc.inner();
    let slot_size = inner.values.slot_size;

    let mut dead = DeadSet::default();
    let mut live_values = 0usize;
    let mut live_bytes = 0usize;

    // Build a set of free-list pointers for fast lookup
    // (slots on the free list are already dead, don't double-free)
    use std::collections::HashSet;
    let free_set: HashSet<*const u8> = inner.values.free_list.iter()
        .map(|&p| p as *const u8)
        .collect();

    for (page_idx, page) in inner.values.pages.iter().enumerate() {
        // Determine how many slots in this page to sweep
        let sweep_limit = if page_idx < watermark.value_page_count.saturating_sub(1) {
            // Pages before the last snapshot page: sweep all bumped slots
            page.bump_count
        } else if page_idx == watermark.value_page_count.saturating_sub(1) {
            // Last snapshot page: only sweep up to watermark
            watermark.last_page_bump_count.min(page.bump_count)
        } else {
            // Pages allocated after watermark: skip entirely (implicitly alive)
            continue;
        };

        for slot_idx in 0..sweep_limit {
            let ptr = page.slot_ptr(slot_idx, slot_size);

            // Skip slots that are on the free list (already dead)
            if free_set.contains(&(ptr as *const u8)) {
                continue;
            }

            if page.is_marked(slot_idx) {
                // Live value
                live_values += 1;
                live_bytes += slot_size;
                // Also count associated data as live
                let inner_val = unsafe { &*(ptr as *const ArenaValueInner<'static>) };
                live_bytes += data_size_of(inner_val);
            } else {
                // Dead value — add to dead set
                dead.dead_values.push(ptr);

                // Also free associated variable-length data
                let inner_val = unsafe { &*(ptr as *const ArenaValueInner<'static>) };
                collect_dead_data(inner_val, &mut dead.dead_data);
            }
        }
    }

    dead.live_values = live_values;
    dead.live_bytes = live_bytes;
    dead
}

/// Compute the variable-length data size associated with a value.
fn data_size_of(inner: &ArenaValueInner<'static>) -> usize {
    match inner {
        ArenaValueInner::Atom(s) => s.len(),
        ArenaValueInner::String(s) => s.len(),
        ArenaValueInner::SExpr(children) => children.len() * std::mem::size_of::<ArenaValue<'static>>(),
        ArenaValueInner::Conjunction(goals) => goals.len() * std::mem::size_of::<ArenaValue<'static>>(),
        ArenaValueInner::Error(msg, _) => msg.len(),
        _ => 0,
    }
}

/// Collect dead data pointers from a dead value for the dead set.
fn collect_dead_data(inner: &ArenaValueInner<'static>, dead_data: &mut Vec<(*mut u8, usize)>) {
    match inner {
        ArenaValueInner::Atom(s) if !s.is_empty() => {
            dead_data.push((s.as_ptr() as *mut u8, s.len()));
        }
        ArenaValueInner::String(s) if !s.is_empty() => {
            dead_data.push((s.as_ptr() as *mut u8, s.len()));
        }
        ArenaValueInner::SExpr(children) if !children.is_empty() => {
            let byte_len = children.len() * std::mem::size_of::<ArenaValue<'static>>();
            dead_data.push((children.as_ptr() as *mut u8, byte_len));
        }
        ArenaValueInner::Conjunction(goals) if !goals.is_empty() => {
            let byte_len = goals.len() * std::mem::size_of::<ArenaValue<'static>>();
            dead_data.push((goals.as_ptr() as *mut u8, byte_len));
        }
        ArenaValueInner::Error(msg, _) if !msg.is_empty() => {
            dead_data.push((msg.as_ptr() as *mut u8, msg.len()));
        }
        _ => {}
    }
}

// ============================================================================
// Tests
// ============================================================================

// ============================================================================
// GcFactory — MettaValueFactory backed by SlabAllocator
// ============================================================================

/// Factory for allocating values in the session's slab allocator.
///
/// Implements `MettaValueFactory<ArenaValue<'static>>` so it can be used as the
/// factory type in `EvalContext` and `GenericEnvironment`.
///
/// Uses interior mutability (via `SlabAllocator`'s `UnsafeCell`) so alloc takes `&self`.
/// The `'static` lifetime on `ArenaValue` is a lie for ergonomics — same as the
/// current bumpalo-based approach.
///
/// ## Safety Invariant
///
/// Same as current system: values must not be accessed after the allocator
/// (ArenaState) is dropped. The `&'static SlabAllocator` reference is obtained
/// via unsafe lifetime extension from `&ArenaState.allocator`.
#[derive(Clone, Copy)]
pub struct GcFactory {
    alloc: &'static SlabAllocator,
}

// SAFETY: Same invariant as current StorageFactory — allocator is session-scoped
// and only accessed from one thread at a time.
unsafe impl Send for GcFactory {}
unsafe impl Sync for GcFactory {}

impl GcFactory {
    /// Create a new GcFactory for the given slab allocator.
    ///
    /// # Safety
    /// The caller must ensure the allocator outlives all values created by this factory.
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

impl super::metta_value_trait::MettaValueFactory<ArenaValue<'static>> for GcFactory {
    #[inline]
    fn atom(&self, s: &str) -> ArenaValue<'static> {
        let s: &'static str = unsafe { std::mem::transmute(self.alloc.alloc_str(s)) };
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Atom(s)))
        })
    }

    #[inline]
    fn bool(&self, b: bool) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Bool(b)))
        })
    }

    #[inline]
    fn long(&self, n: i64) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Long(n)))
        })
    }

    #[inline]
    fn float(&self, f: f64) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Float(f)))
        })
    }

    #[inline]
    fn string(&self, s: &str) -> ArenaValue<'static> {
        let s: &'static str = unsafe { std::mem::transmute(self.alloc.alloc_str(s)) };
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::String(s)))
        })
    }

    #[inline]
    fn sexpr(&self, items: Vec<ArenaValue<'static>>) -> ArenaValue<'static> {
        if items.is_empty() {
            return self.unit();
        }
        let slice: &'static [ArenaValue<'static>] = unsafe {
            std::mem::transmute(self.alloc.alloc_slice_from_iter(items))
        };
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::SExpr(slice)))
        })
    }

    #[inline]
    fn sexpr_from_slice(&self, items: &[ArenaValue<'static>]) -> ArenaValue<'static> {
        if items.is_empty() {
            return self.unit();
        }
        let slice: &'static [ArenaValue<'static>] = unsafe {
            std::mem::transmute(self.alloc.alloc_slice_copy(items))
        };
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::SExpr(slice)))
        })
    }

    #[inline]
    fn error(&self, msg: &str, details: ArenaValue<'static>) -> ArenaValue<'static> {
        let msg: &'static str = unsafe { std::mem::transmute(self.alloc.alloc_str(msg)) };
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Error(msg, details)))
        })
    }

    #[inline]
    fn type_value(&self, inner: ArenaValue<'static>) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Type(inner)))
        })
    }

    #[inline]
    fn conjunction(&self, goals: Vec<ArenaValue<'static>>) -> ArenaValue<'static> {
        let slice: &'static [ArenaValue<'static>] = unsafe {
            std::mem::transmute(self.alloc.alloc_slice_from_iter(goals))
        };
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Conjunction(slice)))
        })
    }

    #[inline]
    fn space(&self, handle: super::SpaceHandle) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Space(handle)))
        })
    }

    #[inline]
    fn state(&self, id: u64) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::State(id)))
        })
    }

    #[inline]
    fn unit(&self) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Unit))
        })
    }

    #[inline]
    fn memo(&self, handle: super::MemoHandle) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Memo(handle)))
        })
    }

    #[inline]
    fn empty(&self) -> ArenaValue<'static> {
        ArenaValue::from_inner(unsafe {
            std::mem::transmute(self.alloc.alloc_value(ArenaValueInner::Empty))
        })
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<(ArenaValue<'static>, usize), String> {
        // For deserialization, we use the GcFactory's own methods via the trait
        // This is a simplified implementation that delegates to ArenaValueFactory
        // using a temporary bump arena. In the future, this should deserialize
        // directly into the slab allocator.
        use bumpalo::Bump;
        use super::arena_value::ArenaValueFactory;
        let arena = Box::leak(Box::new(Bump::new()));
        let bump_factory = ArenaValueFactory::new(arena);
        let (value, consumed) = bump_factory.deserialize(bytes)?;
        // Clone the deserialized value into the slab allocator
        let result = super::clone_value(&value, self);
        Ok((result, consumed))
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
        assert_eq!(alloc.allocated_bytes(), 0);
        assert!(!alloc.needs_gc());
    }

    #[test]
    fn test_value_slot_size() {
        let alloc = SlabAllocator::new();
        let slot_size = alloc.value_slot_size();
        // Must be at least as large as ArenaValueInner and aligned
        assert!(slot_size >= std::mem::size_of::<ArenaValueInner<'static>>());
        assert_eq!(slot_size % SLOT_ALIGN, 0);
    }

    #[test]
    fn test_alloc_value_atom() {
        let alloc = SlabAllocator::new();
        let s = alloc.alloc_str("hello");
        let inner = alloc.alloc_value(ArenaValueInner::Atom(s));
        match inner {
            ArenaValueInner::Atom(a) => assert_eq!(*a, "hello"),
            _ => panic!("expected Atom"),
        }
    }

    #[test]
    fn test_alloc_value_long() {
        let alloc = SlabAllocator::new();
        let inner = alloc.alloc_value(ArenaValueInner::Long(42));
        match inner {
            ArenaValueInner::Long(n) => assert_eq!(*n, 42),
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
            let inner = alloc.alloc_value(ArenaValueInner::Long(i));
            refs.push(inner);
        }
        // Verify all values are intact
        for (i, inner) in refs.iter().enumerate() {
            match inner {
                ArenaValueInner::Long(n) => assert_eq!(*n, i as i64),
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
        let slice: &[ArenaValue<'static>] = alloc.alloc_slice_from_iter(std::iter::empty());
        assert!(slice.is_empty());
    }

    #[test]
    fn test_alloc_slice() {
        let alloc = SlabAllocator::new();
        let v1 = ArenaValue::from_inner(alloc.alloc_value(ArenaValueInner::Long(1)));
        let v2 = ArenaValue::from_inner(alloc.alloc_value(ArenaValueInner::Long(2)));
        let v3 = ArenaValue::from_inner(alloc.alloc_value(ArenaValueInner::Long(3)));

        let slice = alloc.alloc_slice_from_iter(vec![v1, v2, v3]);
        assert_eq!(slice.len(), 3);
        assert_eq!(slice[0].as_long(), Some(1));
        assert_eq!(slice[1].as_long(), Some(2));
        assert_eq!(slice[2].as_long(), Some(3));
    }

    #[test]
    fn test_alloc_slice_copy() {
        let alloc = SlabAllocator::new();
        let v1 = ArenaValue::from_inner(alloc.alloc_value(ArenaValueInner::Long(10)));
        let v2 = ArenaValue::from_inner(alloc.alloc_value(ArenaValueInner::Long(20)));
        let original = &[v1, v2];

        let copy = alloc.alloc_slice_copy(original);
        assert_eq!(copy.len(), 2);
        assert_eq!(copy[0].as_long(), Some(10));
        assert_eq!(copy[1].as_long(), Some(20));
    }

    #[test]
    fn test_allocated_bytes_grows() {
        let alloc = SlabAllocator::new();
        let before = alloc.allocated_bytes();
        for i in 0..1000 {
            alloc.alloc_value(ArenaValueInner::Long(i));
        }
        let after = alloc.allocated_bytes();
        assert!(after > before, "allocated bytes should grow");
    }

    #[test]
    fn test_free_list_reuse() {
        let alloc = SlabAllocator::new();

        // Allocate a value
        let inner = alloc.alloc_value(ArenaValueInner::Long(42));
        let ptr = inner as *const ArenaValueInner<'static> as *mut u8;

        // Free it
        unsafe { alloc.free_value(ptr); }

        // Next allocation should reuse the freed slot
        let inner2 = alloc.alloc_value(ArenaValueInner::Long(99));
        let ptr2 = inner2 as *const ArenaValueInner<'static> as *mut u8;
        assert_eq!(ptr, ptr2, "freed slot should be reused");
    }

    #[test]
    fn test_gc_threshold() {
        let alloc = SlabAllocator::new();
        assert!(!alloc.needs_gc());

        // Set a tiny threshold
        alloc.set_gc_threshold(1);

        // Allocate something
        alloc.alloc_value(ArenaValueInner::Long(1));

        // Now it should need GC
        assert!(alloc.needs_gc());
    }

    #[test]
    fn test_large_string_alloc() {
        let alloc = SlabAllocator::new();
        // String larger than largest size class (4096)
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
        let inner = alloc.alloc_value(ArenaValueInner::Long(42));
        let ptr = inner as *const ArenaValueInner<'static> as *const u8;
        assert!(alloc.contains_value(ptr));

        // A random stack pointer should not be contained
        let stack_var: u8 = 0;
        assert!(!alloc.contains_value(&stack_var as *const u8));
    }

    #[test]
    fn test_multiple_pages() {
        let alloc = SlabAllocator::new();
        let slot_size = alloc.value_slot_size();
        let slots_per_page = PAGE_SIZE / slot_size;

        // Allocate more than one page worth of values
        for i in 0..(slots_per_page + 100) {
            alloc.alloc_value(ArenaValueInner::Long(i as i64));
        }

        // Should have at least 2 pages
        let inner = alloc.inner();
        assert!(inner.values.pages.len() >= 2,
            "expected at least 2 pages, got {}", inner.values.pages.len());
    }

    // ====================================================================
    // GcFactory Tests
    // ====================================================================

    use super::super::metta_value_trait::MettaValueFactory;
    use super::super::metta_value_trait::MettaValue as MettaValueTrait;

    /// Helper to create a GcFactory from a SlabAllocator.
    /// SAFETY: The allocator must outlive the factory. Tests ensure this
    /// by keeping the allocator alive for the test's scope.
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
        // nil() now returns Unit
        assert!(v.is_unit());
        assert!(v.is_unit()); // nil() returns Unit after Nil/Unit merge
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
        // Verify all values intact
        for (i, v) in values.iter().enumerate() {
            assert_eq!(v.as_long(), Some(i as i64), "value at index {} corrupted", i);
        }
    }

    #[test]
    fn test_gc_factory_clone_value_roundtrip() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);

        // Create a complex value
        let original = factory.sexpr(vec![
            factory.atom("="),
            factory.sexpr(vec![factory.atom("double"), factory.atom("$x")]),
            factory.sexpr(vec![factory.atom("+"), factory.atom("$x"), factory.atom("$x")]),
        ]);

        // Clone it using clone_value with the same factory
        let cloned = super::super::arena_state::clone_value(&original, &factory);

        assert!(cloned.is_sexpr());
        let items = cloned.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_atom(), Some("="));
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
        let inner = alloc.alloc_value(ArenaValueInner::Long(42));
        let ptr = inner as *const ArenaValueInner<'static> as *const u8;

        // Initially not marked
        assert!(!alloc.is_value_marked(ptr));

        // Mark it
        assert!(alloc.mark_value(ptr)); // true = newly marked
        assert!(!alloc.mark_value(ptr)); // false = already marked

        // Should be marked
        assert!(alloc.is_value_marked(ptr));

        // Clear marks
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

        // Mark v1 and v3 (not v2)
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

        // Allocate some values
        let _v1 = factory.long(1);
        let _v2 = factory.long(2);

        // Take watermark
        let wm = alloc.watermark();
        assert!(wm.value_page_count >= 1);
        assert!(wm.last_page_bump_count >= 2);

        // Allocate more after watermark
        let _v3 = factory.long(3);

        // New watermark should be ahead
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
        let _v3 = factory.long(3); // Not a root — should be dead

        // Mark from roots v1 and v2
        mark_from_roots(vec![v1, v2].into_iter(), &alloc);

        assert!(alloc.is_value_marked(v1.inner_ptr() as *const u8));
        assert!(alloc.is_value_marked(v2.inner_ptr() as *const u8));
        assert!(!alloc.is_value_marked(_v3.inner_ptr() as *const u8));
    }

    #[test]
    fn test_mark_from_roots_nested() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);

        // Create nested structure: (+ 1 2)
        let atom_plus = factory.atom("+");
        let num1 = factory.long(1);
        let num2 = factory.long(2);
        let expr = factory.sexpr(vec![atom_plus, num1, num2]);

        // Only root is the sexpr — children should be traced
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
        let dead1 = factory.long(2);
        let dead2 = factory.long(3);

        let wm = alloc.watermark();

        // Mark only `alive`
        mark_from_roots(std::iter::once(alive), &alloc);

        let dead_set = sweep(&alloc, &wm);

        // 2 dead values (dead1, dead2)
        assert_eq!(dead_set.dead_values.len(), 2,
            "expected 2 dead values, got {}", dead_set.dead_values.len());
        assert_eq!(dead_set.live_values, 1);

        // Verify dead pointers are the right ones
        let dead_ptrs: std::collections::HashSet<*const u8> = dead_set.dead_values.iter()
            .map(|&p| p as *const u8).collect();
        assert!(dead_ptrs.contains(&(dead1.inner_ptr() as *const u8)));
        assert!(dead_ptrs.contains(&(dead2.inner_ptr() as *const u8)));

        alloc.clear_marks();
    }

    #[test]
    fn test_sweep_respects_watermark() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);

        let v1 = factory.long(1);
        let wm = alloc.watermark();

        // Allocate after watermark — should be implicitly alive
        let _v2 = factory.long(2);

        // Mark nothing
        let dead_set = sweep(&alloc, &wm);

        // Only v1 should be dead (v2 is after watermark)
        assert_eq!(dead_set.dead_values.len(), 1);
        let dead_ptrs: std::collections::HashSet<*const u8> = dead_set.dead_values.iter()
            .map(|&p| p as *const u8).collect();
        assert!(dead_ptrs.contains(&(v1.inner_ptr() as *const u8)));
    }

    #[test]
    fn test_sweep_collects_dead_data() {
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);

        // Create a value with associated string data
        let alive = factory.long(42);
        let _dead_atom = factory.atom("dead-string");

        let wm = alloc.watermark();

        mark_from_roots(std::iter::once(alive), &alloc);

        let dead_set = sweep(&alloc, &wm);

        // Should have dead data entries for the atom's string
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

        // Process the dead set
        alloc.process_dead_set(&dead_set);

        // Next allocation should reuse the freed slot
        let new_inner = alloc.alloc_value(ArenaValueInner::Long(99));
        let new_ptr = new_inner as *const ArenaValueInner<'static> as *mut u8;
        assert_eq!(new_ptr, dead_ptr, "freed slot should be reused");

        alloc.clear_marks();
    }

    #[test]
    fn test_live_value_count() {
        let alloc = SlabAllocator::new();
        assert_eq!(alloc.live_value_count(), 0);

        alloc.alloc_value(ArenaValueInner::Long(1));
        alloc.alloc_value(ArenaValueInner::Long(2));
        assert_eq!(alloc.live_value_count(), 2);

        // Free one
        let inner = alloc.alloc_value(ArenaValueInner::Long(3));
        let ptr = inner as *const ArenaValueInner<'static> as *mut u8;
        unsafe { alloc.free_value(ptr); }

        // Live count = 3 bumped - 1 freed = 2
        assert_eq!(alloc.live_value_count(), 2);
    }

    #[test]
    fn test_full_gc_cycle() {
        // Simulates a complete GC cycle: allocate, mark, sweep, process, verify
        let alloc = SlabAllocator::new();
        let factory = test_factory(&alloc);

        // Allocate some values
        let root1 = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        let root2 = factory.atom("keep-me");
        let _garbage1 = factory.long(999);
        let _garbage2 = factory.atom("throw-away");
        let _garbage3 = factory.sexpr(vec![factory.atom("dead"), factory.atom("expr")]);

        let initial_live = alloc.live_value_count();

        // Take watermark
        let wm = alloc.watermark();

        // Mark phase
        mark_from_roots(vec![root1, root2].into_iter(), &alloc);

        // Sweep phase
        let dead_set = sweep(&alloc, &wm);

        // We should have dead values (the garbage)
        assert!(dead_set.dead_values.len() >= 3,
            "expected at least 3 dead values, got {}", dead_set.dead_values.len());

        // Process dead set — return to free lists
        alloc.process_dead_set(&dead_set);

        // Live count should decrease
        let after_gc_live = alloc.live_value_count();
        assert!(after_gc_live < initial_live,
            "live count should decrease after GC: {} -> {}", initial_live, after_gc_live);

        // Clear marks for next cycle
        alloc.clear_marks();

        // Roots should still be accessible
        assert_eq!(root1.as_sexpr().expect("root1 is sexpr").len(), 3);
        assert_eq!(root2.as_atom(), Some("keep-me"));
    }

    #[test]
    fn test_dead_set_send() {
        fn assert_send<T: Send>() {}
        assert_send::<DeadSet>();
    }
}
