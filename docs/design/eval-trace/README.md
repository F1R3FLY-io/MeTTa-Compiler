# Evaluation Trace System — Design Decisions

Design rationale documents for the eval-trace system. Each document
explains why a specific approach was chosen, what alternatives were
considered, and what trade-offs were made.

| Document | Topic |
|----------|-------|
| [serialization-format.md](serialization-format.md) | Why postcard over rkyv |
| [value-conversion.md](value-conversion.md) | Iterative trampoline for MettaValue → TraceValue |
| [thread-local-sink.md](thread-local-sink.md) | Thread-local collector for bytecode VM and JIT |
| [zero-cost-feature-gate.md](zero-cost-feature-gate.md) | How eval-trace achieves zero overhead when disabled |
| [gc-safety.md](gc-safety.md) | Owned value snapshots for garbage collection safety |

## Related Documentation

- **Architecture**: [`docs/architecture/eval-trace/`](../../architecture/eval-trace/README.md)
- **User guides**: [`docs/guides/eval-trace/`](../../guides/eval-trace/README.md)
