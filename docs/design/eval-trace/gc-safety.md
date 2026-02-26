# Owned Snapshots for Garbage Collection Safety

## Problem

`MettaValue` holds `&'static MettaValueInner`, a reference to
slab-allocated data. The slab allocator's garbage collector
(mark-sweep with quiescent-state protocol) can reclaim any
`MettaValueInner` that is not reachable from registered roots.

If trace events held `MettaValue` references (or raw `*const
MettaValueInner` pointers), the GC could free the underlying data
between event emission and file write:

```
Thread A (evaluator)           GC Thread
─────────────────────          ──────────────
emit event with &MettaValue
  ↓
push to ThreadBuffer
                               mark-sweep begins
                               MettaValue not in root set
                               (ThreadBuffer not registered)
                               → FREE the slab page
  ↓
flush_batch()
  write_event(event)
  → SEGFAULT: inner pointer is dangling
```

The `ThreadBuffer` is not a registered GC root (and cannot be, since
thread-local storage is not compatible with the root registry's
`Weak<dyn RootProvider>` model).

## Solution: Owned TraceValue Snapshots

All `MettaValue` references are converted to fully-owned `TraceValue`
at emission time — before the event enters the `ThreadBuffer`.

**Source**: `src/backend/trace/collector.rs:116`

```rust
pub fn emit(&self, tier: TraceTier, depth: u32,
            input: &MettaValue, outputs: &[MettaValue],
            expr_span: Option<TraceSpan>, kind: TraceEventKind) {
    let input_tv = trace_value(input);          // deep copy
    let outputs_tv: Vec<TraceValue> = outputs
        .iter()
        .map(trace_value)
        .collect();                             // deep copy each

    self.emit_converted(tier, depth, input_tv, outputs_tv, expr_span, kind);
}
```

`trace_value()` performs a deep copy:

- `Atom(&'static str)` → `Atom(String)` — string cloned
- `String(&'static str)` → `String(String)` — string cloned
- `SExpr(&'static [MettaValue])` → `SExpr(Vec<TraceValue>)` — recursively deep-copied
- `Error(msg, details)` → `Error(String, Box<TraceValue>)` — message cloned, details deep-copied
- `Type(inner)` → `Type(Box<TraceValue>)` — deep-copied
- `Quoted(inner)` → `Quoted(Box<TraceValue>)` — deep-copied
- Leaf types (`Bool`, `Long`, `Float`, `Unit`, `Empty`) — trivially copied

After conversion, the `TraceValue` is a completely self-contained,
heap-allocated value with no references to slab memory. The GC can
freely operate without any coordination with the trace system.

## Trade-Off: Allocation Overhead

Converting to owned `TraceValue` allocates:

- One `String` per `Atom` or `String` leaf
- One `Vec<TraceValue>` per `SExpr` node
- One `Box<TraceValue>` per `Error`, `Type`, or `Quoted` wrapper

For a typical expression like `(+ (f 3) (g 4 5))`:

```
SExpr([
    Atom("+"),           → String::from("+")     (1 alloc, 1 byte)
    SExpr([
        Atom("f"),       → String::from("f")     (1 alloc, 1 byte)
        Long(3),         → Long(3)               (0 allocs)
    ]),                  → Vec::with_capacity(2)  (1 alloc, 2 × size)
    SExpr([
        Atom("g"),       → String::from("g")     (1 alloc, 1 byte)
        Long(4),         → Long(4)               (0 allocs)
        Long(5),         → Long(5)               (0 allocs)
    ]),                  → Vec::with_capacity(3)  (1 alloc, 3 × size)
])                       → Vec::with_capacity(3)  (1 alloc, 3 × size)
```

Total: 6 allocations for this expression. At ~100–300ns per event
(including allocation and string copying), this is acceptable for a
tracing system that is explicitly opted into.

## Interaction with GC Safepoints

The `GcSafepoint` trace event kind is emitted **during** GC safepoints,
after root collection but before the safepoint wait:

```
generic_trampoline.rs:152–156:
    #[cfg(feature = "eval-trace")]
    let _root_count = roots.len() as u32;
    ctx.perform_safepoint(roots);
    #[cfg(feature = "eval-trace")]
    {
        if let Some(tc) = ctx.trace_collector() {
            tc.emit_converted(
                TraceTier::TreeWalker, depth,
                TraceValue::Unit, vec![], None,
                TraceEventKind::GcSafepoint {
                    root_count: _root_count,
                    allocation_delta_bytes: ...,
                },
            );
        }
    }
```

This is safe because:

1. The `GcSafepoint` event uses `emit_converted` with pre-constructed
   `TraceValue::Unit` — no `MettaValue` references are converted.
2. The `root_count` is captured as a `u32` before `perform_safepoint()`
   moves the roots.
3. The `allocation_delta_bytes` is a scalar value.

## Alternatives Considered

### Register ThreadBuffer as GC Root

Make `ThreadBuffer` implement `RootProvider` and register it with the
root registry. The GC would then know to keep all values referenced by
pending trace events alive.

**Rejected because:**

- Thread-local storage is not compatible with `Weak<dyn RootProvider>`.
  The root registry uses `Weak` to auto-clean dead entries, but
  thread-locals don't have `Arc` wrappers.
- Would tie the GC's performance to the trace system — more roots
  means longer mark phases.
- Adds coupling between the trace system and the GC.

### Defer Conversion to Flush Time

Store `MettaValue` in the `ThreadBuffer` and convert to `TraceValue`
only during `flush_batch()`.

**Rejected because:**

- The time between `emit()` and `flush_batch()` can be up to 1024
  events — potentially spanning multiple GC safepoints.
- GC could reclaim values during this window.
- Would require either GC root registration (same problems as above)
  or preventing GC during the window (unacceptable for latency).
