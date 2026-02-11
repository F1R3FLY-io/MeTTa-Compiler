# Chapter 4: The Garbage Collector

## Mark-Sweep Overview

Mark-sweep is one of the oldest and simplest garbage collection algorithms, first described by John McCarthy in 1960 for Lisp. It works in two phases:

1. **Mark**: Starting from a set of **root** values (values known to be reachable by the program), recursively trace all references and mark every reachable value.
2. **Sweep**: Iterate all allocated values. Unmarked values are dead (unreachable) and can be freed.

Mark-sweep is well-suited for MeTTaTron's slab allocator because:

- **No compaction needed.** Slab allocators have no external fragmentation (all slots are the same size), so there is nothing to compact. This eliminates the most complex and expensive part of many GC designs.
- **No reference counting overhead.** Values are `Copy` (8-byte pointers with no refcount). Cloning a value is a pointer copy with zero atomic operations.
- **Handles cycles naturally.** Unlike reference counting, mark-sweep correctly handles cyclic data structures (though MeTTa values are typically acyclic trees).

## The Concurrent GC Challenge

A naive mark-sweep implementation would require **stopping the world** — pausing all evaluation threads while the GC marks and sweeps. This is unacceptable for an interactive evaluator or a compiler processing large workloads.

The fundamental problem is that if the GC reads allocator state while the evaluator mutates it, the result is a data race:

- The GC iterates the page list while the evaluator adds a new page (Vec reallocation → use-after-free of the old backing array)
- The GC reads the free list while the evaluator pushes/pops (corrupted traversal)
- The GC reads `bump_count` while the evaluator advances it (inconsistent slot enumeration)

MeTTaTron solves this with **snapshot-based async GC**: the evaluation thread builds an immutable snapshot of the allocator's state and hands it to a background GC thread. The GC thread operates exclusively on the snapshot and never touches the live allocator.

## Snapshot Construction

The evaluation thread builds a `GcSnapshot` that captures a frozen view of the allocator at a single point in time:

```rust
// gc_allocator.rs:1268-1276
pub struct GcSnapshot {
    pub page_snapshots: Vec<PageSnapshot>,   // Frozen page data pointers + bump counts
    pub slot_size: usize,                     // Value slot size (immutable)
    pub free_set: HashSet<*const u8>,         // Currently free slots (empty — see below)
    pub snapshot_epoch: u64,                  // Allocator's epoch at snapshot time
    pub roots: Vec<MettaValue>,      // Live root values
    pub marks: Vec<Vec<u64>>,                 // Fresh mark bitmaps (zeroed)
    pub total_committed_bytes: usize,         // For threshold calibration
}
```

Each `PageSnapshot` captures a page's data pointer and its `bump_count` at snapshot time:

```rust
pub struct PageSnapshot {
    pub data_ptr: *const u8,   // Stable — pages are never moved while live
    pub bump_count: usize,     // Frozen count of committed slots
    pub capacity: usize,       // Maximum slots
}
```

```
                    Snapshot as a frozen window
                    ┌─────────────────────────────────┐
                    │ Captured at time T               │
                    │                                  │
                    │  Page 0: data=0x7f..a, bump=819  │
                    │  Page 1: data=0x7f..b, bump=412  │
                    │  Epoch: 57                       │
                    │  Roots: [val_1, val_2, val_3]    │
                    │  Marks: [[0,0,...], [0,0,...]]    │
                    └─────────────────────────────────┘

After snapshot, the evaluator allocates freely:
  Page 1: bump goes from 412 → 413 → 414 → ...
  Page 2: new page created (not in snapshot)

The GC sees only bump=412 for Page 1, and Page 2 doesn't exist in its view.
New allocations are invisible to the GC — they can never be incorrectly freed.
```

**Key design decisions:**

1. **Page data pointers are stable.** Pages are heap-allocated (`Box<ValuePage>`), so their `data.ptr` never changes even if the `Vec<Box<ValuePage>>` reallocates its backing array.

