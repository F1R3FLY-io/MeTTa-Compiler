# Chapter 2: The Slab Allocator

## What Is a Slab Allocator?

A slab allocator divides memory into pages, and each page into uniform, fixed-size **slots**. Every allocation returns a slot of exactly the same size, regardless of how much of the slot the caller actually uses.

This approach eliminates **external fragmentation** — the problem where a general-purpose allocator has enough total free memory but cannot satisfy a request because the free memory is scattered in non-contiguous chunks. With uniform slots, any free slot can satisfy any request for that slot's size class.

```
General-purpose allocator (fragmented):
┌──┬────┬─┬──────┬──┬───┬─────┬──┐
│##│    │#│      │##│   │     │##│   ## = allocated, spaces = free
└──┴────┴─┴──────┴──┴───┴─────┴──┘   Can't allocate 8 bytes even though 15 free

Slab allocator (no fragmentation):
┌────┬────┬────┬────┬────┬────┬────┬────┐
│ ## │ ## │    │ ## │    │    │ ## │    │   Each slot = 4 bytes
└────┴────┴────┴────┴────┴────┴────┴────┘   Any free slot works
```

MeTTaTron uses two kinds of slabs:

- **ValueAllocator**: One slab for `MettaValueInner` values (all the same size)
- **DataClassAllocator**: Nine slabs for variable-length data (strings, slices), one per power-of-2 size class

## MmapPage — OS-Backed Memory

Every page in the allocator is a 64 KB block of memory obtained directly from the operating system via the `mmap` system call.

### Why mmap?

The standard C allocator (`malloc`/`free`, backed by `brk`/`sbrk` on Linux) grows the process heap but **never returns memory to the OS**. Even after `free()`, the process RSS (Resident Set Size) stays high. The allocator retains freed memory for future `malloc` calls.

By using `mmap(MAP_PRIVATE | MAP_ANONYMOUS)` instead, each page gets its own virtual memory mapping. When the page is no longer needed, `munmap` is called, and the OS **immediately reclaims** both the physical memory (RAM frames) and the virtual address space. The process RSS decreases visibly.

### Implementation

```rust
// gc_allocator.rs:72-114
struct MmapPage {
    ptr: *mut u8,
    len: usize,
}

impl MmapPage {
    fn new(size: usize) -> Self {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),          // let OS choose address
                size,                           // 64 KB
                libc::PROT_READ | libc::PROT_WRITE,  // read + write
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,  // private, no file
                -1, 0,                          // no file descriptor
            ) as *mut u8
        };
        // ... assert success ...
        Self { ptr, len: size }
    }
}

impl Drop for MmapPage {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len); }
    }
}
```

The constant `PAGE_SIZE` is 64 KB (`64 * 1024 = 65,536` bytes), chosen as a balance between:
- Small enough that releasing one page makes a visible RSS difference
- Large enough that the overhead of `mmap` system calls is amortized across many slot allocations

## ValuePage — Fixed-Size Value Slots

A `ValuePage` divides a 64 KB `MmapPage` into uniform slots, each sized to hold one `MettaValueInner` value (aligned to 16 bytes for SIMD-friendly access).

### Memory Layout

```
                         64 KB MmapPage
┌────────┬────────┬────────┬────────┬─── ─ ─ ──┬────────┐
│ slot 0 │ slot 1 │ slot 2 │ slot 3 │   ...     │slot N-1│
│ 80 B   │ 80 B   │ 80 B   │ 80 B   │           │ 80 B   │
└────────┴────────┴────────┴────────┴─── ─ ─ ──┴────────┘
                                                     ↑
                                           capacity = 65536 / 80 = 819 slots
```

The actual slot size depends on `size_of::<MettaValueInner>()`, rounded up to the next multiple of 16 (`SLOT_ALIGN`). At the time of writing, `MettaValueInner` is approximately 72 bytes (the largest enum variant), padded to 80 bytes.

### Fields

```rust
// gc_allocator.rs:124-141
struct ValuePage {
    data: MmapPage,             // The underlying 64 KB memory block
    bump_count: AtomicUsize,    // How many slots have been bump-allocated
    capacity: usize,            // Total slots this page can hold (immutable)
    live_count: AtomicIsize,    // Live slots (bumped − freed); signed for safe concurrent dec
    marks: Vec<AtomicU64>,      // GC mark bitmap: 1 bit per slot
    epochs: Vec<AtomicU64>,     // Per-slot epoch for TOCTOU prevention
}
```

### Bump Allocation

Fresh slots are allocated by atomically advancing the `bump_count` pointer:

