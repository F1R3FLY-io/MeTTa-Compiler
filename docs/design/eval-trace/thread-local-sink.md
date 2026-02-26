# Thread-Local Collector for VM and JIT

## Problem

The bytecode VM (`GenericBytecodeVM<V, F, E>`) and JIT execution paths
do not have access to an `EvalContext`. The tree-walker naturally threads
the `TraceCollector` through `SessionContext`, but the VM and JIT have
their own execution contexts with different generic type parameter systems.

Threading an `Arc<TraceCollector>` through all VM/JIT generic parameters
would cascade into 40+ file modifications:

1. Add `trace_collector: Option<Arc<TraceCollector>>` field to `GenericBytecodeVM`
2. Propagate through all `impl<V, F, E>` blocks
3. Update every external call site: `execute_generic()`, `execute_generic_simple()`
4. Add to `JitContext` and all its constructors
5. Add to `CompiledJitCode` execution functions
6. Thread through all the native function call paths

This is a large, invasive change for an optional feature.

## Alternative Considered: VM/JIT Struct Field

Add a `trace_collector` field directly to `GenericBytecodeVM`:

```rust
pub struct GenericBytecodeVM<V, F, E> {
    // ...
    #[cfg(feature = "eval-trace")]
    trace_collector: Option<Arc<TraceCollector>>,
}
```

**Problems:**
- `GenericBytecodeVM` is generic over `V, F, E`. Adding a trace collector
  field requires updating every `impl` block and every constructor.
- The `F: Copy` cascade: `EvalContext` requires `Factory: Copy`. Adding
  this constraint to `GenericBytecodeVM` cascades to all impl blocks
  and external callers. (This was learned the hard way during the
  type-driven optimization work.)
- The JIT would need the same field in `JitContext` and all its paths.

## Solution: Scoped Thread-Local Sink

**Source**: `src/backend/trace/thread_local_sink.rs`

A thread-local `RefCell<Option<Arc<TraceCollector>>>` that is:

1. **Set** before calling into bytecode VM or JIT
2. **Read** by instrumentation points inside the VM/JIT
3. **Cleared** after the bytecode VM or JIT call returns

### Implementation

```rust
thread_local! {
    static THREAD_TRACE_COLLECTOR: RefCell<Option<Arc<TraceCollector>>>
        = const { RefCell::new(None) };
}

pub fn set_thread_trace_collector(collector: &Arc<TraceCollector>) {
    THREAD_TRACE_COLLECTOR.with(|cell| {
        *cell.borrow_mut() = Some(Arc::clone(collector));
    });
}

pub fn clear_thread_trace_collector() {
    THREAD_TRACE_COLLECTOR.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

pub fn with_thread_trace_collector<R>(
    f: impl FnOnce(&TraceCollector) -> R,
) -> Option<R> {
    THREAD_TRACE_COLLECTOR.with(|cell| {
        let borrow = cell.borrow();
        borrow.as_ref().map(|tc| f(tc))
    })
}
```

### Scoping Pattern

In `eval_inner_with_trace()` (`src/backend/eval/mod.rs:151`):

```rust
// Set before dispatch
set_thread_trace_collector(collector);

// Try JIT Stage 2
if /* jit2 ready */ {
    collector.emit_converted(/* TierDispatch */);
    match execute_jit_arena_with_env(...) {
        Ok((results, env)) => {
            clear_thread_trace_collector();  // clear on success
            return (results, env);
        }
        Err(_) => {} // fallthrough to next tier
    }
}

// Try JIT Stage 1
// ... same pattern ...

// Try Bytecode VM
// ... same pattern ...

// Fallback: Tree-Walker
let result = eval_trampoline_with_trace(...);
clear_thread_trace_collector();  // clear on return
result
```

The collector is cleared on every return path, including early returns
from successful JIT/bytecode execution.

### Usage in VM Opcode Handlers

```rust
#[cfg(feature = "eval-trace")]
{
    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
    with_thread_trace_collector(|tc| {
        tc.emit_converted(
            trace_format::TraceTier::BytecodeVM, 0,
            input_tv, outputs_tv, None,
            trace_format::TraceEventKind::GroundedOp { op_name, args },
        );
    });
}
```

If tracing is not active (no collector set), `with_thread_trace_collector`
returns `None` and the closure is never called.

## Why This Is Safe

1. **Each thread has its own thread-local**: No cross-thread interference.
2. **`Arc<TraceCollector>` is cloned**: The thread-local holds its own
   `Arc`, keeping the collector alive. No dangling references.
3. **Scoped set/clear**: The collector is set before dispatch and cleared
   after return, matching the call/return pattern exactly.
4. **No lifetime issues**: `Arc` is owned, not borrowed. Thread-local
   storage has `'static` lifetime.
5. **Feature-gated**: All code is inside `#[cfg(feature = "eval-trace")]`
   blocks. When the feature is disabled, the thread-local doesn't exist.

## Trade-Offs

| Aspect | Thread-Local Sink | Struct Field |
|--------|-------------------|-------------|
| Lines of code | ~60 (thread_local_sink.rs) | ~200+ across 40+ files |
| Invasiveness | None (no type changes) | High (generic param cascade) |
| Runtime cost | TLS lookup per event (~5ns) | Direct field access (~0ns) |
| Correctness risk | Must clear on all paths | Automatic (field lifetime) |
| Maintainability | Self-contained module | Spread across codebase |

The ~5ns TLS lookup overhead per event is negligible compared to the
~100–500ns postcard serialization cost per event.