2. **The free set is empty.** The Treiber stack cannot be iterated safely (it's a lock-free data structure with concurrent push/pop). Instead, the snapshot uses an empty free set. Free-list slots appear as "unmarked allocated" during sweep — they're already dead, so reporting them as dead again is harmless. The epoch filter (below) prevents double-freeing.

3. **Fresh mark bitmaps.** The snapshot contains its own zeroed mark bitmaps, separate from the allocator's `ValuePage::marks`. This prevents data races — the GC thread writes marks into the snapshot's bitmaps, not the live page bitmaps.

## Root Collection

Roots are the set of values known to be reachable by the program. If a value is not transitively reachable from a root, it is garbage.

### RootProvider Trait

```rust
// gc_allocator.rs:1114-1119
pub trait RootProvider: Send + Sync {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>);
}
```

Any object that holds GC-managed values can implement `RootProvider` and register itself with the global root registry. In practice, the primary root provider is `GenericEnvironmentShared<MettaValue>`, which collects:
- Rule left-hand sides and right-hand sides
- Symbol bindings (variable → value mappings)
- State cell values
- Space contents

### Global Root Registry

```rust
// gc_allocator.rs:1130
static ROOT_REGISTRY: OnceLock<RwLock<Vec<Weak<dyn RootProvider>>>> = OnceLock::new();
```

The registry uses **weak references** (`Weak<dyn RootProvider>`) so that environments are automatically removed when they go out of scope. No explicit unregistration is needed — dead entries are pruned during `collect_all_roots()`:

```rust
// gc_allocator.rs:1152-1164
pub fn collect_all_roots() -> Vec<MettaValue> {
    let mut registry = root_registry().write().expect("poisoned");
    let mut roots = Vec::with_capacity(registry.len() * 64);
    registry.retain(|weak| {
        if let Some(strong) = weak.upgrade() {
            strong.collect_roots(&mut roots);
            true    // Keep this provider
        } else {
            false   // Provider was dropped — remove
        }
    });
    roots
}
```

### Conditional Registration

Registration uses `Any` downcasting to ensure only `MettaValue` environments are registered (not environments parameterized over other value types):

```rust
// gc_allocator.rs:1173-1186
pub fn try_register_env_roots<V>(shared: &Arc<GenericEnvironmentShared<V>>) {
    let any: Arc<dyn Any + Send + Sync> = shared.clone();
    if let Ok(arena_shared) = any.downcast::<GenericEnvironmentShared<MettaValue>>() {
        let provider: Arc<dyn RootProvider> = arena_shared;
        register_root_provider(&provider);
    }
    // For non-MettaValue types, this is a no-op
}
```

## Mark Phase

The mark phase uses an iterative worklist algorithm (not recursive, to avoid stack overflow on deeply nested expressions):

```rust
// gc_allocator.rs:1469-1526
pub fn mark_snapshot(snapshot: &mut GcSnapshot) {
    let slot_size = snapshot.slot_size;
    let mut worklist: Vec<*const MettaValueInner> = Vec::with_capacity(1024);

    // Seed worklist with root pointers
    for root in &snapshot.roots {
        let ptr = root.inner_ptr();
        if snapshot_mark_value(snapshot, ptr as *const u8, slot_size) {
            worklist.push(ptr);  // Newly marked — trace its children
        }
    }

    // Trace children iteratively
    while let Some(ptr) = worklist.pop() {
        match unsafe { &*ptr } {
            MettaValueInner::SExpr(children) => {
                for child in children.iter() {
                    if snapshot_mark_value(snapshot, child.inner_ptr() as *const u8, slot_size) {
                        worklist.push(child.inner_ptr());
                    }
                }
            }
            MettaValueInner::Conjunction(goals) => { /* same pattern */ }
            MettaValueInner::Error(_, details) => { /* mark details */ }
            MettaValueInner::Type(inner) => { /* mark inner */ }
            // Leaf values: Atom, Bool, Long, Float, String, Unit, Empty, Space, State, Memo
            _ => {}  // No children to trace
        }
    }
}
```

`snapshot_mark_value()` marks a value in the snapshot's mark bitmaps (not the live page's bitmaps) and returns `true` if the value was newly marked (not already marked from a previous trace path):