```
Before allocation (bump_count = 3):
┌────────┬────────┬────────┬────────┬────────┬────────┬─── ─ ─ ──┐
│ used   │ used   │ used   │  FREE  │  FREE  │  FREE  │   ...     │
│ slot 0 │ slot 1 │ slot 2 │ slot 3 │ slot 4 │ slot 5 │           │
└────────┴────────┴────────┴────────┴────────┴────────┴─── ─ ─ ──┘
                             ↑
                         bump_count = 3

After allocation (bump_count = 4):
┌────────┬────────┬────────┬────────┬────────┬────────┬─── ─ ─ ──┐
│ used   │ used   │ used   │ TAKEN  │  FREE  │  FREE  │   ...     │
│ slot 0 │ slot 1 │ slot 2 │ slot 3 │ slot 4 │ slot 5 │           │
└────────┴────────┴────────┴────────┴────────┴────────┴─── ─ ─ ──┘
                                      ↑
                                  bump_count = 4
```

The bump allocation uses an atomic compare-and-swap (CAS) loop to ensure thread safety without locks (see [Chapter 3](03-lock-free-concurrency.md) for details):

```rust
// gc_allocator.rs:170-191
fn bump_alloc(&self, slot_size: usize) -> Option<(*mut u8, usize)> {
    loop {
        let current = self.bump_count.load(Ordering::Acquire);
        if current >= self.capacity {
            return None;  // Page is full
        }
        match self.bump_count.compare_exchange_weak(
            current, current + 1,
            Ordering::AcqRel, Ordering::Acquire,
        ) {
            Ok(_) => {
                self.live_count.fetch_add(1, Ordering::Relaxed);
                let ptr = self.slot_ptr(current, slot_size);
                return Some((ptr, current));
            }
            Err(_) => continue,  // Another thread won the race; retry
        }
    }
}
```

### Mark Bitmap

The mark bitmap uses 1 bit per slot, packed into `AtomicU64` words. For a page with 819 slots, this requires `ceil(819 / 64) = 13` words (104 bytes total — negligible overhead).

```
Mark bitmap layout (first 2 words covering slots 0-127):

    word 0 (bits 0-63)              word 1 (bits 64-127)
┌─┬─┬─┬─┬─┬─┬─┬─┬─── ─ ─ ──┐  ┌─┬─┬─┬─┬─┬─┬─┬─┬─── ─ ─ ──┐
│1│0│1│1│0│0│1│0│   ...      │  │0│0│1│0│0│0│0│1│   ...      │
└─┴─┴─┴─┴─┴─┴─┴─┴─── ─ ─ ──┘  └─┴─┴─┴─┴─┴─┴─┴─┴─── ─ ─ ──┘
 ↑   ↑               ↑          ↑           ↑
 slot 0 marked       slot 6     slot 64     slot 71
                     marked     not marked  marked
```

Setting a mark bit is a single atomic `fetch_or`:

```rust
fn set_mark(&self, idx: usize) {
    let word = idx / 64;
    let bit = idx % 64;
    self.marks[word].fetch_or(1u64 << bit, Ordering::Relaxed);
}
```

### Epoch Array

Each slot has a 64-bit epoch counter used for TOCTOU prevention (explained in detail in [Chapter 4](04-garbage-collector.md)). When a slot is re-allocated from the free list, its epoch is set to the allocator's current monotonic epoch. The GC uses this to determine whether a slot was re-allocated after the snapshot was taken.

## DataPage — Variable-Length Data Slots

`DataPage` has the same structure as `ValuePage` but without `marks` and `epochs`:

```rust
// gc_allocator.rs:261-266
struct DataPage {
    data: MmapPage,
    bump_count: AtomicUsize,
    capacity: usize,
    live_count: AtomicIsize,
}
```

Data pages don't need mark bitmaps because data is **tracked through its owning value**. When a value like `MettaValueInner::Atom("hello")` is marked as reachable, its string data is implicitly reachable too. When the value is swept as dead, the `collect_dead_data()` function extracts the data pointer and size for freeing.

Data pages hold the content referenced by values:
- `Atom(&str)` → string bytes in a data page
- `String(&str)` → string bytes in a data page
- `SExpr(&[MettaValue])` → slice of pointers in a data page
- `Conjunction(&[MettaValue])` → slice of pointers in a data page
- `Error(&str, _)` → error message bytes in a data page

## Size Classes

Variable-length data is allocated from power-of-2 size classes:

