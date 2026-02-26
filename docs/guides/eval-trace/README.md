# Evaluation Trace System — User Guides

Practical how-to documentation for using the eval-trace system.

## Quick Start

```bash
# Build with tracing enabled
cargo build --release --features eval-trace

# Run with trace output
./target/release/mettatron --trace /tmp/trace.mtrace input.metta

# Analyze the trace
cd tools/trace-analyzer && cargo run -- stats /tmp/trace.mtrace
```

## Reading Order

| # | Document | Topic |
|---|----------|-------|
| 1 | [getting-started.md](getting-started.md) | Building, running, and producing your first trace |
| 2 | [trace-analyzer.md](trace-analyzer.md) | trace-analyzer tool reference (all subcommands) |
| 3 | [reading-traces.md](reading-traces.md) | Understanding trace event structure and semantics |
| 4 | [debugging-with-traces.md](debugging-with-traces.md) | Common debugging workflows |
| 5 | [adding-instrumentation.md](adding-instrumentation.md) | Developer guide: adding new trace events |

## Related Documentation

- **Architecture**: [`docs/architecture/eval-trace/`](../../architecture/eval-trace/README.md)
- **Design decisions**: [`docs/design/eval-trace/`](../../design/eval-trace/README.md)
