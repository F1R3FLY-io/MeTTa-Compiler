# Developer Guide: Adding New Trace Events

This guide explains how to add new instrumentation points to the
eval-trace system.

## When to Add a New Event Kind

Add a new `TraceEventKind` variant when:

- A new special form is implemented (e.g., `pragma`, `import-from`)
- A new category of grounded operation is added (e.g., string operations)
- A new tier transition occurs (e.g., a new JIT stage)
- A new optimization is visible at the evaluation level (e.g., memoization hit)

## Step-by-Step Guide

### Step 1: Add the Variant to TraceEventKind

**File**: `trace-format/src/lib.rs`

Add a new variant to the `TraceEventKind` enum. Choose an appropriate
category section (marked by comments) and follow the existing naming
convention.

```rust
pub enum TraceEventKind {
    // ... existing variants ...

    // ---- Memoization ----
    /// A memoized result was returned without re-evaluation.
    MemoizationHit {
        expression_hash: u64,
        cached_result_count: u32,
    },
}
```

Requirements:
- Derive `Serialize, Deserialize, Clone, Debug, PartialEq` (inherited from enum)
- All fields must be serializable with postcard (no lifetimes, no raw pointers)
- Add a doc comment explaining what the event represents

### Step 2: Add Instrumentation at the Emission Site

The emission approach depends on which tier you're instrumenting.

#### Tree-Walker (uses EvalContext)

**Pattern**: `#[cfg(feature = "eval-trace")]` block with `ctx.trace_collector()`

```rust
// In src/backend/eval/trampoline/generic_trampoline.rs or
// src/backend/eval/step/generic_sexpr.rs:

#[cfg(feature = "eval-trace")]
{
    if let Some(tc) = ctx.trace_collector() {
        use crate::backend::trace::trace_value_generic;
        tc.emit_converted(
            trace_format::TraceTier::TreeWalker,
            depth,
            trace_value_generic(&expr),
            vec![/* outputs */],
            None, // or Some(trace_span)
            trace_format::TraceEventKind::MemoizationHit {
                expression_hash: hash,
                cached_result_count: results.len() as u32,
            },
        );
    }
}
```

Key points:
- Wrap the entire block in `#[cfg(feature = "eval-trace")]`
- Use `trace_value_generic(&v)` to convert generic `C::Value` to `TraceValue`
- Use `ctx.trace_collector()` to get the `Option<&TraceCollector>`
- Use `emit_converted()` when you have pre-converted `TraceValue` data
- Use `emit()` when you have `&MettaValue` references

#### Bytecode VM (uses thread-local sink)

**Pattern**: `#[cfg(feature = "eval-trace")]` block with `with_thread_trace_collector`

```rust
// In src/backend/bytecode/vm/mod.rs:

#[cfg(feature = "eval-trace")]
{
    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
    use crate::backend::trace::trace_value_generic;
    with_thread_trace_collector(|tc| {
        tc.emit_converted(
            trace_format::TraceTier::BytecodeVM,
            0, // VM doesn't track trampoline depth
            trace_value_generic(&expr),
            vec![/* outputs */],
            None,
            trace_format::TraceEventKind::MemoizationHit {
                expression_hash: hash,
                cached_result_count: results.len() as u32,
            },
        );
    });
}
```

Key points:
- Import `with_thread_trace_collector` inside the `#[cfg]` block
- The closure is only called if a trace collector is set (tracing active)
- Use `TraceTier::BytecodeVM` for the tier

#### JIT (uses thread-local sink)

**Pattern**: Same as bytecode VM but with `TraceTier::JitStage1` or `JitStage2`

```rust
// In src/backend/bytecode/jit/hybrid/arena.rs:

#[cfg(feature = "eval-trace")]
{
    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
    with_thread_trace_collector(|tc| {
        tc.emit_converted(
            trace_format::TraceTier::JitStage1,
            0,
            /* ... */
        );
    });
}
```

### Step 3: Update trace-analyzer dump.rs

**File**: `tools/trace-analyzer/src/dump.rs`

Add a match arm in `print_kind_details()` for the new variant:

```rust
fn print_kind_details(kind: &TraceEventKind) {
    match kind {
        // ... existing arms ...
        TraceEventKind::MemoizationHit { expression_hash, cached_result_count } => {
            println!("  MemoizationHit {{ hash: 0x{:016x}, cached_results: {} }}",
                     expression_hash, cached_result_count);
        }
    }
}
```

