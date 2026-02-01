# JIT Stage 2 Performance Results

Date: 2025-12-15
Branch: `perf/exp16-jit-stage1-primitives`

## Overview

This document presents benchmark results for the Cranelift JIT Stage 2 implementation. Stage 2 extends Stage 1 with **runtime function calls** for operations that cannot be fully inlined (Pow, large constants).

## Stage 2 Features

Stage 2 adds support for:
- **Pow**: Exponentiation via `jit_runtime_pow()` runtime call
- **PushLong**: Large integers (>127 or <-128) via `jit_runtime_load_constant()`
- **PushConstant**: Generic constant loading via `jit_runtime_load_constant()`

These operations use Cranelift's function import mechanism to call into Rust runtime functions while maintaining JIT-compiled execution for the rest of the code.

## Three-Tier Execution Pipeline

```
Tier 0: Tree-walking interpreter (fallback)
Tier 1: Bytecode VM (40-750x speedup)
Tier 2: Cranelift JIT
  - Stage 1: Pure primitives (8-10x speedup over bytecode)
  - Stage 2: +Runtime calls (4-6x speedup for Pow operations)
```

## Benchmark Results

### Stage 2: Pow Operations (Exponentiation)

| Depth | Execution Tier | Time | Throughput | vs Tree-Walker | vs Bytecode |
|-------|----------------|------|------------|----------------|-------------|
| 3 | Tree-walker | 4.8 µs | 625 Kelem/s | 1x | - |
| 3 | Bytecode (exec only) | 159 ns | 18.9 Melem/s | 30x | 1x |
| 3 | **JIT (exec only)** | **28.7 ns** | **104 Melem/s** | **167x** | **5.5x** |
| 5 | Tree-walker | 8.2 µs | 610 Kelem/s | 1x | - |
| 5 | Bytecode (exec only) | 188 ns | 26.6 Melem/s | 43x | 1x |
| 5 | **JIT (exec only)** | **48.5 ns** | **103 Melem/s** | **169x** | **3.9x** |

### Stage 2: Mixed Operations (Large Constants)

Mixed arithmetic with large integers now JIT-compiles via `PushLong`:

| Depth | Execution Tier | Time | Throughput | vs Tree-Walker | vs Bytecode |
|-------|----------------|------|------------|----------------|-------------|
| 50 | Tree-walker | 14.9 µs | 3.4 Melem/s | 1x | - |
| 50 | Bytecode (exec only) | 189 ns | 264 Melem/s | 79x | 1x |
| 50 | **JIT (exec only)** | **19.3 ns** | **2.6 Gelem/s** | **772x** | **9.8x** |
| 100 | Tree-walker | 28.0 µs | 3.6 Melem/s | 1x | - |
| 100 | Bytecode (exec only) | 177 ns | 565 Melem/s | 158x | 1x |
| 100 | **JIT (exec only)** | **17.8 ns** | **5.6 Gelem/s** | **1,573x** | **9.9x** |

### Stage 1 Performance (Unchanged)

Primitive operations maintain Stage 1 performance:

| Operation | Bytecode (exec) | JIT (exec) | Speedup |
|-----------|-----------------|------------|---------|
| Arithmetic (depth 100) | 140 ns | 14.7 ns | 9.5x |
| Boolean (depth 100) | 119 ns | 14.1 ns | 8.4x |

## Runtime Call Overhead Analysis

Runtime calls add ~10-15 ns per call compared to inlined operations:

| Operation Type | Per-Operation | Notes |
|----------------|---------------|-------|
| Inlined (Add, Sub, etc.) | ~0.15 ns | Direct native instructions |
| Runtime call (Pow) | ~10 ns | Function call + computation |
| Runtime call (LoadConst) | ~8 ns | Memory access + return |

For Pow-heavy workloads, the **~10 ns overhead per call** is significantly better than:
- Bytecode interpreter dispatch (~40-50 ns per Pow)
- Tree-walker evaluation (~1-2 µs per Pow)

## Stage 2 Supported Operations

### Compilable Opcodes (Stage 1 + Stage 2)

**Stage 1 (Inlined)**:
- **Stack**: Nop, Pop, Dup, Swap, Rot3, Over, DupN, PopN
- **Constants**: PushNil, PushTrue, PushFalse, PushUnit, PushLongSmall
- **Arithmetic**: Add, Sub, Mul, Div, Mod, FloorDiv, Neg, Abs
- **Boolean**: And, Or, Not
- **Comparison**: Lt, Le, Gt, Ge, Eq, Ne
- **Control**: Return

**Stage 2 (Runtime Calls)**:
- **Constants**: PushLong, PushConstant
- **Arithmetic**: Pow

### Not Compilable (Future Stages)

- Call, TailCall (function calls)
- Fork, Choice (non-determinism)
- GetSpace, SetSpace (space operations)
- MakeList, Cons, Car, Cdr (list operations)

## Implementation Details

### Runtime Function Registration

Runtime functions are registered with Cranelift's symbol builder:

```rust
// compiler.rs - register_runtime_symbols()
builder.symbol("jit_runtime_pow", jit_runtime_pow as *const u8);
builder.symbol("jit_runtime_load_constant", jit_runtime_load_constant as *const u8);
```

### Function Import

Functions are declared as imports and called via Cranelift's `call` instruction:

```rust
// Declare import
let pow_func_id = module.declare_function("jit_runtime_pow", Linkage::Import, &sig)?;

// Call from JIT code
let func_ref = self.module.declare_func_in_func(self.pow_func_id, codegen.builder.func);
let call_inst = codegen.builder.ins().call(func_ref, &[base, exp]);
```

### NaN-Boxing in Runtime Calls

Runtime functions receive and return NaN-boxed values directly:

```rust
pub extern "C" fn jit_runtime_pow(base_boxed: u64, exp_boxed: u64) -> u64 {
    let base = unbox_long(base_boxed);
    let exp = unbox_long(exp_boxed);
    let result = base.pow(exp as u32);
    box_long(result)
}
```

## Benchmark Configuration

- CPU affinity: cores 0-17
- Measurement time: 10s per benchmark
- Sample size: 100 iterations
- Framework: Criterion.rs

### Running the Benchmarks

```bash
# Full JIT benchmark suite (Stage 1 + Stage 2)
taskset -c 0-17 cargo bench --features jit --bench jit_comparison

# Stage 2 Pow benchmark
cargo bench --features jit --bench jit_comparison -- jit_pow

# Mixed operations (uses PushLong)
cargo bench --features jit --bench jit_comparison -- jit_mixed
```

## Future Work (Stage 3+)

1. **User-defined function calls**: Call, TailCall with JIT trampolines
2. **Non-determinism**: Fork, Choice with continuation support
3. **List operations**: MakeList, Cons, Car, Cdr via runtime or inline
4. **Loop optimization**: Trace-based compilation for hot loops
5. **Inline caching**: Speculative optimization for dynamic dispatch

## Conclusion

JIT Stage 2 successfully extends the JIT compiler with **runtime call support**, enabling compilation of expressions containing:
- **Pow operations**: ~4-6x speedup over bytecode
- **Large integers**: ~10x speedup over bytecode (via PushLong)

The runtime call infrastructure provides a foundation for future stages that will add user-defined function support and more complex operations while maintaining the performance benefits of JIT compilation.

Key achievements:
- **Pow JIT execution**: 28-48 ns (vs 159-188 ns bytecode)
- **Mixed operations**: 17-19 ns (vs 177-189 ns bytecode)
- **Total speedup**: ~170x over tree-walker for Pow, ~1,500x for mixed operations
