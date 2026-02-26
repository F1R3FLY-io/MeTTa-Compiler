# Getting Started with Evaluation Tracing

## Prerequisites

- Rust toolchain (1.70+)
- MeTTaTron source code

## Building with Tracing Enabled

The eval-trace system is behind a Cargo feature gate. To enable it:

```bash
cargo build --release --features eval-trace
```

This adds two additional dependencies (`postcard` and `trace-format`)
and compiles the instrumentation code into the binary. The resulting
binary is slightly larger but functionally identical to a non-trace
build for normal evaluation.

## Running with Trace Output

Use the `--trace` flag to specify the output trace file:

```bash
./target/release/mettatron --trace /tmp/trace.mtrace input.metta
```

What happens:
1. The trace file is created at the specified path
2. Evaluation proceeds normally — the trace system records events in
   per-thread buffers and flushes them to the file in batches of 1024
3. When evaluation completes, the trace is finalized (footer written,
   event count printed to stderr)

### Verifying the Trace Was Written

```bash
ls -la /tmp/trace.mtrace
```

A trace file for a simple program will be a few kilobytes. Complex
programs (mmverify, PLN) can produce trace files in the hundreds of
megabytes or gigabytes.

## Quick Analysis

Build and run the trace-analyzer:

```bash
cd tools/trace-analyzer
cargo build --release
./target/release/trace-analyzer stats /tmp/trace.mtrace
```

## Complete Example

### 1. Create a Simple MeTTa Program

Create `example.metta`:

```metta
(= (double $x) (* 2 $x))
!(double 21)
```

### 2. Build with Tracing

```bash
cargo build --release --features eval-trace
```

### 3. Run with Tracing

```bash
./target/release/mettatron --trace /tmp/example.mtrace example.metta
```

Expected stdout output:

```
[42]
```

Expected stderr output:

```
[trace] Finalized: N events written to /tmp/example.mtrace
```

### 4. View Statistics

```bash
cd tools/trace-analyzer
cargo run -- stats /tmp/example.mtrace
```

Expected output (approximate):

```
=== MeTTaTron Trace Statistics ===

Source: example.metta
Version: 0.2.0
Total events: 8
Max eval depth: 3
Errors: 0
Bailouts: 0
GC safepoints: 0

--- Events by Tier ---
  TreeWalker          8  (100.0%)

--- Events by Kind ---
  EvalStart                            1  (12.5%)
  EvalEnd                              1  (12.5%)
  SpecialForm                          2  (25.0%)
  RuleMatchSet                         1  (12.5%)
  GroundedOp                           2  (25.0%)
  ApplicativePreEval                   1  (12.5%)

--- Depth Histogram ---
  D0         2  ████████████████████████████████████████
  D1         3  ████████████████████████████████████████████████
  D2         2  ████████████████████████████████████████
  D3         1  ████████████████████
```

### 5. Dump Individual Events

```bash
cd tools/trace-analyzer
cargo run -- dump /tmp/example.mtrace
```

This shows every event in human-readable format, including input/output
expressions, event kinds, and source locations.

### 6. Search for Specific Events

```bash
cd tools/trace-analyzer
cargo run -- search /tmp/example.mtrace "double"
```

This filters events where the atom `double` appears in the input,
output, or event kind fields.

## Non-Trace Builds

A standard build without `--features eval-trace` produces a binary with
zero tracing overhead — no runtime branches, no additional memory, no
dead code. The `--trace` flag is not recognized:

```bash
cargo build --release
./target/release/mettatron --trace /tmp/trace.mtrace input.metta
# Error: unrecognized option '--trace'
```

See [`docs/design/eval-trace/zero-cost-feature-gate.md`](../../design/eval-trace/zero-cost-feature-gate.md)
for details on how this is achieved.

## Profile-Guided Optimization with Tracing

PGO builds work with the eval-trace feature:

```bash
# Step 1: Build instrumented
cargo clean
RUSTFLAGS="-Cprofile-generate=/tmp/pgo-data -Ctarget-cpu=native" \
    cargo build --release --features eval-trace

# Step 2: Collect profile data
./target/release/mettatron examples/mmverify/demo0/verify_demo0.metta

# Step 3: Merge
llvm-profdata merge -o /tmp/pgo-data/merged.profdata /tmp/pgo-data/*.profraw

# Step 4: Build optimized
cargo clean
RUSTFLAGS="-Cprofile-use=/tmp/pgo-data/merged.profdata -Ctarget-cpu=native" \
    cargo build --release --features eval-trace
```

Note: Tracing adds overhead to the profiled workload. For maximum
performance of the traced binary itself, include traced runs in the
PGO training set. For maximum performance of the non-traced binary,
profile without the eval-trace feature.
