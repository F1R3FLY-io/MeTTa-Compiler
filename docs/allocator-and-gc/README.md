# Slab Allocator and Garbage Collector

MeTTaTron uses a custom lock-free slab allocator with snapshot-based mark-sweep garbage collection. The system provides concurrent, lock-free allocation across threads, zero-cost value cloning (pointer copy), incremental reclamation of dead values without stop-the-world pauses, and guaranteed RSS decrease when pages are released back to the OS via `munmap`. The design has been formally verified with TLA+, which uncovered and proved fixes for four concurrency bugs.

## Reading Order

The chapters build progressively. Each chapter assumes understanding of prior ones.

| # | Chapter | What You'll Learn |
|---|---------|-------------------|
| 1 | [Motivation and Overview](01-motivation-and-overview.md) | Why a custom allocator, design goals, architecture overview, key invariants |
| 2 | [The Slab Allocator](02-slab-allocator.md) | Page layout, slot management, size classes, bump allocation, the `SlabAllocator` API |
| 3 | [Lock-Free Concurrency](03-lock-free-concurrency.md) | Compare-and-swap, the ABA problem, Treiber stacks, atomic bump allocation, memory ordering |
| 4 | [The Garbage Collector](04-garbage-collector.md) | Mark-sweep algorithm, snapshots, epoch-based TOCTOU prevention, GC thread protocol, cron manager |
| 5 | [System Integration](05-integration.md) | `GcFactory`, `MettaValue`, root registry, evaluation loop, JIT runtime, deserialization |
| 6 | [Formal Verification](06-formal-verification.md) | TLA+ models, four bugs discovered, fixes verified, relationship to source code |
| 7 | [Backpressure](07-backpressure.md) | Graduated allocation throttling, Tier 1/2 application, TLA+ verification |

## Quick Reference

### Key Entry Points

| Function | Location | Purpose |
|----------|----------|---------|
| `global_allocator()` | `gc_allocator.rs:1043` | Get `&'static SlabAllocator` (lazy init) |
| `global_factory()` | `gc_allocator.rs:1048` | Get `GcFactory` backed by global allocator |
| `global_gc_thread()` | `gc_allocator.rs:1065` | Get `&'static Mutex<GcThread>` (lazy spawn) |
| `maybe_quiescent_gc()` | `gc_allocator.rs` | Trigger GC at quiescent point (no active evaluators) |
| `maybe_process_gc_response()` | `gc_allocator.rs` | Process pending GC response, update threshold + backpressure |
| `apply_backpressure_tier1()` | `gc_allocator.rs` | Graduated yield/sleep during eval (called every 256 iterations) |
| `apply_backpressure_tier2()` | `gc_allocator.rs` | Block at MAX level until GC completes (between expressions) |
| `request_gc()` | `gc_allocator.rs` | Set `GC_REQUESTED` flag for next quiescent point |
| `collect_all_roots()` | `gc_allocator.rs:1152` | Gather roots from all registered `RootProvider`s |
| `register_root_provider()` | `gc_allocator.rs:1140` | Register a `Weak<dyn RootProvider>` with the root registry |

### Key Types

| Type | Size | Purpose |
|------|------|---------|
| `MettaValue` | 8 bytes (pointer) | Copy-semantic MeTTa value |
| `GcFactory` | 8 bytes (pointer) | `MettaValueFactory` backed by slab allocator |
| `SlabAllocator` | ~800 bytes | Top-level allocator (1 `ValueAllocator` + 9 `DataClassAllocator`s) |
| `GcSnapshot` | Variable | Frozen allocator state sent to GC thread |
| `GcResponse` | Variable | Dead values + epoch returned from GC thread |

## Source File to Chapter Mapping

| Source File | Primary Chapter | Description |
|-------------|----------------|-------------|
| `src/backend/models/gc_allocator.rs` | 2, 3, 4, 5 | SlabAllocator, Treiber stack, mark-sweep, GcFactory, root registry |
| `src/backend/models/gc_thread.rs` | 4 | GcThread, GcRequest/GcResponse protocol |
| `src/backend/models/gc_cron.rs` | 4 | CronStateMachine, memory monitor, allocation rate detection |
| `src/backend/models/metta_value.rs` | 5 | MettaValue, MettaValueInner definitions |
| `src/backend/models/metta_value_trait.rs` | 5 | MettaValueFactory trait |
| `src/backend/models/metta_state.rs` | 5 | MettaState session container |
| `src/backend/eval/trampoline/session_context.rs` | 5 | SessionContext, `maybe_gc()` trigger |
| `src/backend/environment/generic.rs` | 5 | RootProvider impl, `try_register_env_roots()` |
| `src/backend/bytecode/jit/runtime/` | 5 | JIT runtime functions using arena pointer |
| `tla/SlabGC.tla` | 6 | Original model (3 bugs) |
| `tla/SlabGC_Reactive.tla` | 6 | Fixed model (epoch + snapshot) |
| `tla/SlabGC_Pages.tla` | 6 | Page-level model (4th bug) |
| `tla/SlabGC_Quiescent.tla` | 6, 7 | Multi-thread quiescent-state protocol + backpressure model |
