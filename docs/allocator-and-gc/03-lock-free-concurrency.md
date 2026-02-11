# Chapter 3: Lock-Free Concurrency

## Why Lock-Free?

MeTTaTron evaluates MeTTa expressions across multiple threads. The allocator is on the critical path — every value creation, every rule application, every pattern match allocates. If the allocator used a mutex, threads would serialize on allocation, negating the benefits of parallelism.

Lock-based allocators suffer from two specific problems under contention:

1. **Priority inversion.** A low-priority thread holds the lock, blocking a high-priority thread. The high-priority thread cannot proceed even though it has more important work to do.

2. **Convoy effect.** When a thread holding the lock is descheduled (preempted by the OS), all other threads waiting for the lock are blocked until the holding thread is rescheduled and releases the lock. Under heavy contention, this creates a "convoy" of blocked threads.

A **lock-free** algorithm guarantees **system-wide progress**: even if individual threads stall (are preempted, sleep, or crash), at least one thread in the system always makes forward progress. The MeTTaTron allocator achieves this using compare-and-swap (CAS) loops for both the Treiber stack free list and the bump pointer.

## Compare-and-Swap (CAS)

Compare-and-swap is the fundamental building block of lock-free algorithms. It is a single atomic instruction that performs three operations in one indivisible step:

1. Read the current value of a memory location
2. Compare it to an expected value
3. If they match, write a new value; otherwise, do nothing

The operation returns the old value so the caller can tell whether the swap succeeded.

### Rust API

```rust
// Atomically: if *self == expected, set *self = new and return Ok(expected)
//             else return Err(current_value)
let result = atomic.compare_exchange_weak(
    expected,     // what we expect the value to be
    new,          // what we want to set it to
    Ordering::AcqRel,   // memory ordering on success
    Ordering::Acquire,  // memory ordering on failure
);
```

### `compare_exchange_weak` vs `compare_exchange`

The allocator uses `compare_exchange_weak` throughout. The difference:

- **`compare_exchange`** (strong): Guaranteed to succeed if the current value matches `expected`. Compiles to a CAS loop on platforms that don't have native CAS (like ARM's LL/SC).
- **`compare_exchange_weak`**: May **spuriously fail** even when the current value matches `expected`. This is cheaper on ARM/RISC-V because it maps directly to a single LL/SC pair without a retry loop.

Since the allocator already wraps CAS in a retry loop (to handle genuine contention), spurious failures are harmless — they just cause one extra iteration.

## The ABA Problem

The ABA problem is a classic pitfall in lock-free data structures that use CAS. It occurs when the value at a memory location changes from A to B and back to A, making a CAS operation believe nothing changed when in fact the underlying state has been modified.

### Step-by-Step Example

Consider a lock-free stack with nodes X and Y:

```
Initial state: head → X → Y → NULL

Thread A:                          Thread B:
─────────                          ─────────
1. Read head = X
   Read X.next = Y
   (preparing to pop X)
                                   2. Pop X:  head → Y → NULL
                                   3. Pop Y:  head → NULL
                                   4. Push X: head → X → NULL
                                      (X.next is now NULL, not Y)
5. CAS head: X → Y
   CAS succeeds! (head is still X)
   But X.next is NULL, not Y!

Result: head → Y → ???
   Y was freed and its memory may be corrupted.
   Thread A has set head to a dangling pointer.
```

The CAS in step 5 succeeds because `head` still contains the pointer to X, but the stack's structure has changed. Thread A's stale snapshot of `X.next = Y` is now wrong — Y was freed and X's next pointer now points to NULL.

### Why This Matters

In a memory allocator, the ABA problem can cause:
- **Use-after-free**: A freed slot's next pointer is overwritten by a new allocation, but another thread uses the stale next pointer
- **Memory corruption**: The free list becomes inconsistent, leading to double allocation of the same slot

## Treiber Stack — Lock-Free Free List

The Treiber stack is a lock-free singly-linked stack invented by R. Kent Treiber in 1986. Each freed slot becomes a node in the stack, with its first 16 bytes repurposed to hold a next-pointer.

