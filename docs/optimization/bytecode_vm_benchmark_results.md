# Bytecode VM Benchmark Results

**Date:** 2025-12-08
**Branch:** perf/exp11-pattern-match-cache (Bytecode VM Phase)

## Summary

The bytecode VM demonstrates **dramatic performance improvements** over the tree-walking interpreter, with speedups ranging from **4× to 750×** depending on workload characteristics.

## Bytecode VM vs Tree-Walker Comparison

### Arithmetic Expression Chains
Nested arithmetic: `((((1 + 0) + 1) + 2) + ... + n)`

| Depth | Tree-Walker | Bytecode | Speedup |
|-------|-------------|----------|---------|
| 5     | 17.6 µs     | 498 ns   | **35×** |
| 10    | 39.2 µs     | 797 ns   | **49×** |
| 20    | 161.6 µs    | 1.27 µs  | **127×** |
| 50    | 2.08 ms     | 2.92 µs  | **714×** |

**Analysis:** The bytecode VM shows near-linear scaling while the tree-walker exhibits superlinear (quadratic) scaling. At depth 50, the bytecode VM is **714× faster**.

### Boolean Logic Chains
Alternating `not`, `or`, `and` operations.

| Depth | Tree-Walker | Bytecode | Speedup |
|-------|-------------|----------|---------|
| 5     | 15.6 µs     | 436 ns   | **36×** |
| 10    | 34.3 µs     | 704 ns   | **49×** |
| 20    | 138.3 µs    | 1.16 µs  | **119×** |
| 50    | 1.90 ms     | 2.52 µs  | **754×** |

**Analysis:** Similar scaling characteristics to arithmetic. Boolean operations have identical speedup patterns.

### Conditional Chains
Nested if expressions: `(if (< i 5) i (if ...))`

| Depth | Tree-Walker | Bytecode | Speedup |
|-------|-------------|----------|---------|
| 5     | 12.5 µs     | 574 ns   | **22×** |
| 10    | 19.6 µs     | 907 ns   | **22×** |
| 20    | 53.8 µs     | 1.61 µs  | **33×** |

**Analysis:** Conditionals show consistent 22-33× improvement. Lower than arithmetic/boolean due to branch evaluation overhead.

### Nondeterminism (Superpose)
`(superpose (0 1 2 ... n))`

| Alternatives | Tree-Walker | Bytecode | Speedup |
|--------------|-------------|----------|---------|
| 5            | 103.1 µs    | 897 ns   | **115×** |
| 10           | 113.2 µs    | 1.65 µs  | **68×** |
| 50           | 171.0 µs    | 11.67 µs | **15×** |
| 100          | 138.3 µs    | 35.72 µs | **4×** |

**Analysis:** Superpose shows decreasing speedup with more alternatives. The bytecode VM's Fork/Yield pattern has higher per-alternative overhead than the tree-walker's simpler result collection, but starts from a much lower base.

### Quote Operations

| Type   | Tree-Walker | Bytecode | Speedup |
|--------|-------------|----------|---------|
| Simple | 7.16 µs     | 402 ns   | **18×** |
| Deep   | 936.2 µs    | 190.3 µs | **5×** |

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
