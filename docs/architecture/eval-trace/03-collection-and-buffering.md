# 03 — TraceCollector Architecture

**Source**: `src/backend/trace/collector.rs`

## Overview

The `TraceCollector` is the central component that receives trace events
from all evaluation threads and writes them to a single `.mtrace` file.
It uses per-thread batching to minimize lock contention on the shared
output file.

## Thread-Safety Model

```
  Thread 0 (main eval)         Thread 1 (parallel eval)     Thread 2 (parallel eval)
  ┌─────────────────────┐      ┌─────────────────────┐      ┌─────────────────────┐
  │   ThreadBuffer      │      │   ThreadBuffer      │      │   ThreadBuffer      │
  │ ┌─────────────────┐ │      │ ┌─────────────────┐ │      │ ┌─────────────────┐ │
  │ │ events[0..1024] │ │      │ │ events[0..1024] │ │      │ │ events[0..1024] │ │
  │ │ seq: u64        │ │      │ │ seq: u64        │ │      │ │ seq: u64        │ │
  │ │ thread_id: u32  │ │      │ │ thread_id: u32  │ │      │ │ thread_id: u32  │ │
  │ └─────────────────┘ │      │ └─────────────────┘ │      │ └─────────────────┘ │
  └──────────┬──────────┘      └──────────┬──────────┘      └──────────┬──────────┘
             │ flush when full             │ flush when full             │ flush when full
             │                             │                             │
             └─────────────┐   ┌───────────┘   ┌────────────────────────┘
                           ▼   ▼               ▼
                    ┌──────────────────────────────┐
                    │  Mutex<SharedState>           │
                    │ ┌──────────────────────────┐ │
                    │ │ BufWriter<File> (256 KB)  │ │
                    │ │ FileTable                 │ │
                    │ │ event_count: u64          │ │
                    │ └──────────────────────────┘ │
                    └──────────────────────────────┘
                               │
                               ▼
                        .mtrace file
```

## TraceCollector Structure

```rust
pub struct TraceCollector {
    shared: Mutex<SharedState>,    // Output file + file table + count
    start: Instant,                // Trace start time (for timestamp_ns)
    global_seq: AtomicU64,         // Global sequence counter (unused — per-thread seq used)
    next_thread_id: AtomicU64,     // Next thread ID to assign
}
```

The collector is always wrapped in `Arc<TraceCollector>` and shared
across all evaluation threads.

### SharedState

```rust
struct SharedState {
    writer: BufWriter<File>,    // 256 KB buffered writer
    file_table: FileTable,      // HashMap<String, u16> for path interning
    event_count: u64,           // Total events written to file
}
```

## Per-Thread ThreadBuffer

```rust
struct ThreadBuffer {
    events: Vec<TraceEvent>,     // Pre-allocated to BATCH_SIZE (1024)
    seq: u64,                    // Per-thread monotonic sequence counter
    thread_id: u32,              // Assigned on first access
}
```

The thread buffer is stored in a `thread_local!` `RefCell<Option<ThreadBuffer>>`.
It is lazily initialized on the first `emit()` call from each thread. The
thread ID is assigned atomically from `TraceCollector::next_thread_id`.

### Batch Size

```rust
const BATCH_SIZE: usize = 1024;
```

Events are accumulated in the thread-local buffer until it reaches 1024
events, at which point the entire batch is flushed under a single mutex
acquisition.

## Emission Hot Path

### `emit()` — Live MettaValue input

**Source**: `src/backend/trace/collector.rs:116`

```
emit(tier, depth, &input, &[outputs], expr_span, kind)
  │
  ├─ trace_value(&input) → TraceValue        (owned snapshot)
  ├─ outputs.iter().map(trace_value) → Vec    (owned snapshots)
  │
  └─ emit_converted(tier, depth, input_tv, outputs_tv, expr_span, kind)
```