### Step 4: Update trace-analyzer stats.rs

**File**: `tools/trace-analyzer/src/stats.rs`

Add a match arm in `kind_label()`:

```rust
fn kind_label(kind: &TraceEventKind) -> &'static str {
    match kind {
        // ... existing arms ...
        TraceEventKind::MemoizationHit { .. } => "MemoizationHit",
    }
}
```

If the new event kind should be counted in a special category (like
errors or bailouts), add it to the counting logic in `run()`:

```rust
match &event.kind {
    TraceEventKind::MemoizationHit { .. } => {
        memo_hit_count += 1;
    }
    // ...
}
```

### Step 5: Update trace-analyzer search.rs (if applicable)

**File**: `tools/trace-analyzer/src/search.rs`

If the new event kind has searchable string fields, add pattern matching
in `kind_matches_pattern()`:

```rust
fn kind_matches_pattern(kind: &TraceEventKind, pattern: &str) -> bool {
    match kind {
        // ... existing arms ...
        // MemoizationHit has no string fields — skip
        _ => false,
    }
}
```

### Step 6: Test

```bash
# Verify all three crates compile
cd trace-format && cargo build
cd ../tools/trace-analyzer && cargo build
cd ../.. && cargo build --release --features eval-trace

# Run tests
cargo test --features eval-trace

# Verify non-trace build still works
cargo build --release
```

## Pattern Examples from Existing Code

### Tree-Walker: GroundedOp Success

**Source**: `src/backend/eval/trampoline/generic_trampoline.rs:253`

```rust
#[cfg(feature = "eval-trace")]
{
    if let Some(tc) = ctx.trace_collector() {
        let input = crate::backend::trace::trace_value_generic(
            &grounded_expr,
        );
        let outputs_tv: Vec<trace_format::TraceValue> = result_values
            .iter()
            .map(crate::backend::trace::trace_value_generic)
            .collect();
        let op_name = op_name.to_string();
        let args_tv: Vec<trace_format::TraceValue> = evaluated_args
            .iter()
            .map(crate::backend::trace::trace_value_generic)
            .collect();
        tc.emit_converted(
            trace_format::TraceTier::TreeWalker,
            depth,
            input,
            outputs_tv,
            None,
            trace_format::TraceEventKind::GroundedOp {
                op_name,
                args: args_tv,
            },
        );
    }
}
```

### Bytecode VM: RuleMatchSet

**Source**: `src/backend/bytecode/vm/mod.rs:3311`

```rust
#[cfg(feature = "eval-trace")]
{
    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
    use crate::backend::trace::trace_value_generic;
    with_thread_trace_collector(|tc| {
        tc.emit_converted(
            trace_format::TraceTier::BytecodeVM, 0,
            trace_value_generic(&expr),
            vec![],
            None,
            trace_format::TraceEventKind::RuleMatchSet {
                match_count: matches.len() as u32,
                matches: vec![], // Simplified for VM
            },
        );
    });
}
```

### JIT: Bailout

**Source**: `src/backend/bytecode/jit/hybrid/arena.rs:158`

```rust
#[cfg(feature = "eval-trace")]
{
    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
    with_thread_trace_collector(|tc| {
        tc.emit_converted(
            trace_format::TraceTier::JitStage1, 0,
            trace_format::TraceValue::Unit,
            vec![],
            None,
            trace_format::TraceEventKind::JitBailout {
                bailout_ip: ctx.bailout_ip,
                reason: format!("{:?}", ctx.bailout_reason),
                fallback_tier: "tree-walker".to_string(),
            },
        );
    });
}
```

## Checklist

- [ ] New variant added to `TraceEventKind` in `trace-format/src/lib.rs`
- [ ] `#[cfg(feature = "eval-trace")]` block added at instrumentation site
- [ ] Uses correct pattern (EvalContext for tree-walker, thread-local for VM/JIT)
- [ ] Uses correct `TraceTier` variant
- [ ] `dump.rs` updated with `print_kind_details()` match arm
- [ ] `stats.rs` updated with `kind_label()` match arm
- [ ] `search.rs` updated if event has searchable string fields
- [ ] `cargo test --features eval-trace` passes
- [ ] `cargo build --release` (without feature) still compiles
