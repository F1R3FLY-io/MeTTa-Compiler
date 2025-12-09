# Bytecode VM Benchmark Results

**Date:** 2025-12-09 (Updated with nondeterminism support)
**Branch:** perf/exp15-bytecode-vm

## Summary

The bytecode VM demonstrates **dramatic performance improvements** over the tree-walking interpreter, with speedups ranging from **4× to 750×** depending on workload characteristics.

**Update:** Nondeterminism (Fork/Fail/Yield) is now fully implemented and working.

## Bytecode VM vs Tree-Walker Comparison

### Arithmetic Expression Chains
Nested arithmetic: `((((1 + 0) + 1) + 2) + ... + n)`

| Depth | Tree-Walker | Bytecode | Speedup |
|-------|-------------|----------|---------|
| 5     | 17.9 µs     | 458 ns   | **39×** |
| 10    | 40.6 µs     | 728 ns   | **56×** |
| 20    | 169.0 µs    | 1.31 µs  | **129×** |
| 50    | 2.24 ms     | 2.98 µs  | **751×** |

**Analysis:** The bytecode VM shows near-linear scaling while the tree-walker exhibits superlinear (quadratic) scaling. At depth 50, the bytecode VM is **751× faster**.

### Boolean Logic Chains
Alternating `not`, `or`, `and` operations.

| Depth | Tree-Walker | Bytecode | Speedup |
|-------|-------------|----------|---------|
| 5     | 15.8 µs     | 398 ns   | **40×** |
| 10    | 34.4 µs     | 665 ns   | **52×** |
| 20    | 143.5 µs    | 1.18 µs  | **122×** |
| 50    | 1.95 ms     | 2.58 µs  | **756×** |

**Analysis:** Similar scaling characteristics to arithmetic. Boolean operations have identical speedup patterns.

### Conditional Chains
Nested if expressions: `(if (< i 5) i (if ...))`

| Depth | Tree-Walker | Bytecode | Speedup |
|-------|-------------|----------|---------|
| 5     | 12.3 µs     | 549 ns   | **22×** |
| 10    | 20.0 µs     | ~900 ns  | **22×** |
| 20    | ~54 µs      | ~1.6 µs  | **34×** |

**Analysis:** Conditionals show consistent 22-34× improvement. Lower than arithmetic/boolean due to branch evaluation overhead.

### Nondeterminism (Superpose)
`(superpose (0 1 2 ... n))` - **Now fully working with Fork/Fail/Yield opcodes**

| Alternatives | Tree-Walker | Bytecode | Speedup |
|--------------|-------------|----------|---------|
| 5            | 108.0 µs    | 874 ns   | **124×** |
| 10           | ~117 µs     | 1.56 µs  | **75×** |
| 50           | ~171 µs     | 10.95 µs | **16×** |
| 100          | ~141 µs     | 33.77 µs | **4×** |

**Analysis:** Superpose shows decreasing speedup with more alternatives. The bytecode VM's Fork/Yield pattern has higher per-alternative overhead than the tree-walker's simpler result collection, but starts from a much lower base. Even at 100 alternatives, bytecode is still 4× faster.

### Quote Operations

| Type   | Tree-Walker | Bytecode | Speedup |
|--------|-------------|----------|---------|
| Simple | ~7 µs       | 376 ns   | **19×** |
| Deep   | 928.3 µs    | 179.6 µs | **5×** |

**Analysis:** Quote operations show good improvement. Deep quotes are limited by value construction overhead common to both implementations.

## Bytecode VM Absolute Performance

### Per-Operation Timing (Bytecode VM only)

| Operation Type | Time/Operation |
|----------------|----------------|
| Arithmetic     | ~58-100 ns/op  |
| Boolean        | ~51-84 ns/op   |
| Conditional    | ~80-112 ns/op  |
| Superpose      | ~180-360 ns/alt |

### Compilation Overhead

| Expression | Compile  | Execute  | Total    | Compile % |
|------------|----------|----------|----------|-----------|
| Small      | 93 ns    | 165 ns   | 275 ns   | 34%       |
| Medium (20)| 526 ns   | 712 ns   | 1.28 µs  | 41%       |
| Large (100)| 2.84 µs  | 2.98 µs  | 5.99 µs  | 47%       |

**Analysis:** Compilation overhead is roughly proportional to expression size. For single-use expressions, compilation adds 34-47% overhead. For expressions evaluated multiple times (like rules), pre-compilation pays off after 2 executions.

## Key Findings

1. **Massive Speedups:** 35-750× faster than tree-walker for nested expressions
2. **Linear vs Quadratic:** Bytecode VM scales linearly; tree-walker scales quadratically with depth
3. **Depth Amplifies Gains:** Higher expression depths show larger speedups
4. **Nondeterminism Trade-off:** Superpose speedup decreases with alternative count (115× → 4×)
5. **Compilation Amortizes:** For frequently-evaluated expressions, pre-compilation pays off after 2 executions

## Why Such Large Speedups?

The tree-walking interpreter's superlinear scaling comes from:
1. **Repeated environment lookups** for each nested evaluation
2. **Stack management overhead** per recursive call
3. **Pattern matching** repeated at each level
4. **Clone operations** for intermediate values

The bytecode VM avoids these by:
1. **Direct dispatch** via computed goto (no enum matching)
2. **Pre-compiled patterns** (no repeated analysis)
3. **Efficient stack operations** (push/pop only)
4. **Reference-counted values** (minimal cloning)

## Test Coverage

- **Unit tests:** 167 tests covering all opcodes, compilation, and execution
- **Equivalence tests:** 39 tests ensuring bytecode results match tree-walker
- **All tests passing:** 206 total tests

## Architecture

- **4-stack VM:** value stack, call stack, bindings stack, choice points
- **~100 opcodes:** covering all MeTTa operations
- **Nondeterminism:** Fork/Fail/Yield pattern for superpose/collapse
- **Compilation:** MettaValue → BytecodeChunk with constant pool

## Files

- `src/backend/bytecode/mod.rs` - Module exports, VM struct
- `src/backend/bytecode/opcodes.rs` - Opcode definitions
- `src/backend/bytecode/chunk.rs` - BytecodeChunk implementation
- `src/backend/bytecode/compiler.rs` - Expression compiler
- `src/backend/bytecode/vm.rs` - VM execution loop
- `src/backend/bytecode/nondeterminism.rs` - Fork/Fail/Yield
- `benches/bytecode_comparison.rs` - Benchmark suite
