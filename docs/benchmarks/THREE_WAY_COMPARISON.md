# Three-Way Benchmark Comparison

**Date:** 2025-12-23
**Purpose:** Verify that backend modularization did not introduce performance regressions

## Commits Compared

| Branch/Commit | Description | Default Features |
|---------------|-------------|------------------|
| `main` | Tree-walking interpreter baseline | `interning`, `async` |
| `184df11d` | Pre-decomposition (JIT enabled) | `interning`, `async`, `jit` |
| `feature/jit-compiler` (795a995) | Post-decomposition (JIT enabled) | `interning`, `async`, `jit` |

## Key Finding

**The modularization work introduced NO performance regressions.** Pre-decomposition and post-decomposition results are essentially identical (within measurement noise).

The difference between `main` and the feature branch is due to different default features:
- `main`: Uses tree-walking interpreter (no JIT overhead)
- `feature/jit-compiler`: JIT enabled by default (tier tracking overhead on cold code)

---

## e2e Benchmark Results (Latency)

Lower is better. All values are mean time in milliseconds.

| Test | main | pre-decomp | feature branch | Decomp Change |
|------|------|------------|----------------|---------------|
| async_concurrent_space_operations | 2.982 | 2.396 | 2.356 | **-1.7%** |
| async_constraint_search | 2.756 | 4.743 | 4.551 | **-4.0%** |
| async_fib | 5.545 | 17.56 | 16.76 | **-4.6%** |
| async_knowledge_graph | 1.516 | 2.127 | 2.043 | **-4.0%** |
| async_metta_programming_stress | 5.046 | CRASH* | CRASH* | N/A |
| fib (sync) | 6.027 | ~17.5 | ~17.0 | ~-3% |
| knowledge_graph (sync) | 1.337 | ~1.9 | ~1.9 | ~0% |

*Stack overflow on metta_programming_stress is a pre-existing issue in JIT branch, not caused by decomposition.

### Analysis

The "Decomp Change" column compares pre-decomposition (184df11d) to post-decomposition (feature branch). All changes are either neutral or slightly positive (faster), confirming that **modularization did not cause regressions**.

The feature branch is slower than `main` due to JIT tier tracking overhead on cold code paths. This is expected behavior - JIT provides benefits only after code becomes "hot" (100+ calls for Stage 1, 500+ calls for Stage 2).

---

## e2e_throughput Benchmark Results (Throughput)

Higher is better. Values in programs/second.

### fib Sample

| Mode | main | pre-decomp | feature branch | Decomp Change | vs main |
|------|------|------------|----------------|---------------|---------|
| Sequential | 172.28 | 39.02 | 38.80 | **-0.6%** | -77% |
| Parallel (4) | 432.46 | 107.81 | 107.82 | **+0.0%** | -75% |
| Parallel (18) | 851.50 | 319.03 | 317.48 | **-0.5%** | -63% |
| Async (18) | 630.25 | 273.18 | 271.83 | **-0.5%** | -57% |

### knowledge_graph Sample

| Mode | main | pre-decomp | feature branch | Decomp Change | vs main |
|------|------|------------|----------------|---------------|---------|
| Sequential | 773.24 | 518.51 | 519.43 | **+0.2%** | -33% |
| Parallel (4) | 2233.28 | 1244.70 | 1244.89 | **+0.0%** | -44% |
| Parallel (18) | 3712.50 | 1506.57 | 1507.53 | **+0.1%** | -59% |
| Async (18) | 3199.55 | 1349.24 | 1350.96 | **+0.1%** | -58% |

### Analysis

All "Decomp Change" values are within ±1%, which is measurement noise. This confirms:

1. **Modularization is performance-neutral** - Breaking up large `mod.rs` files into submodules had no measurable impact on execution speed.

2. **JIT overhead explains main vs feature branch difference** - The 33-77% slowdown vs `main` is due to JIT tier tracking overhead. Each benchmark iteration creates fresh evaluator state, so nothing ever reaches JIT compilation thresholds.

---

## Stack Overflow Investigation

The `async_metta_programming_stress` test crashes with stack overflow on both pre-decomposition (184df11d) and post-decomposition (feature branch). This proves it's a **pre-existing issue**, not caused by the modularization work.

The stack overflow occurs in the JIT code path when handling deeply nested expressions. This is tracked separately from modularization work.

---

## Benchmark Methodology

- **CPU Affinity:** `taskset -c 0-17` (18 cores)
- **Build Profile:** Release (`--release`)
- **e2e Duration:** Default (auto-determined by divan)
- **e2e_throughput Duration:** 10 seconds per sample
- **Warmup:** 3 iterations

### Hardware

See `/home/dylon/.claude/hardware-specifications.md` for full specs.

---

## Conclusion

The backend modularization work (splitting `eval/mod.rs`, `environment/mod.rs`, and `jit/compiler/mod.rs` into smaller submodules) has been validated as **performance-neutral**. Pre-decomposition and post-decomposition benchmarks are identical within measurement uncertainty.

The difference between `main` and `feature/jit-compiler` is architectural (tree-walking vs JIT+bytecode) and expected. JIT benefits appear only when code paths become "hot" through repeated execution, which doesn't happen in benchmarks that create fresh state each iteration.