The `input` and `outputs` parameters are live `&MettaValue` references.
They are immediately converted to fully-owned `TraceValue` snapshots
(deep copy of all strings and nested structures). This ensures GC safety —
the GC can freely reclaim the original slab-allocated values after this
point. See [`docs/design/eval-trace/gc-safety.md`](../../design/eval-trace/gc-safety.md).

### `emit_converted()` — Pre-converted TraceValue

**Source**: `src/backend/trace/collector.rs:135`

Used when the caller already has `TraceValue` data (e.g., lifecycle events
like `EvalStart`/`EvalEnd` where the input is `TraceValue::Unit`).

Steps:

1. Compute `timestamp_ns` from `self.start.elapsed()`
2. Access thread-local buffer (lazy-init with new thread ID if needed)
3. Increment per-thread `seq` counter
4. Construct `TraceEvent` and push to buffer
5. If buffer length reaches `BATCH_SIZE`:
   a. `mem::replace` the events vec with a fresh `Vec::with_capacity(BATCH_SIZE)`
   b. **Drop the `RefCell` borrow** before acquiring the mutex
   c. Call `flush_batch(batch)`

### `flush_batch()` — Write to file

**Source**: `src/backend/trace/collector.rs:231`

```rust
fn flush_batch(&self, batch: Vec<TraceEvent>) {
    let mut shared = self.shared.lock().expect("...");
    for event in &batch {
        format::write_event(&mut shared.writer, event)?;
        shared.event_count += 1;
    }
}
```

Acquires the shared mutex once per batch (not per event). Each event is
length-prefix encoded and written via the 256 KB `BufWriter`.

## File Table Interning

**Source**: `src/backend/trace/convert.rs:314`

```rust
pub struct FileTable {
    paths: Vec<String>,
    index: HashMap<String, u16>,
}
```

File paths are interned to `u16` IDs to avoid repeating full path strings
in every `TraceSpan`. The `FileTable` is stored inside `SharedState` and
accessed via `TraceCollector::intern_file()`, which acquires the shared
mutex.

At finalization, `FileTable::into_paths()` returns the `Vec<String>` in
ID order, which is serialized into the trace file footer.

## Finalization

**Source**: `src/backend/trace/collector.rs:194`

`TraceCollector::finalize(self: Arc<Self>)`:

1. Flush the calling thread's buffer (via `THREAD_BUF.with`)
2. Attempt `Arc::try_unwrap(self)` to get owned access
   - If other threads still hold references, log the count and return
3. Move `SharedState` out of the `Mutex`
4. Extract the file table paths
5. Call `format::write_footer(writer, event_count, file_table)`
6. Flush the `BufWriter`
7. Return the event count

### Thread-Local Buffer Limitation

Only the calling thread's buffer is flushed in `finalize()`. If other
threads emitted events that haven't been batch-flushed, those events may
be lost. In practice, this is not an issue because:

- MeTTaTron's evaluation model ensures all parallel evaluations complete
  before the main thread calls `finalize()`
- Each thread's buffer is flushed every 1024 events during evaluation
- Only the tail (< 1024 events) of each non-main thread could be lost

## Span Conversion

**Source**: `src/backend/trace/convert.rs:358`

```rust
pub fn trace_span(span: &Span, file_id: u16) -> TraceSpan {
    TraceSpan {
        file_id,
        start_row: span.start.row as u32,
        start_col: span.start.column as u32,
        end_row: span.end.row as u32,
        end_col: span.end.column as u32,
    }
}
```

Converts the evaluator's `ir::Span` (which includes byte offsets) to a
compact `TraceSpan` (which stores only row/col coordinates and a file ID).

## Bindings Conversion

**Source**: `src/backend/trace/convert.rs:373`

```rust
pub fn trace_bindings<V, I>(bindings: I) -> Vec<(String, TraceValue)>
where
    I: IntoIterator<Item = (String, MettaValue)>,
```

Snapshots a set of variable bindings into owned `(String, TraceValue)`
pairs for inclusion in `RuleApplication` events.
