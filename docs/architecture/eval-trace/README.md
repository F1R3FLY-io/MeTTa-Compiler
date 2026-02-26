# Evaluation Trace System — Architecture

Production-quality, feature-gated binary tracing system for MeTTaTron.
Records every rewrite, type check, error, bailout, and optimization with
full source location provenance across all three evaluation tiers
(tree-walker, bytecode VM, JIT).

## Reading Order

| # | Document | Topic |
|---|----------|-------|
| 0 | [00-overview.md](00-overview.md) | System architecture overview with diagrams |
| 1 | [01-data-model.md](01-data-model.md) | TraceEvent, TraceValue, TraceEventKind data model |
| 2 | [02-binary-format.md](02-binary-format.md) | `.mtrace` binary file format specification |
| 3 | [03-collection-and-buffering.md](03-collection-and-buffering.md) | TraceCollector buffering architecture |
| 4 | [04-instrumentation-points.md](04-instrumentation-points.md) | Where trace events are emitted (per tier) |
| 5 | [05-threading-model.md](05-threading-model.md) | EvalContext vs thread-local sink patterns |

## Quick Source File Reference

| Source File | Relevant Chapter |
|-------------|-----------------|
| `trace-format/src/lib.rs` | [01-data-model](01-data-model.md), [02-binary-format](02-binary-format.md) |
| `src/backend/trace/mod.rs` | [00-overview](00-overview.md) |
| `src/backend/trace/collector.rs` | [03-collection-and-buffering](03-collection-and-buffering.md) |
| `src/backend/trace/convert.rs` | [03-collection-and-buffering](03-collection-and-buffering.md) |
| `src/backend/trace/macros.rs` | [05-threading-model](05-threading-model.md) |
| `src/backend/trace/format.rs` | [02-binary-format](02-binary-format.md) |
| `src/backend/trace/thread_local_sink.rs` | [05-threading-model](05-threading-model.md) |
| `src/backend/eval/mod.rs` | [04-instrumentation-points](04-instrumentation-points.md) |
| `src/backend/eval/trampoline/generic_trampoline.rs` | [04-instrumentation-points](04-instrumentation-points.md) |
| `src/backend/eval/step/generic_sexpr.rs` | [04-instrumentation-points](04-instrumentation-points.md) |
| `src/backend/bytecode/vm/mod.rs` | [04-instrumentation-points](04-instrumentation-points.md) |
| `src/backend/bytecode/jit/hybrid/arena.rs` | [04-instrumentation-points](04-instrumentation-points.md) |
| `src/backend/eval/trampoline/session_context.rs` | [05-threading-model](05-threading-model.md) |
| `src/backend/eval/trampoline/context.rs` | [05-threading-model](05-threading-model.md) |

## Related Documentation

- **Design decisions**: [`docs/design/eval-trace/`](../../design/eval-trace/README.md)
- **User guides**: [`docs/guides/eval-trace/`](../../guides/eval-trace/README.md)