```
Size Class   Slot Size   Slots per 64 KB Page   Usage
─────────────────────────────────────────────────────────────────
Class 0      16 bytes    4,096 slots            Tiny strings (≤16 B)
Class 1      32 bytes    2,048 slots            Short strings, small slices
Class 2      64 bytes    1,024 slots            Medium strings
Class 3      128 bytes   512 slots              Longer strings
Class 4      256 bytes   256 slots              Large strings
Class 5      512 bytes   128 slots              Large slices
Class 6      1,024 bytes 64 slots               Very large strings
Class 7      2,048 bytes 32 slots               Very large slices
Class 8      4,096 bytes 16 slots               Maximum slab size
```

The minimum slot size is 16 bytes because every slot must be large enough to hold a `FreeNode` (a 128-bit/16-byte next-pointer) when it is on the Treiber stack free list.

**Size class selection**: `alloc_data(size)` iterates the size classes and selects the smallest class ≥ the requested size:

```rust
// gc_allocator.rs:878-897
fn alloc_data(&self, size: usize) -> *mut u8 {
    if size == 0 {
        return SLOT_ALIGN as *mut u8;
    }
    for (i, &class_size) in DATA_SIZE_CLASSES.iter().enumerate() {
        if size <= class_size {
            return self.data_classes[i].alloc();
        }
    }
    // Large allocation (> 4096): use system allocator
    let layout = Layout::from_size_align(size, SLOT_ALIGN).expect("invalid layout");
    let ptr = unsafe { std::alloc::alloc(layout) };
    // ... store in large_allocs Vec ...
    ptr
}
```

**Example**: A 5-byte string `"hello"` is allocated from the 16-byte size class. The remaining 11 bytes are wasted (internal fragmentation), but this is an acceptable trade-off for O(1) allocation and deallocation.

**Large allocations**: Data larger than 4,096 bytes falls back to the system allocator (`std::alloc::alloc`). These are tracked in a `Mutex<Vec<(*mut u8, Layout)>>` for cleanup. This path is rare in practice — most MeTTa values have small string and slice payloads.

## ValueAllocator and DataClassAllocator

The `SlabAllocator` is composed of one `ValueAllocator` and nine `DataClassAllocator`s:

```rust
// gc_allocator.rs:751-764
pub struct SlabAllocator {
    values: ValueAllocator,                  // Fixed-size MettaValueInner slots
    data_classes: Vec<DataClassAllocator>,   // 9 power-of-2 size classes
    large_allocs: Mutex<Vec<(*mut u8, Layout)>>,  // Fallback for > 4096 B
    gc_threshold: AtomicUsize,               // When to trigger GC
    committed_bytes_atomic: Arc<AtomicUsize>, // For cron manager reads
    alloc_count_atomic: Arc<AtomicU64>,       // For cron manager reads
}
```

Both `ValueAllocator` and `DataClassAllocator` follow the same three-tier allocation strategy.

### Three-Tier Allocation Fallback

Each sub-allocator tries three strategies in order, from fastest to slowest:

```
                    ┌─────────────────────┐
                    │  Allocation Request  │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
              ┌─YES─┤ Free list non-empty? ├─NO─┐
              │     └─────────────────────┘     │
              │                                  │
    ┌─────────▼─────────┐            ┌──────────▼──────────┐
    │ Tier 1: Free list │      ┌─YES─┤ Current page has    ├─NO─┐
    │  Treiber stack pop│      │     │  room (bump < cap)? │     │
    │  O(1), lock-free  │      │     └─────────────────────┘     │
    └───────────────────┘      │                                  │
                     ┌─────────▼─────────┐            ┌──────────▼──────────┐
                     │ Tier 2: Bump alloc │            │ Tier 3: New page    │
                     │  Atomic CAS on    │            │  Write-lock pages   │
                     │  bump_count, O(1) │            │  mmap 64 KB, O(1)   │
                     │  Lock-free        │            │  Amortized, rare    │
                     └───────────────────┘            └─────────────────────┘
```

**Tier 1 (free list pop)** is the fastest path. It reuses a previously freed slot via the lock-free Treiber stack. No memory is allocated; the slot is immediately available.

**Tier 2 (bump allocation)** uses the current page's atomic bump pointer. A CAS loop advances `bump_count` by 1 and returns the slot pointer. This is lock-free and O(1).

**Tier 3 (new page)** is the slow path. It acquires a write lock on the page vector, allocates a new 64 KB `MmapPage`, creates a new `ValuePage` or `DataPage`, and bump-allocates the first slot. A **double-check** pattern prevents duplicate page allocation when multiple threads contend on the write lock:

```rust
// gc_allocator.rs:655-670
fn alloc_new_page(&self) -> *mut u8 {
    let mut pages = self.pages.write().expect("poisoned");
    // Double-check: another thread may have added a page while we waited
    if let Some(last) = pages.last() {
        if let Some((ptr, _)) = last.bump_alloc(self.slot_size) {
            return ptr;
        }
    }
    let page = Box::new(ValuePage::new(self.slot_size));
    let (ptr, _) = page.bump_alloc(self.slot_size).expect("fresh page");
    let page_ptr = &*page as *const ValuePage as *mut ValuePage;
    self.current_page.store(page_ptr, Ordering::Release);
    pages.push(page);
    ptr
}
```