### Data Structure

```rust
// gc_allocator.rs:349-361
#[repr(C)]
struct FreeNode {
    next: u128,  // Packed [64-bit counter | 64-bit pointer]
}

struct TreiberStack {
    head: AtomicU128,  // Packed head pointer + ABA counter
}
```

### ABA Prevention via 128-bit Packed Word

The key insight is to pair the pointer with a monotonically increasing counter in a single 128-bit atomic word:

```
128-bit packed word layout:
┌────────────────────────────────┬────────────────────────────────┐
│     Counter (64 bits)          │     Pointer (64 bits)          │
│     bits 127..64               │     bits 63..0                 │
└────────────────────────────────┴────────────────────────────────┘
```

The counter increments on every push and pop. Even if the pointer value cycles back to the same address (the "ABA" scenario), the counter will be different, causing the CAS to fail and retry with fresh data.

With a 64-bit counter, wrap-around requires 2^64 operations (~18.4 quintillion). At 1 billion operations per second, this takes 584 years — effectively impossible in practice.

```rust
// gc_allocator.rs:332-347
fn treiber_pack(ptr: *mut u8, counter: u64) -> u128 {
    ((counter as u128) << 64) | (ptr as u64 as u128)
}

fn treiber_unpack_ptr(packed: u128) -> *mut u8 {
    (packed as u64) as *mut u8
}

fn treiber_unpack_counter(packed: u128) -> u64 {
    (packed >> 64) as u64
}
```

### Push Operation

Pushing a freed slot onto the stack:

```
Before: head → [A] → [B] → NULL        (counter = 5)

Push C:
  1. Load old_head = pack(ptr_A, 5)         // Acquire
  2. Write C.next = old_head                // Link C to current head
  3. CAS head: pack(ptr_A, 5) → pack(ptr_C, 6)  // AcqRel

After:  head → [C] → [A] → [B] → NULL  (counter = 6)
```

```rust
// gc_allocator.rs:371-389
fn push(&self, ptr: *mut u8) {
    loop {
        let old_head = self.head.load(Ordering::Acquire);
        let node = ptr as *mut FreeNode;
        unsafe { (*node).next = old_head; }
        let old_counter = treiber_unpack_counter(old_head);
        let new_head = treiber_pack(ptr, old_counter.wrapping_add(1));
        match self.head.compare_exchange_weak(
            old_head, new_head,
            Ordering::AcqRel, Ordering::Acquire,
        ) {
            Ok(_) => return,
            Err(_) => continue,  // Contention — retry
        }
    }
}
```

### Pop Operation

Popping a free slot from the stack:

```
Before: head → [C] → [A] → [B] → NULL  (counter = 6)

Pop:
  1. Load old_head = pack(ptr_C, 6)              // Acquire
  2. Read C.next = pack(ptr_A, 5)                // Follow link
  3. CAS head: pack(ptr_C, 6) → pack(ptr_A, 7)  // AcqRel
  4. Return ptr_C

After:  head → [A] → [B] → NULL         (counter = 7)
```

```rust
// gc_allocator.rs:393-419
fn pop(&self) -> Option<*mut u8> {
    loop {
        let old_head = self.head.load(Ordering::Acquire);
        if old_head == TREIBER_NULL {
            return None;  // Stack is empty
        }
        let ptr = treiber_unpack_ptr(old_head);
        let old_counter = treiber_unpack_counter(old_head);
        let next = unsafe { (*(ptr as *const FreeNode)).next };
        let new_head = if next == TREIBER_NULL {
            TREIBER_NULL
        } else {
            treiber_pack(treiber_unpack_ptr(next), old_counter.wrapping_add(1))
        };
        match self.head.compare_exchange_weak(
            old_head, new_head,
            Ordering::AcqRel, Ordering::Acquire,
        ) {
            Ok(_) => return Some(ptr),
            Err(_) => continue,  // Contention — retry
        }
    }
}
```

### Platform-Specific Implementations

The 128-bit atomic operations are provided by the `portable-atomic` crate, which selects the best implementation for each platform:

| Platform | Instruction | ABA Protection |
|----------|-------------|----------------|
| x86-64 | `CMPXCHG16B` | Counter-based (64-bit counter in packed word) |
| ARM64 (AArch64) | `LDXP` / `STXP` | LL/SC: hardware invalidates the exclusive monitor on any intervening store to the same cache line, inherently preventing ABA |
| RISC-V | `LR` / `SC` | LL/SC: same inherent ABA immunity as ARM64 |
| Fallback | Software emulation | `portable-atomic` provides a spin-lock fallback for platforms without native 128-bit atomics |

On ARM64 and RISC-V, the counter is technically redundant because the LL/SC mechanism provides inherent ABA immunity. However, the counter is still used for consistency and to support x86-64.

## Atomic Bump Allocation

The bump pointer in `ValuePage::bump_alloc()` uses the same CAS loop pattern:

```rust
loop {
    let current = self.bump_count.load(Ordering::Acquire);
    if current >= self.capacity {
        return None;  // Page full
    }
    match self.bump_count.compare_exchange_weak(
        current, current + 1,
        Ordering::AcqRel, Ordering::Acquire,
    ) {
        Ok(_) => {
            self.live_count.fetch_add(1, Ordering::Relaxed);
            return Some((self.slot_ptr(current, slot_size), current));
        }
        Err(_) => continue,
    }
}
```

This is **lock-free but not wait-free**:
- **Lock-free**: If multiple threads contend, at least one will always succeed on each round of CAS attempts
- **Not wait-free**: An individual thread could theoretically be starved by always losing the CAS race (in practice, this doesn't happen because CAS contention resolves in nanoseconds)

### Double-Check Pattern in New Page Allocation

When multiple threads simultaneously discover that the current page is full, they all attempt to acquire the write lock to create a new page. The double-check pattern ensures only one page is created:

```rust
fn alloc_new_page(&self) -> *mut u8 {
    let mut pages = self.pages.write().expect("poisoned");

    // Double-check: another thread may have added a page while we waited
    if let Some(last) = pages.last() {
        if let Some((ptr, _)) = last.bump_alloc(self.slot_size) {
            return ptr;  // Another thread already added a page; use it
        }
    }

    // We're the first thread here — create new page
    let page = Box::new(ValuePage::new(self.slot_size));
    // ...
}
```

## Memory Ordering

The allocator uses the minimum memory ordering required for correctness. Stronger orderings (like `SeqCst`) provide stronger guarantees but generate more expensive fence instructions.

| Ordering | Where Used | Why |
|----------|-----------|-----|
| `Relaxed` | `total_allocated`, `live_count`, `alloc_count_atomic`, `committed_bytes_atomic`, mark bits | These are counters and diagnostics. Eventual visibility is sufficient — they don't guard other memory accesses. Mark bits use `Relaxed` because the GC thread owns its snapshot bitmaps exclusively. |
| `Acquire` | Loading `head` pointer, loading `bump_count`, loading `current_page` | **Acquire** ensures that after loading the pointer, all stores made by the thread that previously released it are visible. This is necessary to read the `FreeNode.next` field or page data safely. |
| `Release` | Storing `current_page`, storing slot epochs | **Release** ensures that all stores made before this point (e.g., writing data into a page, setting a slot epoch) are visible to any thread that subsequently performs an Acquire load. |
| `AcqRel` | CAS operations on `head` and `bump_count`, `epoch.fetch_add` | **Acquire-Release** on CAS: the successful CAS both acquires the old value's context and releases the new value's context. This ensures the thread that pops a node sees all prior writes to the node's data, and the thread that pushes a node makes its writes visible. |

**No `SeqCst` needed.** Sequential consistency requires a total ordering across all atomic variables, which generates full memory fences on x86 and DMB instructions on ARM. The allocator doesn't need a global total order — each CAS loop establishes local happens-before ordering, which is sufficient for correctness.

## What's Next

[Chapter 4](04-garbage-collector.md) describes the garbage collector: how snapshots provide concurrent isolation, how epoch-based filtering prevents use-after-free, and how the GC thread communicates with the evaluation thread.
