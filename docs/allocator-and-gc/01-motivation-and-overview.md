# Chapter 1: Motivation and Overview

## The Problem

MeTTaTron's evaluation engine allocates and discards millions of small, immutable values during pattern matching, rule application, and expression evaluation. The allocator must support concurrent access from multiple threads (parallel evaluation, JIT compilation) while keeping allocation overhead to a minimum. Two prior approaches were tried and proved insufficient.

### Arc\<MettaValue\> — Reference Counting

The first implementation wrapped every value in `Arc<MettaValue>`, using atomic reference counts for memory management. This has three problems:

1. **Per-value atomic overhead.** Every clone increments an atomic counter; every drop decrements it. In a tight evaluation loop that clones values millions of times per second, these atomic read-modify-write operations dominate CPU time.

2. **Cache-line bouncing.** When multiple threads share and clone the same value, the cache line containing the reference count bounces between CPU cores via the cache coherency protocol (MESI/MOESI). This serializes concurrent access to shared values.

3. **No bulk reclamation.** Each value is freed individually when its count reaches zero. There is no way to release a batch of temporaries in O(1) time.

### Bumpalo — Arena Allocation

The second implementation used `bumpalo::Bump` for arena allocation with session-scoped bulk deallocation. This solved the bulk-reclaim problem but introduced new ones:

1. **`!Sync` interior mutability.** Bumpalo's `Bump` allocator uses `Cell<NonNull<u8>>` for its bump pointer, making it `!Sync`. This caused **SIGSEGV** crashes in multi-threaded tests (observed in 2 out of 20 runs) when multiple threads attempted to allocate from the same arena concurrently.

2. **No incremental reclamation.** Bumpalo supports only bulk deallocation (drop the entire arena). Long-running evaluations accumulate dead values that cannot be reclaimed until the arena is dropped, leading to unbounded RSS growth.

3. **No guaranteed OS memory return.** Bumpalo allocates pages via `alloc::alloc` (ultimately `brk`/`sbrk` on Linux), which grows the process heap but never returns memory to the OS. RSS monotonically increases even after values become unreachable.

### Requirements

The replacement allocator must:

- Support **lock-free concurrent allocation** from multiple threads (no mutexes on the hot path)
- Provide **zero-cost value cloning** (pointer copy, not deep copy or atomic increment)
- Perform **concurrent mark-sweep garbage collection** without stop-the-world pauses
- **Guarantee RSS decrease** when the GC releases empty pages (via `munmap`)
- Be amenable to **formal verification** of correctness properties

## Design Goals

| Goal | Mechanism |
|------|-----------|
| Lock-free allocation hot path | Treiber stack (free list) + atomic CAS bump pointer |
| Zero-cost cloning | `MettaValue` is `Copy` — 8-byte pointer, no refcount |
| Concurrent GC without pausing | Snapshot-based async mark-sweep on a dedicated thread |
| Guaranteed RSS decrease | `mmap`/`munmap` for page allocation — OS reclaims immediately |
| Formal verification | TLA+ models exhaustively exploring all thread interleavings |

## Architecture Overview

The allocator is organized in layers. Each layer provides a well-defined abstraction to the layer above it.

```
┌─────────────────────────────────────────────────┐
│              GcFactory (User API)                │  atom(), sexpr(), long(), ...
│  Implements MettaValueFactory<MettaValue>         │  8 bytes, Copy + Clone + Send + Sync
├─────────────────────────────────────────────────┤
│           SlabAllocator (Core)                   │  alloc_value(), alloc_str(),
│  1 ValueAllocator + 9 DataClassAllocators        │  alloc_slice_from_iter()
├──────────────────┬──────────────────────────────┤
│  ValueAllocator  │  DataClassAllocator ×9        │  Fixed-size value slots,
│  (MettaValueInner│  (16, 32, 64, ... 4096 B)    │  power-of-2 data classes
│   uniform slots) │                              │
├──────────────────┴──────────────────────────────┤
│        MmapPage (64 KB per page, mmap)           │  OS memory management,
│  MAP_PRIVATE | MAP_ANONYMOUS                     │  munmap on drop → RSS ↓
├─────────────────────────────────────────────────┤
│      TreiberStack (lock-free free list)           │  Per-allocator free list,
│  128-bit ABA-safe CAS (CMPXCHG16B / LDXP+STXP)  │  O(1) push/pop
├─────────────────────────────────────────────────┤
│  GcThread  │  GcCron  │  RootRegistry            │  Background GC thread,
│  (snapshot  │ (100ms   │  (Weak<dyn RootProvider>) │  rate monitoring,
│   mark-sweep│  monitor)│                          │  auto-pruning roots
└─────────────┴──────────┴──────────────────────────┘
```

**Data flow:**

1. Evaluation code calls `GcFactory` methods (e.g., `factory.sexpr(vec![...])`)
2. `GcFactory` delegates to `SlabAllocator::alloc_value()` and `alloc_str()`/`alloc_slice_from_iter()`
3. `SlabAllocator` routes to `ValueAllocator` (for values) or the appropriate `DataClassAllocator` (for strings/slices)
4. Each sub-allocator tries three tiers: free list pop → bump allocate → new page
5. Results are returned as `&'static MettaValueInner` references, wrapped in `MettaValue`

**GC flow (concurrent, non-blocking):**

1. Every 256 trampoline iterations, `SessionContext::maybe_gc()` calls `maybe_trigger_gc()`
2. If `GC_REQUESTED` is set (by the cron manager or allocation pressure), a `GcSnapshot` is built
3. The snapshot is sent to the `GcThread` via an `mpsc` channel
4. The GC thread marks reachable values from roots, then sweeps to build a dead set
5. The `GcResponse` is sent back and processed (with epoch filtering) on the next `maybe_trigger_gc()` call
6. Dead value and data slots are returned to free lists; empty pages are `munmap`'d

## Key Invariants

The design upholds the following safety invariants:

1. **Values are immutable after allocation.** Once a `MettaValueInner` is written to a slot, its content never changes. This makes concurrent reads safe without synchronization.

2. **`'static` lifetime is valid.** The global `SlabAllocator` lives for the entire program duration (stored in a `OnceLock`). Values are only freed by the GC when they are proven unreachable, so the `&'static` reference remains valid for any code that holds it.

3. **GC thread never touches live allocator state.** The GC operates exclusively on an owned `GcSnapshot` — a frozen copy of page pointers, bump counts, and roots captured at snapshot time. The evaluation thread can freely allocate new values while the GC runs, because new allocations go into slots not covered by the snapshot.

4. **Epoch filtering prevents TOCTOU use-after-free.** When a slot is re-allocated from the free list after a snapshot is taken, its epoch is incremented. When the GC response is processed, slots with `epoch > snapshot_epoch` are skipped (they contain live values, not dead ones).

5. **Page live_count tracks all allocation paths.** Both bump allocation and free-list re-allocation increment the page's `live_count`. This prevents premature page release while live values still reference slots in the page.

6. **`munmap` guarantees RSS decrease.** Pages are backed by `mmap(MAP_PRIVATE | MAP_ANONYMOUS)`. When a page's `live_count` reaches zero, `Drop` calls `munmap`, and the OS immediately reclaims the physical memory and virtual address space.

## What's Next

[Chapter 2](02-slab-allocator.md) describes the slab allocator's internal structure — how pages are laid out, how slots are managed, and how the three-tier allocation fallback works.