## SlabAllocator — Top-Level API

The `SlabAllocator` exposes three main allocation methods:

### `alloc_value(val)` — Allocate a MettaValueInner

Writes a `MettaValueInner` into a value slot and returns `&'static MettaValueInner`:

```rust
// gc_allocator.rs:805-825
pub fn alloc_value(&self, val: MettaValueInner) -> &'static MettaValueInner {
    let ptr = self.values.alloc();
    // ... update atomic counters ...
    unsafe {
        std::ptr::write_bytes(ptr, 0, self.values.slot_size);  // zero padding
        std::ptr::write(ptr as *mut MettaValueInner, val);
        &*(ptr as *const MettaValueInner)
    }
}
```

The slot is zero-initialized before writing to eliminate stale padding bytes that could affect comparison or hashing.

### `alloc_str(s)` — Allocate a String Slice

Copies the string's bytes into a data slot and returns `&'static str`:

```rust
// gc_allocator.rs:829-839
pub fn alloc_str(&self, s: &str) -> &'static str {
    if s.is_empty() { return ""; }
    let bytes = s.as_bytes();
    let ptr = self.alloc_data(bytes.len());
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, bytes.len()))
    }
}
```

### `alloc_slice_from_iter(iter)` — Allocate a Value Slice

Collects an iterator into a `Vec`, copies the values into a data slot, and returns `&'static [MettaValue]`:

```rust
// gc_allocator.rs:842-860
pub fn alloc_slice_from_iter(
    &self,
    items: impl IntoIterator<Item = MettaValue>,
) -> &'static [MettaValue] {
    let items: Vec<MettaValue> = items.into_iter().collect();
    if items.is_empty() { return &[]; }
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
```

### Example: Allocating `(+ 1 2)`

To allocate the MeTTa expression `(+ 1 2)`, the factory performs these steps:

```
Step 1: alloc_str("+")
  → alloc_data(1)  → 16-byte class, slot ptr = 0x7f...a010
  → copy "+" byte  → returns &'static str pointing to 0x7f...a010

Step 2: alloc_value(Atom("+"))
  → values.alloc() → value slot ptr = 0x7f...b000
  → write MettaValueInner::Atom(&str) into slot
  → returns &'static MettaValueInner (an Atom)

Step 3: alloc_value(Long(1))
  → values.alloc() → value slot ptr = 0x7f...b050  (next slot, +80 bytes)
  → write MettaValueInner::Long(1)
  → returns &'static MettaValueInner

Step 4: alloc_value(Long(2))
  → values.alloc() → value slot ptr = 0x7f...b0a0  (next slot, +80 bytes)
  → write MettaValueInner::Long(2)
  → returns &'static MettaValueInner

Step 5: alloc_slice_from_iter([Atom("+"), Long(1), Long(2)])
  → alloc_data(3 * 8 = 24) → 32-byte class, slot ptr = 0x7f...c020
  → write 3 MettaValue pointers
  → returns &'static [MettaValue; 3]

Step 6: alloc_value(SExpr(&[MettaValue; 3]))
  → values.alloc() → value slot ptr = 0x7f...b0f0
  → write MettaValueInner::SExpr(slice_ref)
  → returns &'static MettaValueInner (an SExpr)

Result: MettaValue wrapping the SExpr at 0x7f...b0f0
```

The resulting memory layout:

```
Value page (80-byte slots):
┌──────────────┬──────────────┬──────────────┬──────────────┬── ─ ─ ──┐
│ Atom("+")    │ Long(1)      │ Long(2)      │ SExpr(→data) │  ...    │
│ ptr→data page│              │              │ ptr→data page│         │
└──────┬───────┴──────────────┴──────────────┴──────┬───────┴── ─ ─ ──┘
       │                                            │
       │  Data page (16-byte class):                │  Data page (32-byte class):
       │  ┌──────────────┐                          │  ┌────────────────────────┐
       └─→│ "+"  (1 byte)│                          └─→│ [ptr, ptr, ptr] (24 B) │
          │ + 15 B pad   │                             │ + 8 B padding          │
          └──────────────┘                             └────────────────────────┘
```

## What's Next

[Chapter 3](03-lock-free-concurrency.md) dives into the lock-free algorithms that make allocation thread-safe without mutexes — the Treiber stack, compare-and-swap loops, the ABA problem, and memory ordering.
