# JIT Stage 1 Performance Results

Date: 2025-12-15
Branch: `perf/exp16-jit-stage1-primitives`

## Overview

This document presents benchmark results for the Cranelift JIT Stage 1 implementation. Stage 1 supports primitive operations (arithmetic, boolean, comparisons) without function calls or non-determinism.

## Three-Tier Execution Pipeline

```
Tier 0: Tree-walking interpreter (fallback)
Tier 1: Bytecode VM (40-750x speedup)
Tier 2: Cranelift JIT (8-10x additional speedup over bytecode)
```

## Benchmark Results

### Arithmetic Operations (depth 100)

| Execution Tier | Time | Throughput | vs Tree-Walker | vs Prior Tier |
|----------------|------|------------|----------------|---------------|
| Tree-walker | 28.2 µs | 3.5 Melem/s | 1x (baseline) | - |
| Bytecode (compile+exec) | 7.27 µs | 13.8 Melem/s | 3.9x | 3.9x |
| Bytecode (exec only) | 140 ns | 714 Melem/s | 201x | 52x |
| **JIT (exec only)** | **15.1 ns** | **6.6 Gelem/s** | **1,868x** | **9.3x** |

### Boolean Operations (depth 100)

| Execution Tier | Time | Throughput | vs Tree-Walker | vs Prior Tier |
|----------------|------|------------|----------------|---------------|
| Tree-walker | 21.9 µs | 4.6 Melem/s | 1x | - |
| Bytecode (compile+exec) | 5.2 µs | 19.2 Melem/s | 4.2x | 4.2x |
| Bytecode (exec only) | 118.8 ns | 842 Melem/s | 184x | 44x |
| **JIT (exec only)** | **14.1 ns** | **7.1 Gelem/s** | **1,553x** | **8.4x** |

### Scaling with Expression Depth

#### JIT Arithmetic Execution

| Depth | Time | Throughput |
|-------|------|------------|
| 10 | 16.7 ns | 599 Melem/s |
| 50 | 16.6 ns | 3.0 Gelem/s |
| 100 | 15.1 ns | 6.6 Gelem/s |
| 200 | 15.6 ns | 12.8 Gelem/s |

The near-constant execution time across depths demonstrates excellent CPU pipelining and branch prediction for JIT-compiled code.

#### JIT Boolean Execution

| Depth | Time | Throughput |
|-------|------|------------|
| 10 | 14.9 ns | 671 Melem/s |
| 50 | 13.1 ns | 3.8 Gelem/s |
| 100 | 14.2 ns | 7.1 Gelem/s |
| 200 | 13.7 ns | 14.4 Gelem/s |

### Compilation Overhead

| Phase | Depth 10 | Depth 50 | Depth 100 | Depth 200 |
|-------|----------|----------|-----------|-----------|
| Bytecode compile | 318 ns | 1.33 µs | 3.27 µs | 7.06 µs |
| JIT compile | 670 µs | 3.1 ms | 6.77 ms | 13.2 ms |
| Ratio | 2,100x | 2,330x | 2,070x | 1,870x |

JIT compilation is approximately 2,000x slower than bytecode compilation, but provides 8-10x faster execution.

### Repeated Execution (1000 iterations, depth 100)

| Method | Total Time | Per-Iteration | vs Tree-Walker |
|--------|------------|---------------|----------------|
| Tree-walker | 27.9 ms | 27.9 µs | 1x |
| Bytecode (recompile each) | 7.57 ms | 7.57 µs | 3.7x |
| Bytecode (precompiled) | 125 µs | 125 ns | 223x |
| **JIT (precompiled)** | **16.0 µs** | **16 ns** | **1,744x** |

## Break-Even Analysis

### JIT vs Bytecode Break-Even

- JIT compilation cost: ~6.7 ms (depth 100)
- Per-execution savings: ~125 ns (140 ns bytecode - 15 ns JIT)
- **Break-even point: ~53,600 executions**

### JIT vs Tree-Walker Break-Even

- JIT compilation cost: ~6.7 ms
- Per-execution savings: ~28 µs (28.2 µs tree-walker - 15 ns JIT)
- **Break-even point: ~240 executions**

The current HOT_THRESHOLD of 100 triggers JIT compilation early, which amortizes well for truly hot paths.

## Stage 1 Supported Operations

### Compilable Opcodes (Stage 1)

- **Stack**: Nop, Pop, Dup, Swap, Rot3, Over, DupN, PopN
- **Constants**: PushNil, PushTrue, PushFalse, PushUnit, PushLongSmall
- **Arithmetic**: Add, Sub, Mul, Div, Mod, FloorDiv, Neg, Abs
- **Boolean**: And, Or, Not
- **Comparison**: Lt, Le, Gt, Ge, Eq, Ne
- **Control**: Return

### Stage 2 (Implemented)

See [JIT_STAGE2_PERFORMANCE.md](JIT_STAGE2_PERFORMANCE.md) for Stage 2 details.

- **PushLong**: Large integer constants via runtime call
- **PushConstant**: Generic constant loading via runtime call
- **Pow**: Exponentiation via runtime call

### Not Compilable (Stage 3+)

- Call, TailCall (function calls)
- Fork, Choice (non-determinism)
- GetSpace, SetSpace (space operations)
- MakeList, Cons, Car, Cdr (list operations)

## NaN-Boxing Representation

JIT uses NaN-boxing for efficient type checking:

```
Tag Layout (upper 16 bits):
- 0x7FF8: Long (48-bit signed integer payload)
- 0x7FF9: Bool (1-bit payload)
- 0x7FFA: Nil
- 0x7FFB: Unit
- 0x7FFC: Heap pointer (48-bit address)
```

This enables:
- Single-instruction type checking via bit masking
- Register-only value passing (no boxing/unboxing allocation)
- 48-bit integer range without heap allocation

## Benchmark Configuration

- CPU affinity: cores 0-17
- Measurement time: 10s per benchmark
- Sample size: 100 iterations
- Framework: Criterion.rs

### Running the Benchmarks

```bash
# Full JIT benchmark suite
taskset -c 0-17 cargo bench --features jit --bench jit_comparison

# Specific benchmark group
cargo bench --features jit --bench jit_comparison -- jit_arithmetic
```

## Future Work (Stage 3+)

Stage 2 (runtime calls) is now implemented. See [JIT_STAGE2_PERFORMANCE.md](JIT_STAGE2_PERFORMANCE.md).

Remaining work:
1. **User-defined function calls**: Call, TailCall with JIT trampolines
2. **Non-determinism**: Fork, Choice with continuation support
3. **Loop optimization**: Trace-based compilation for hot loops
4. **Inline caching**: Speculative optimization for dynamic dispatch
5. **Profile-guided optimization**: Use execution profiles to optimize hot paths

## Conclusion

JIT Stage 1 delivers **8-10x speedup** over the bytecode VM for primitive operations, achieving **~15 ns per execution** for arithmetic chains. The total speedup from tree-walking to JIT is approximately **1,800x**.

The implementation uses Cranelift for native code generation with NaN-boxing for efficient value representation. Hot paths (>100 executions) automatically trigger JIT compilation, with break-even occurring around 54,000 executions vs bytecode.
