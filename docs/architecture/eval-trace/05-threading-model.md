# 05 — Threading Model

The trace system uses two different patterns to provide the `TraceCollector`
to instrumentation points, depending on which evaluation tier is active.

## Pattern 1: EvalContext (Tree-Walker)

The tree-walker evaluation path threads the `TraceCollector` through the
`EvalContext` trait.

### EvalContext Trait

**Source**: `src/backend/eval/trampoline/context.rs:92`

```rust
pub trait EvalContext {
    // ...
    #[cfg(feature = "eval-trace")]
    fn trace_collector(&self) -> Option<&TraceCollector>;
}
```

When the `eval-trace` feature is enabled, every `EvalContext` implementor
provides an optional trace collector reference. When the feature is
disabled, this method does not exist at all — zero overhead.

### SessionContext

**Source**: `src/backend/eval/trampoline/session_context.rs:74`

```rust
pub struct SessionContext {
    // ...
    #[cfg(feature = "eval-trace")]
    trace_collector: Option<Arc<TraceCollector>>,
}
```

The `SessionContext` is the concrete `EvalContext` implementation used
during evaluation. It optionally holds an `Arc<TraceCollector>` when
tracing is active.

### How It Flows

```
main.rs
  │  --trace /tmp/trace.mtrace
  ▼
TraceCollector::new("/tmp/trace.mtrace", "input.metta")
  │
  ▼
Arc<TraceCollector>
  │
  ├─ eval_with_trace(value, env, state, &collector)
  │    │
  │    ▼
  │  eval_inner_with_trace(value, env, state, &collector)
  │    │
  │    ├─ set_thread_trace_collector(&collector)  ← for VM/JIT
  │    │
  │    └─ eval_trampoline_with_trace(value, env, state, &collector)
  │         │
  │         ▼
  │       SessionContext::new_with_trace(state, Arc::clone(&collector))
  │         │
  │         ▼
  │       generic_trampoline_loop(ctx)
  │         │
  │         ├─ ctx.trace_collector()  → Some(&TraceCollector)
  │         │   └─ tc.emit(...)
  │         │
  │         └─ (on return) clear_thread_trace_collector()
  │
  └─ collector.finalize()
```

### trace_emit_ctx! Macro

**Source**: `src/backend/trace/macros.rs:31`

```rust
macro_rules! trace_emit_ctx {
    ($ctx:expr, $tier:expr, $depth:expr, $input:expr,
     $outputs:expr, $expr_span:expr, $kind:expr) => {
        #[cfg(feature = "eval-trace")]
        {
            if let Some(tc) = $ctx.trace_collector() {
                tc.emit($tier, $depth, $input, $outputs, $expr_span, $kind);
            }
        }
    };
}
```

The `#[cfg(feature = "eval-trace")]` inside the macro body ensures
the entire block compiles to nothing when the feature is disabled.
This is a **compile-time zero-cost gate** — no runtime branch, no
dead code in the binary.

### trace_emit! Macro

**Source**: `src/backend/trace/macros.rs:20`

```rust
macro_rules! trace_emit {
    ($collector:expr, $tier:expr, $depth:expr, $input:expr,
     $outputs:expr, $expr_span:expr, $kind:expr) => {
        if let Some(tc) = $collector {
            tc.emit($tier, $depth, $input, $outputs, $expr_span, $kind);
        }
    };
}
```

This variant takes an `Option<&TraceCollector>` directly, for callsites
that already have the collector reference.

## Pattern 2: Thread-Local Sink (Bytecode VM / JIT)

**Source**: `src/backend/trace/thread_local_sink.rs`

The bytecode VM and JIT execution paths do not have access to an
`EvalContext`. Threading a trace collector through their generic type
parameters would cascade into 40+ files. Instead, a scoped thread-local
pattern is used.

### Thread-Local Storage

```rust
thread_local! {
    static THREAD_TRACE_COLLECTOR: RefCell<Option<Arc<TraceCollector>>>
        = const { RefCell::new(None) };
}
```

### API

```rust
/// Set before dispatching to bytecode VM or JIT.
pub fn set_thread_trace_collector(collector: &Arc<TraceCollector>);

/// Clear after bytecode VM or JIT returns.
pub fn clear_thread_trace_collector();

/// Access from within opcode handlers or JIT runtime.
pub fn with_thread_trace_collector<R>(
    f: impl FnOnce(&TraceCollector) -> R,
) -> Option<R>;
```

