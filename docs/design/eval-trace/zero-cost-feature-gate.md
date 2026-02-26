# How eval-trace Achieves Zero Overhead When Disabled

## Goal

When the `eval-trace` feature is not enabled, the trace system must have
**zero overhead** — no runtime branches, no dead code, no additional
dependencies, no increased binary size, no per-context memory overhead.

## Feature Gate Mechanism

### Cargo Feature Definition

**Source**: `Cargo.toml:268`

```toml
[features]
eval-trace = ["dep:postcard", "dep:trace-format"]
```

The `eval-trace` feature enables two optional dependencies:

- `postcard` — Binary serialization format
- `trace-format` — Shared trace types crate (local path dependency)

When the feature is disabled, neither crate is compiled or linked.

### Module-Level Gate

**Source**: `src/backend/mod.rs:25`

```rust
#[cfg(feature = "eval-trace")]
pub mod trace;
```

The entire `src/backend/trace/` module tree (collector, convert, format,
macros, thread_local_sink, tests) is not compiled when the feature is
disabled. The module simply does not exist.

### Re-Export Gate

**Source**: `src/lib.rs:121`

```rust
#[cfg(feature = "eval-trace")]
pub use backend::trace;
#[cfg(feature = "eval-trace")]
pub use backend::trace::TraceCollector;
```

The public API does not expose any trace types when the feature is
disabled.

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

The `#[cfg(feature = "eval-trace")]` is **inside** the macro body.
When the feature is disabled, the macro expands to an empty block `{}`.
The compiler eliminates this entirely — no function calls, no branches.

### Inline #[cfg] Blocks

At each instrumentation point, the trace emission code is wrapped in:

```rust
#[cfg(feature = "eval-trace")]
{
    // ... trace emission code ...
}
```

When disabled, the entire block is removed at compile time.

### SessionContext Field

**Source**: `src/backend/eval/trampoline/session_context.rs:74`

```rust
pub struct SessionContext {
    // ...
    #[cfg(feature = "eval-trace")]
    trace_collector: Option<Arc<TraceCollector>>,
}
```

When the feature is disabled, this 8-byte `Option<Arc<TraceCollector>>`
field does not exist. The `SessionContext` struct is smaller.

### EvalContext Trait Method

**Source**: `src/backend/eval/trampoline/context.rs:97`

```rust
pub trait EvalContext {
    // ...
    #[cfg(feature = "eval-trace")]
    fn trace_collector(&self) -> Option<&TraceCollector>;
}
```

When disabled, the `trace_collector()` method does not exist on the
trait. Any code that calls it is also behind `#[cfg(feature = "eval-trace")]`.

### CLI Flag

**Source**: `src/main.rs:38`

```rust
#[cfg(feature = "eval-trace")]
eprintln!("    --trace <FILE>          Write binary evaluation trace to FILE");
```

The `--trace` CLI flag is only recognized when the feature is enabled.
The `Options` struct's `trace_output` field is also gated:

```rust
struct Options {
    // ...
    #[cfg(feature = "eval-trace")]
    trace_output: Option<String>,
}
```

## Verification

### Binary Comparison

A `cargo build --release` (no feature) produces a binary identical to
what existed before the trace system was implemented. This can be
verified by:

```bash
# Build without trace
cargo build --release
cp target/release/mettatron /tmp/mettatron_no_trace

# Build with trace
cargo build --release --features eval-trace
cp target/release/mettatron /tmp/mettatron_with_trace

# Compare sizes
ls -la /tmp/mettatron_no_trace /tmp/mettatron_with_trace
```

The `_with_trace` binary will be larger due to the postcard, trace-format,
and instrumentation code.

### Compile-Time Verification

```bash
# Verify no trace symbols in non-trace build
nm target/release/mettatron | grep -i trace
# Should produce no results

# Verify trace symbols exist in trace build
nm target/release/mettatron | grep -i trace_collector
# Should produce results
```

## Summary of Gate Points

| Location | What Is Gated |
|----------|--------------|
| `Cargo.toml` | `postcard` and `trace-format` dependencies |
| `src/backend/mod.rs:25` | `pub mod trace` module declaration |
| `src/lib.rs:121-123` | Public re-exports of trace types |
| `src/main.rs:38,66,83,131,163,335,364,398` | CLI flag, `Options` field, trace setup/teardown |
| `src/backend/eval/trampoline/session_context.rs:74,99,109,228` | `trace_collector` field and methods |
| `src/backend/eval/trampoline/context.rs:97` | `trace_collector()` trait method |
| `src/backend/eval/trampoline/generic_trampoline.rs:91,152-156,253,301,1312` | Tree-walker instrumentation points |
| `src/backend/eval/step/generic_sexpr.rs:84,1562,1594,1638` | S-expr step instrumentation points |
| `src/backend/eval/mod.rs:125,150` | `eval_with_trace` and `eval_inner_with_trace` |
| `src/backend/eval/trampoline/mod.rs:47` | `eval_trampoline_with_trace` export |
| `src/backend/eval/trampoline/arena_engine.rs:20,66` | Arena engine trace wrapper |
| `src/backend/bytecode/vm/mod.rs:1236,2775,3035,3075,3311,3341,3392` | VM opcode instrumentation |
| `src/backend/bytecode/jit/hybrid/arena.rs:158,324` | JIT bailout instrumentation |
