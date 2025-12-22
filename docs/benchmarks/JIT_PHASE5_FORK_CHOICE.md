# JIT Phase 5: Fork/Choice Implementation Results

Date: 2025-12-16
Branch: `perf/exp16-jit-stage1-primitives`

## Overview

This document presents the Phase 5 implementation results for JIT Fork/Choice support. Phase 5 establishes the JIT infrastructure for non-deterministic evaluation through a **bailout-to-VM approach**, where JIT-compiled code transitions to the bytecode VM when encountering Fork/Choice opcodes.

## Design Decision: Bailout vs Native JIT Fork

After implementing the runtime infrastructure (JitContext choice point fields, JitChoicePoint/JitAlternative types, and runtime functions), we evaluated two approaches for Fork/Choice handling:

### Option 1: Native JIT Fork (Complex)
- Generate native assembly for choice point creation/backtracking
- Requires complex state management in generated code
- Risk of stack corruption during backtracking
- Estimated 2-3 weeks additional development

### Option 2: Bailout to VM (Chosen)
- JIT rejects chunks containing Fork/Choice opcodes
- VM handles non-determinism with existing well-tested implementation
- Hybrid execution: JIT for deterministic parts, VM for non-determinism
- Zero correctness risk for non-deterministic semantics

**Decision**: Bailout approach selected. The correctness guarantees of the VM's Fork/Choice implementation outweigh the potential performance benefits of native JIT Fork. Hot deterministic code paths still benefit from full JIT speedup.

## Phase 5 Implementation Summary

### Phase 5.1-5.5: JIT Infrastructure (Complete)

1. **JitBailoutReason::NonDeterminism** - New bailout reason for Fork opcodes
2. **JitChoicePoint struct** - C-compatible choice point representation
3. **JitAlternative enum** - Alternative types (Value, Chunk, Index, RuleMatch)
4. **JitContext extension** - Choice point stack and results buffer fields
5. **Runtime functions** - `jit_runtime_push_choice_point`, `jit_runtime_fail`, `jit_runtime_yield`, `jit_runtime_collect`

### Phase 5.6-5.7: Native JIT Fork (Deferred)

Native assembly generation for Fork/Choice deferred in favor of bailout approach. The infrastructure from Phase 5.1-5.5 is ready if native implementation becomes necessary.

## Baseline Benchmark Results

These benchmarks establish the performance baseline before Phase 6 experimental optimizations.

### Test Configuration

- CPU: AMD Ryzen 9 7950X (32 threads, 16 cores)
- CPU affinity: cores 0-17
- Measurement time: 10s per benchmark
- Sample size: 100 iterations
- Framework: Criterion.rs

### Arithmetic Operations

| Depth | Execution Tier | Time | Throughput | vs Tree-Walker | vs Bytecode |
|-------|----------------|------|------------|----------------|-------------|
| 10 | Tree-walker | 3.00 µs | 3.33 Melem/s | 1x | - |
| 10 | Bytecode (exec) | 90.0 ns | 111 Melem/s | 33x | 1x |
| 10 | **JIT (exec)** | **16.9 ns** | **592 Melem/s** | **178x** | **5.3x** |
| 50 | Tree-walker | 13.7 µs | 3.65 Melem/s | 1x | - |
| 50 | Bytecode (exec) | 140 ns | 357 Melem/s | 98x | 1x |
| 50 | **JIT (exec)** | **15.1 ns** | **3.31 Gelem/s** | **907x** | **9.3x** |
| 100 | Tree-walker | 28.2 µs | 3.55 Melem/s | 1x | - |
| 100 | Bytecode (exec) | 142 ns | 704 Melem/s | 199x | 1x |
| 100 | **JIT (exec)** | **15.2 ns** | **6.58 Gelem/s** | **1,855x** | **9.3x** |
| 200 | Tree-walker | 52.2 µs | 3.83 Melem/s | 1x | - |
| 200 | Bytecode (exec) | 220 ns | 909 Melem/s | 237x | 1x |
| 200 | **JIT (exec)** | **15.8 ns** | **12.6 Gelem/s** | **3,300x** | **13.9x** |

### Boolean Operations

| Depth | Execution Tier | Time | Throughput | vs Tree-Walker | vs Bytecode |
|-------|----------------|------|------------|----------------|-------------|
| 10 | Tree-walker | 2.44 µs | 4.10 Melem/s | 1x | - |
| 10 | Bytecode (exec) | 58.7 ns | 170 Melem/s | 42x | 1x |
| 10 | **JIT (exec)** | **14.9 ns** | **671 Melem/s** | **164x** | **3.9x** |
| 50 | Tree-walker | 11.6 µs | 4.31 Melem/s | 1x | - |
| 50 | Bytecode (exec) | 97.9 ns | 511 Melem/s | 118x | 1x |
| 50 | **JIT (exec)** | **13.8 ns** | **3.62 Gelem/s** | **841x** | **7.1x** |
| 100 | Tree-walker | 22.8 µs | 4.39 Melem/s | 1x | - |
| 100 | Bytecode (exec) | 119 ns | 840 Melem/s | 192x | 1x |
| 100 | **JIT (exec)** | **14.3 ns** | **6.99 Gelem/s** | **1,594x** | **8.3x** |
| 200 | Tree-walker | 45.0 µs | 4.44 Melem/s | 1x | - |
| 200 | Bytecode (exec) | 175 ns | 1.14 Gelem/s | 257x | 1x |
| 200 | **JIT (exec)** | **15.2 ns** | **13.1 Gelem/s** | **2,960x** | **11.5x** |

### Mixed Operations (Arithmetic + Comparison)