```rust
// gc_allocator.rs:1529-1546
fn snapshot_mark_value(snapshot: &mut GcSnapshot, ptr: *const u8, slot_size: usize) -> bool {
    for (page_idx, ps) in snapshot.page_snapshots.iter().enumerate() {
        let offset = (ptr as usize).wrapping_sub(ps.data_ptr as usize);
        if offset < ps.capacity * slot_size {
            let idx = offset / slot_size;
            if idx < ps.bump_count {
                let word = idx / 64;
                let bit = idx % 64;
                if (snapshot.marks[page_idx][word] & (1u64 << bit)) != 0 {
                    return false;  // Already marked
                }
                snapshot.marks[page_idx][word] |= 1u64 << bit;
                return true;  // Newly marked
            }
        }
    }
    false  // Not in any snapshot page (e.g., stack-allocated or in a post-snapshot page)
}
```

### Trace Example

Tracing the expression `(+ (* 2 3) 4)`:

```
Root: SExpr([+, SExpr([*, 2, 3]), 4])

Step 1: Mark root SExpr → worklist = [root]
Step 2: Pop root, trace children:
  Mark Atom("+")     → newly marked, but Atom is a leaf
  Mark SExpr([*,2,3])→ newly marked, push to worklist
  Mark Long(4)       → newly marked, but Long is a leaf
  worklist = [inner_sexpr]
Step 3: Pop inner_sexpr, trace children:
  Mark Atom("*")     → newly marked, leaf
  Mark Long(2)       → newly marked, leaf
  Mark Long(3)       → newly marked, leaf
  worklist = []
Step 4: Worklist empty — mark phase complete

Marked: root, "+", inner_sexpr, "*", 2, 3, 4  (7 values)

                    root (marked)
                   / |  \
                  /  |   \
           "+" (m) inner(m) 4 (m)
                  / |  \
                 /  |   \
           "*" (m) 2(m) 3(m)
```

## Sweep Phase

The sweep phase iterates every committed slot in the snapshot. For each slot, it checks the mark bitmap:

- **Marked** → live. Count toward `live_bytes` and `live_values`.
- **Unmarked and not in free set** → dead. Add to `dead_values` and extract associated data pointers via `collect_dead_data()`.

```rust
// gc_allocator.rs:1555-1587
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
                continue;  // Already free
            }

            if snapshot_is_marked(snapshot, page_idx, slot_idx) {
                response.live_values += 1;
                response.live_bytes += slot_size;
                let inner = unsafe { &*(ptr as *const MettaValueInner) };
                response.live_bytes += data_size_of(inner);
            } else {
                response.dead_values.push(ptr);
                let inner = unsafe { &*(ptr as *const MettaValueInner) };
                collect_dead_data(inner, &mut response.dead_data);
            }
        }
    }
    response
}
```

`collect_dead_data()` extracts data pointers from dead values so their data slots can also be freed:

```rust
// gc_allocator.rs:1707-1728
fn collect_dead_data(inner: &MettaValueInner, dead_data: &mut Vec<(*mut u8, usize)>) {
    match inner {
        MettaValueInner::Atom(s) if !s.is_empty() =>
            dead_data.push((s.as_ptr() as *mut u8, s.len())),
        MettaValueInner::String(s) if !s.is_empty() =>
            dead_data.push((s.as_ptr() as *mut u8, s.len())),
        MettaValueInner::SExpr(children) if !children.is_empty() =>
            dead_data.push((children.as_ptr() as *mut u8,
                           children.len() * size_of::<MettaValue>())),
        MettaValueInner::Conjunction(goals) if !goals.is_empty() =>
            dead_data.push((goals.as_ptr() as *mut u8,
                           goals.len() * size_of::<MettaValue>())),
        MettaValueInner::Error(msg, _) if !msg.is_empty() =>
            dead_data.push((msg.as_ptr() as *mut u8, msg.len())),
        _ => {}
    }
}
```

