# 00 — Overview and System Architecture

## What the Eval-Trace System Is

The eval-trace system is a production-quality binary tracing facility for
MeTTaTron. When enabled via `--features eval-trace`, it records a complete
log of every evaluation step — rule applications, grounded operations,
special form dispatches, type checks, errors, nondeterministic forks, tier
transitions, JIT bailouts, and GC safepoints — with full source location
provenance.

The trace is written as a compact binary `.mtrace` file that can be analyzed
offline with the standalone `trace-analyzer` tool.

## Problem It Solves

MeTTaTron's three-tier evaluation engine (tree-walker, bytecode VM, JIT)
makes it difficult to understand what happens during evaluation by
inspecting code alone. The eval-trace system provides post-hoc visibility
into:

- **Reduction chains**: What rewrites applied to reach a result
- **Tier selection**: Which expressions ran in which tier and why
- **Error provenance**: Where errors were created, propagated, and caught
- **JIT bailouts**: When and why JIT fell back to a lower tier
- **Nondeterminism**: How many branches were explored and their outcomes
- **GC pressure**: When safepoints triggered and how many roots were live
- **Type-driven optimization**: Which arguments were pre-evaluated and why

## High-Level Architecture

```
                          MeTTa Source
                              │
                              ▼
                    ┌───────────────────┐
                    │     Compiler      │
                    │  (compile.rs)     │
                    └────────┬──────────┘
                             │ MettaValue
                             ▼
                    ┌───────────────────┐
                    │   Tier Selector   │
                    │   (eval/mod.rs)   │
                    └──┬──────┬──────┬──┘
                       │      │      │
              ┌────────┘      │      └────────┐
              ▼               ▼               ▼
     ┌──────────────┐ ┌─────────────┐ ┌─────────────┐
     │ Tree-Walker  │ │ Bytecode VM │ │     JIT     │
     │  (trampoline │ │  (vm/mod.rs)│ │ (arena.rs)  │
     │   engine)    │ │             │ │             │
     └──────┬───────┘ └──────┬──────┘ └──────┬──────┘
            │                │               │
            │  trace events  │  trace events  │  trace events
            │                │               │
            └────────┐       │       ┌───────┘
                     ▼       ▼       ▼
              ┌──────────────────────────┐
              │     TraceCollector       │
              │    (collector.rs)        │
              │                          │
              │  thread-local buffers    │
              │  → batched file I/O     │
              └────────────┬─────────────┘
                           │
                           ▼
                    ┌──────────────┐
                    │ .mtrace file │
                    │ (binary)     │
                    └──────┬───────┘
                           │
                           ▼
              ┌──────────────────────────┐
              │    trace-analyzer        │
              │  (standalone CLI tool)   │
              │                          │
              │  dump │ stats │ search   │
              │  errors │ bailouts       │
              └──────────────────────────┘
```

The dotted boundary between tier selection and trace emission shows that
trace events are emitted **within** each tier's execution, not by an
external observer. Each tier has its own instrumentation points that call
into the `TraceCollector`.

## Crate Layout

The trace system spans three Rust crates:

```
MeTTa-Compiler/
├── trace-format/                     Shared types crate (no evaluator deps)
│   └── src/lib.rs                    TraceEvent, TraceValue, TraceEventKind,
│                                     TraceSpan, TraceHeader, TraceTier,
│                                     TRACE_MAGIC, TRACE_FORMAT_VERSION,
│                                     postcard serialize/deserialize helpers
│
├── src/backend/trace/                Instrumentation module (inside mettatron)
│   ├── mod.rs                        Module root, re-exports
│   ├── collector.rs                  TraceCollector (thread-safe writer)
│   ├── convert.rs                    MettaValue → TraceValue conversion
│   ├── format.rs                     Binary file I/O (header, events, footer)
│   ├── macros.rs                     trace_emit! / trace_emit_ctx! macros
│   ├── thread_local_sink.rs          Thread-local collector for VM/JIT
│   └── tests.rs                      Unit + integration tests
│
└── tools/trace-analyzer/             Standalone analysis CLI
    └── src/
        ├── main.rs                   clap CLI dispatcher
        ├── reader.rs                 Memory-mapped trace file reader
        ├── dump.rs                   Sequential event dump (text / JSON)
        ├── stats.rs                  Summary statistics + histograms
        ├── search.rs                 Pattern-based event filtering
        ├── errors.rs                 Error event listing
        └── bailouts.rs              JIT/bytecode bailout summary
```

### Why Three Crates?

- **`trace-format`** has zero dependency on the evaluator. This allows
  `trace-analyzer` to link only the shared types (postcard + serde),
  avoiding the heavy mettatron dependency graph.
- **`src/backend/trace/`** is compiled only when `--features eval-trace`
  is active. It depends on `trace-format` and on mettatron internals
  (`MettaValue`, `MettaValueInner`, `Span`, etc.).
- **`tools/trace-analyzer/`** is a separate binary crate. It depends on
  `trace-format`, `postcard`, `serde_json`, `clap`, `memmap2`, and
  `colored`.

## Feature Gate: Zero Cost When Disabled

All trace code is gated behind `#[cfg(feature = "eval-trace")]`. When the
feature is not enabled:

- The `src/backend/trace/` module is not compiled
- The `trace_emit!` macro expands to `{}` (empty block)
- The `trace_emit_ctx!` macro's body is inside `#[cfg(feature = "eval-trace")]`
- `SessionContext` does not have a `trace_collector` field
- `postcard` and `trace-format` are not linked

This means a standard `cargo build --release` produces an identical binary
to what existed before the trace system was implemented. See
[`docs/design/eval-trace/zero-cost-feature-gate.md`](../../design/eval-trace/zero-cost-feature-gate.md)
for the full design rationale.

## Source File Summary

| File | Purpose |
|------|---------|
| `trace-format/src/lib.rs` | Shared data model (TraceEvent, TraceValue, etc.) |
| `src/backend/trace/mod.rs` | Module root and re-exports |
| `src/backend/trace/collector.rs` | Thread-safe event collector with batched I/O |
| `src/backend/trace/convert.rs` | Iterative trampoline: MettaValue → TraceValue |
| `src/backend/trace/format.rs` | Binary file format read/write |
| `src/backend/trace/macros.rs` | Zero-cost conditional emission macros |
| `src/backend/trace/thread_local_sink.rs` | Thread-local collector for VM/JIT tiers |
| `src/backend/trace/tests.rs` | Unit and integration tests |
| `tools/trace-analyzer/src/main.rs` | CLI entry point (clap subcommands) |
| `tools/trace-analyzer/src/reader.rs` | Memory-mapped streaming trace reader |
| `tools/trace-analyzer/src/dump.rs` | Human-readable / JSON event dump |
| `tools/trace-analyzer/src/stats.rs` | Statistics, histograms, hot expression ranking |
| `tools/trace-analyzer/src/search.rs` | Pattern-based event filtering |
| `tools/trace-analyzer/src/errors.rs` | Error/exception event listing |
| `tools/trace-analyzer/src/bailouts.rs` | JIT/bytecode bailout summary |