| Depth | Execution Tier | Time | Throughput | vs Tree-Walker | vs Bytecode |
|-------|----------------|------|------------|----------------|-------------|
| 10 | Tree-walker | 3.40 µs | 2.94 Melem/s | 1x | - |
| 10 | Bytecode (exec) | 93.5 ns | 107 Melem/s | 36x | 1x |
| 10 | **JIT (exec)** | **20.0 ns** | **500 Melem/s** | **170x** | **4.7x** |
| 50 | Tree-walker | 15.1 µs | 3.31 Melem/s | 1x | - |
| 50 | Bytecode (exec) | 189 ns | 265 Melem/s | 80x | 1x |
| 50 | **JIT (exec)** | **19.5 ns** | **2.56 Gelem/s** | **774x** | **9.7x** |
| 100 | Tree-walker | 28.3 µs | 3.53 Melem/s | 1x | - |
| 100 | Bytecode (exec) | 181 ns | 553 Melem/s | 156x | 1x |
| 100 | **JIT (exec)** | **19.8 ns** | **5.05 Gelem/s** | **1,429x** | **9.1x** |

### Repeated Execution (1000 iterations, depth 100)

| Method | Total Time | Per-Iteration | Throughput | vs Tree-Walker |
|--------|------------|---------------|------------|----------------|
| Tree-walker | 27.9 ms | 27.9 µs | 3.58 Melem/s | 1x |
| Bytecode (recompile) | 7.54 ms | 7.54 µs | 13.3 Melem/s | 3.7x |
| Bytecode (precompiled) | 176 µs | 176 ns | 56.8 Melem/s | 159x |
| **JIT (precompiled)** | **17.6 µs** | **17.6 ns** | **568 Melem/s** | **1,585x** |

### Pow Operations (Runtime Calls)

| Depth | Execution Tier | Time | Throughput | vs Tree-Walker | vs Bytecode |
|-------|----------------|------|------------|----------------|-------------|
| 1 | Tree-walker | 1.66 µs | 602 Kelem/s | 1x | - |
| 1 | Bytecode (exec) | 81.9 ns | 12.2 Melem/s | 20x | 1x |
| 1 | **JIT (exec)** | **16.8 ns** | **59.5 Melem/s** | **99x** | **4.9x** |
| 3 | Tree-walker | 5.08 µs | 591 Kelem/s | 1x | - |
| 3 | Bytecode (exec) | 120 ns | 25.0 Melem/s | 42x | 1x |
| 3 | **JIT (exec)** | **26.1 ns** | **115 Melem/s** | **195x** | **4.6x** |
| 5 | Tree-walker | 8.27 µs | 605 Kelem/s | 1x | - |
| 5 | Bytecode (exec) | 139 ns | 36.0 Melem/s | 59x | 1x |
| 5 | **JIT (exec)** | **40.4 ns** | **124 Melem/s** | **205x** | **3.4x** |

## Performance Summary

### Throughput Comparison (depth 200)

| Tier | Arithmetic | Boolean | Mixed |
|------|------------|---------|-------|
| Tree-walker | 3.8 Melem/s | 4.4 Melem/s | 3.5 Melem/s |
| Bytecode | 909 Melem/s | 1.14 Gelem/s | 553 Melem/s |
| **JIT** | **12.6 Gelem/s** | **13.1 Gelem/s** | **5.0 Gelem/s** |

### Speedup Factors

| Comparison | Arithmetic | Boolean | Mixed | Pow |
|------------|------------|---------|-------|-----|
| JIT vs Bytecode | **13.9x** | **11.5x** | **9.1x** | **3.4-4.9x** |
| JIT vs Tree-walker | **3,300x** | **2,960x** | **1,429x** | **99-205x** |
| Bytecode vs Tree-walker | 237x | 257x | 156x | 20-59x |

## Files Modified in Phase 5

| File | Changes |
|------|---------|
| `src/backend/bytecode/jit/types.rs` | JitBailoutReason::NonDeterminism, JitChoicePoint, JitAlternative, JitContext fields |
| `src/backend/bytecode/jit/runtime.rs` | Runtime functions for choice points |
| `src/backend/bytecode/jit/compiler.rs` | Fork opcode rejection (bailout approach) |
| `benches/jit_comparison.rs` | Updated benchmarks |
| `examples/jit_profile.rs` | JIT profiling example |

## Running the Benchmarks

```bash
# Full benchmark suite
taskset -c 0-17 cargo bench --features jit --bench jit_comparison

# Specific groups
cargo bench --features jit --bench jit_comparison -- jit_arithmetic
cargo bench --features jit --bench jit_comparison -- jit_boolean
cargo bench --features jit --bench jit_comparison -- jit_mixed
cargo bench --features jit --bench jit_comparison -- jit_pow
cargo bench --features jit --bench jit_comparison -- repeated_execution
```

## Phase 6 Experiments

With the Phase 5 baseline established, Phase 6 will explore experimental optimizations:

1. **Phase 6.1: Pruning Strategies** - Early termination (once, depth-limited search)
2. **Phase 6.2: Parallel Exploration** - Concurrent branch evaluation
3. **Phase 6.3: Tabling/Memoization** - Cache results of non-deterministic subqueries

Each experiment will be implemented in a cascading feature branch and benchmarked against this baseline.

## Conclusion

Phase 5 successfully establishes:
- **Bailout infrastructure** for non-determinism (JitBailoutReason::NonDeterminism)
- **Runtime function framework** for potential future native Fork support
- **Baseline benchmarks** showing 10-14x JIT speedup over bytecode VM

The bailout approach ensures correctness for non-deterministic evaluation while maintaining full JIT performance benefits for deterministic code paths. The baseline benchmarks will guide Phase 6 optimization experiments.