The result is a `GcResponse` containing `dead_values`, `dead_data`, and the `snapshot_epoch` used for TOCTOU filtering.

## Epoch-Based TOCTOU Prevention

### The Problem

Without epochs, a time-of-check-to-time-of-use (TOCTOU) race can cause use-after-free:

```
Time    Eval Thread                     GC Thread
────    ──────────                      ─────────
T0      Allocate slot S (bump)
T1      Drop root for S
T2      Free slot S (push to free list)
T3      Build snapshot:
          roots = [... no S ...]
          free_set = {S}
T4      <continues evaluating>          Receive snapshot
T5      Re-allocate S from free list    Mark phase (S not a root)
        Write new value into S
T6      S is now live!                  Sweep phase:
                                          S is in free_set → skip? No!
                                          S was freed THEN re-allocated.
                                          If GC says "S is dead" → UAF!
```

The subtlety: at T3, slot S was in the free set. But by T5, the evaluator re-allocated S from the free list and wrote a new live value into it. If the GC naively treats "free at snapshot time" as "dead", it will free a live value.

In practice, MeTTaTron uses an empty free set (because the Treiber stack can't be iterated), so the scenario is slightly different: S appears as "unmarked allocated" during sweep, which the GC reports as dead. The result is the same — the live value in S would be incorrectly freed.

### The Fix: Epoch Tagging

Every free-list re-allocation increments a monotonic epoch counter and tags the slot:

```rust
// In ValueAllocator::alloc() — gc_allocator.rs:626-638
if let Some(ptr) = self.free_list.pop() {
    // EPOCH: increment and tag the re-allocated slot
    let new_epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
    let pages = self.pages.read().expect("poisoned");
    for page in pages.iter() {
        if let Some(idx) = page.slot_index(ptr as *const u8, self.slot_size) {
            page.set_slot_epoch(idx, new_epoch);
            page.live_count.fetch_add(1, Ordering::Relaxed);
            break;
        }
    }
    return ptr;
}
```

Bump allocations don't need epoch tagging because post-snapshot bump allocations are in slots that the snapshot cannot see (they have `slot_idx >= ps.bump_count`).

When the snapshot is built, the current epoch is captured:

```rust
snapshot_epoch: self.values.epoch.load(Ordering::Acquire),
```

When the GC response is processed, each dead value is checked:

```rust
// gc_allocator.rs:1340-1363
for &ptr in &response.dead_values {
    let mut skip = false;
    for page in pages.iter() {
        if let Some(idx) = page.slot_index(ptr as *const u8, self.values.slot_size) {
            if page.slot_epoch(idx) > response.snapshot_epoch {
                skip = true;  // Re-allocated after snapshot — LIVE, not dead
            }
            break;
        }
    }
    if !skip {
        // Truly dead — free it
        self.values.free_list.push(ptr);
    }
}
```

### Worked Example with Epochs

```
Time    Eval Thread                     GC Thread         Epoch
────    ──────────                      ─────────         ─────
T0      Allocate slot S (bump)                             0
T1      Drop root for S                                    0
T2      Free slot S                                        0
T3      Build snapshot:
          snapshot_epoch = 0
          roots = [...]
          S appears as unmarked
T4                                      Receive snapshot   0
T5      Re-allocate S from free list:                      1
          epoch → 1
          S.epoch = 1
T6      S holds live value                                 1
T7                                      Sweep: S unmarked  1
                                          → dead_values = [S]
                                          → response.snapshot_epoch = 0
T8      Process GC response:
          S.epoch (1) > snapshot_epoch (0)
          → SKIP! S is live!
```

The epoch filter at T8 correctly identifies that S was re-allocated after the snapshot and prevents freeing it.

## GC Thread Protocol

The GC runs on a dedicated background thread named `mettatron-gc`, communicating with the evaluation thread via two `mpsc` channels:

```
┌──────────────────┐                    ┌──────────────────┐
│   Eval Thread    │                    │   GC Thread      │
│                  │  GcRequest         │                  │
│  maybe_trigger   │──(Collect(snap))──→│  gc_thread_main  │
│    _gc()         │                    │                  │
│                  │  GcResponse        │  mark_snapshot() │
│  process_gc      │←─(dead, epoch)────│  sweep_snapshot()│
│    _response()   │                    │                  │
└──────────────────┘                    └──────────────────┘
```

### GcThread Structure

```rust
// gc_thread.rs:73-80
pub struct GcThread {
    request_tx: mpsc::Sender<GcRequest>,
    response_rx: mpsc::Receiver<GcResponse>,
    handle: Option<JoinHandle<()>>,
}
```

### GC Thread Main Loop

The GC thread blocks on `recv()`, waiting for requests:

```rust
// gc_thread.rs:157-183
fn gc_thread_main(
    request_rx: mpsc::Receiver<GcRequest>,
    response_tx: mpsc::Sender<GcResponse>,
) {
    loop {
        match request_rx.recv() {
            Ok(GcRequest::Collect(mut snapshot)) => {
                mark_snapshot(&mut snapshot);
                let response = sweep_snapshot(&snapshot);
                if response_tx.send(response).is_err() {
                    break;  // Eval thread gone
                }
            }
            Ok(GcRequest::Shutdown) | Err(_) => break,
        }
    }
}
```

### Integration with the Trampoline

The evaluation trampoline calls `SessionContext::maybe_gc()` every 256 iterations. This calls `maybe_trigger_gc()`, which performs three steps:

```rust
// gc_allocator.rs:1082-1098
pub fn maybe_trigger_gc() -> bool {
    let gc = global_gc_thread().lock().expect("poisoned");

    // Step 1: Drain any pending GC response from previous cycle
    let alloc = global_allocator();
    if let Some(response) = gc.try_recv_response() {
        alloc.process_gc_response(&response);
        alloc.release_empty_pages();
    }

    // Step 2: Check if new GC was requested
    if GC_REQUESTED.compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return false;  // No GC requested
    }

    // Step 3: Collect roots, build snapshot, send to GC thread
    trigger_gc_cycle_locked(&gc)
}
```

### Sequence Diagram

```
Eval Thread                          GC Thread                    GC Cron
    │                                    │                            │
    │  (evaluating expressions)          │ (blocked on recv)          │
    │                                    │                            │
    │                                    │            ┌───────────────┤
    │                                    │            │ Monitor: rate │
    │                                    │            │ > 100k/s      │
    │                                    │            │ → set flag    │
    │                                    │            └───────────────┤
    │                                    │                            │
    ├─ maybe_trigger_gc() ───────────────┤                            │
    │  1. try_recv_response → None       │                            │
    │  2. GC_REQUESTED = true → false    │                            │
    │  3. collect_all_roots()            │                            │
    │  4. build_snapshot(roots)          │                            │
    │  5. request_gc(snapshot) ─────────→│                            │
    │                                    │                            │
    │  (continues evaluating)            ├─ mark_snapshot()           │
    │  (allocates new values freely)     │  (traces from roots)       │
    │                                    │                            │
    │                                    ├─ sweep_snapshot()          │
    │                                    │  (builds dead set)         │
    │                                    │                            │
    │  (256 iterations later)            │                            │
    ├─ maybe_trigger_gc() ───────────────┤                            │
    │  1. try_recv_response → Some(resp) │                            │
    │  2. process_gc_response(resp)      │                            │
    │     - epoch filter dead_values     │                            │
    │     - free surviving dead slots    │                            │
    │     - free dead data slots         │                            │
    │  3. release_empty_pages()          │                            │
    │     - munmap pages w/ live_count=0 │                            │
    │  4. GC_REQUESTED = false → no-op   │                            │
    │                                    │                            │
    │  (continues evaluating)            │ (blocked on recv)          │
    │                                    │                            │
```

## Page Release

After processing a GC response, empty pages can be released:

```rust
// gc_allocator.rs:1456-1461
pub fn release_empty_pages(&self) {
    self.values.release_empty_pages();
    for dc in &self.data_classes {
        dc.release_empty_pages();
    }
}
```

A page is eligible for release when:
- `live_count <= 0`: No live values remain in the page
- `bump_count > 0`: The page has been used (don't release fresh pages)

When a page is dropped, its `MmapPage::drop()` calls `munmap`, and the OS immediately reclaims the physical memory. RSS decreases visibly.

**Why `live_count` is `AtomicIsize` (signed):** Concurrent increments and decrements from multiple threads can temporarily produce a negative value. For example, if thread A decrements and thread B hasn't yet incremented for its concurrent allocation, the count may briefly be -1. The signed type prevents unsigned underflow (which would wrap to a huge positive number and prevent page release).

## GC Cron Manager

The cron manager runs on a third dedicated thread (`mettatron-gc-cron`), monitoring allocation rate and requesting GC when appropriate.

### Architecture

The cron manager is a reactive state machine with a `BinaryHeap` priority queue (min-heap via reversed `Ord`):

```
┌────────────────────────────────────────────────────────────────┐
│                    Cron State Machine                          │
│                                                                │
│  CheckTasks ──TaskDue──→ ExecuteTask ──→ CheckTasks            │
│      │                                                         │
│      └──NoTasksDue──→ Sleeping (50ms chunks) ──→ CheckTasks    │
│      │                                                         │
│      └──TerminationRequested──→ Terminated                     │
└────────────────────────────────────────────────────────────────┘
```

### Scheduled Tasks

| Task | Interval | Purpose |
|------|----------|---------|
| Memory Monitor | 100 ms | Read `alloc_count_atomic`, compute allocation rate, set `GC_REQUESTED` if rate > 100,000 allocs/s |
| Stats Reporter | 5 s | Log committed bytes and total allocations to stderr (enabled by `METTA_GC_STATS=1`) |

### Memory Monitor Logic

```rust
// gc_cron.rs:382-403
fn execute_memory_monitor(
    _committed_bytes: &AtomicUsize,
    alloc_count: &AtomicU64,
    gc_requested: &AtomicBool,
    monitor: &mut MonitorState,
) {
    let now = Instant::now();
    let current_alloc_count = alloc_count.load(Ordering::Relaxed);
    let elapsed = now.duration_since(monitor.prev_poll_time);

    let delta = current_alloc_count.saturating_sub(monitor.prev_alloc_count);
    let rate = (delta as f64 / elapsed.as_secs_f64()) as u64;

    if rate > ALLOC_RATE_THRESHOLD {  // 100,000 allocs/s
        gc_requested.store(true, Ordering::Relaxed);
    }

    monitor.prev_alloc_count = current_alloc_count;
    monitor.prev_poll_time = now;
}
```

### Lock-Free Communication

The cron manager communicates with the evaluation thread entirely through atomics — no channels, no locks, no heap allocations on the hot path:

- **Reads** `alloc_count_atomic` and `committed_bytes_atomic` with `Relaxed` ordering (eventual visibility is sufficient for rate detection)
- **Writes** `GC_REQUESTED` with `Relaxed` ordering (the evaluation thread reads it with `AcqRel` CAS in `maybe_trigger_gc()`)
- **Shutdown** via `terminating: Arc<AtomicBool>` with `Release`/`Acquire` ordering

The cron thread sleeps in 50ms chunks so it can respond to shutdown requests within 50ms.

## What's Next

[Chapter 5](05-integration.md) describes how the allocator and GC integrate with the rest of MeTTaTron — the `MettaValue` type, the `GcFactory`, the evaluation loop, the JIT compiler, and deserialization.