### Scoping in eval_inner_with_trace

**Source**: `src/backend/eval/mod.rs:151`

```
eval_inner_with_trace(value, env, state, collector)
│
├─ set_thread_trace_collector(collector)  ← set TLS
│
├─ [try JIT Stage 2]
│   ├─ collector.emit_converted(TierDispatch { JitStage2 })
│   ├─ execute_jit_arena_with_env(...)
│   └─ clear_thread_trace_collector()  ← clear on success return
│
├─ [try JIT Stage 1]
│   ├─ collector.emit_converted(TierDispatch { JitStage1 })
│   ├─ execute_jit_arena_with_env(...)
│   └─ clear_thread_trace_collector()  ← clear on success return
│
├─ [try Bytecode VM]
│   ├─ collector.emit_converted(TierDispatch { BytecodeVM })
│   ├─ eval_bytecode_arena_with_env(...)
│   └─ clear_thread_trace_collector()  ← clear on success return
│
└─ [fallback: Tree-Walker]
    ├─ collector.emit_converted(TierDispatch { TreeWalker })
    ├─ eval_trampoline_with_trace(...)  ← uses EvalContext pattern
    └─ clear_thread_trace_collector()  ← clear on return
```

The thread-local collector is always cleared on every return path,
including early returns from successful JIT/bytecode execution.

### Usage Inside VM Opcode Handlers

```rust
#[cfg(feature = "eval-trace")]
{
    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
    with_thread_trace_collector(|tc| {
        tc.emit_converted(
            trace_format::TraceTier::BytecodeVM,
            0,                        // depth (VM doesn't track trampoline depth)
            input_tv,
            outputs_tv,
            None,                     // expr_span
            trace_format::TraceEventKind::GroundedOp { op_name, args },
        );
    });
}
```

If no collector is set (tracing not active), `with_thread_trace_collector`
returns `None` and the closure is not called.

## Why Two Patterns?

The tree-walker's `EvalContext` is threaded through every function in the
trampoline evaluation loop. It naturally carries the trace collector.

The bytecode VM (`GenericBytecodeVM`) and JIT execution context have
their own generic type parameter systems. Adding a `TraceCollector`
parameter would require:

1. Adding a field to `GenericBytecodeVM<V, F, E>`
2. Propagating it through all `impl` blocks
3. Updating every external call site (`execute_generic`,
   `execute_generic_simple`)
4. Adding it to JIT's `JitContext` and all its constructors

This would affect 40+ files for a feature that is optional. The
thread-local pattern is:

- **Minimal**: 3 functions, 1 thread-local, ~60 lines of code
- **Non-invasive**: No type parameter changes to VM/JIT
- **Scoped**: Set/clear follows the call/return pattern naturally
- **Safe**: `Arc<TraceCollector>` is cloned (not borrowed), no lifetime issues

See [`docs/design/eval-trace/thread-local-sink.md`](../../design/eval-trace/thread-local-sink.md)
for the full design rationale.

## Diagram: Both Patterns in Context

```
┌──────────────────────────────────────────────────────────────┐
│                    eval_inner_with_trace()                    │
│                                                              │
│  1. set_thread_trace_collector(collector)                     │
│                                                              │
│  ┌──────────────────────┐  ┌───────────────────────────────┐ │
│  │    Tree-Walker        │  │    Bytecode VM / JIT          │ │
│  │                       │  │                               │ │
│  │  SessionContext {     │  │  with_thread_trace_collector(  │ │
│  │    trace_collector:   │  │    |tc| tc.emit_converted(..) │ │
│  │      Some(Arc<TC>)    │  │  )                            │ │
│  │  }                    │  │                               │ │
│  │                       │  │  ┌─────────────────────────┐  │ │
│  │  ctx.trace_collector()│  │  │ THREAD_TRACE_COLLECTOR  │  │ │
│  │    ↓                  │  │  │ RefCell<Option<Arc<TC>>>│  │ │
│  │  tc.emit(...)         │  │  └─────────────────────────┘  │ │
│  │                       │  │                               │ │
│  │  Pattern: EvalContext │  │  Pattern: Thread-Local Sink   │ │
│  └──────────────────────┘  └───────────────────────────────┘ │
│                                                              │
│  2. clear_thread_trace_collector()                            │
└──────────────────────────────────────────────────────────────┘
```
